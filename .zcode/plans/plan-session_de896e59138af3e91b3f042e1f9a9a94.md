# PSK 设备配对（方案 1）实施计划

## 目标与安全模型

在不动 relay 服务器的前提下，为 Peer Device Mode 增加带外预共享密钥（PSK）配对：两台设备通过扫码/复制配对码人工交换一个 256 位 PSK，PSK 混入设备间消息密钥派生。恶意 relay 即使替换目录公钥也无法派生密钥（解密必败、攻击可见），将信任模型从 trust-on-first-use 升级为带外验证。

四条硬规则（安全性的根基，测试要锚定）：
1. **旧 KDF 字节不变**：未配对的设备对完全走现状路径，新旧客户端互通不受影响。
2. **配对后绝不静默回退**：`key_id` 标明 PSK 变体却解密失败时，报"配对密钥不匹配"错，绝不降级重试旧 KDF（降级正是 MITM 想要的）。
3. **`key_id` 是解密变体的唯一裁决**：信封带 `key_id` → 必须用 PSK 派生；不带 → 一律旧 KDF（即使已配对）。这保证配对切换瞬间和单侧撤销时双方不出现解密歧义。
4. **响应用与请求相同的变体**：宿主应答沿用该请求解密所用的密钥（每请求选钥后自然成立）；主动扇出的 device_event 在已配对时用 PSK 变体。

## 密码学设计（Rust 与 TS 双端对等）

新函数 `derive_message_key_psk(secret, peer_public, psk)`（放在 `device_crypto.rs`，旧函数不动）：
- `shared = X25519(secret, peer_public)`，保留 `was_contributory` 低阶点检查
- `ikm = shared(32B) || psk(32B)`
- 新域分隔盐 `PSK_KDF_SALT = b"OpenBitFun Relay v1.1.0 device key psk"`（不碰旧 `KDF_SALT`）
- `info = 排序后双方公钥(64B) || psk_id(8B)`；`psk_id = SHA256(b"OpenBitFun PSK id v1" || psk)[..8]`
- 输出 32 字节，沿用现有 AES-256-GCM 封装

加密信封在 `{encrypted_data, nonce}` 基础上增加可选字段 `key_id: "psk_v1:<psk_id_hex>"`（relay 不透明转发，旧端忽略未知字段，无服务器改动）。

配对码（带外载荷，93 字节二进制 → Crockford Base32 约 149 字符，5 字符一组）：`版本1B || psk 32B || 发起方 device_id 16B || 发起方公钥 32B || psk_id 8B || CRC32 4B`。QR 形式编码为 `<relay>/#/pair?did=<id>&k=<base32>`（纯 fragment，不发给服务器）。完成方将码内嵌的发起方公钥与 relay 目录取回的公钥比对——不一致即警告"检测到可能的中间人"并拒绝，这是配对时刻的 MITM 检测点。

配对流程：A 生成码（状态 offered）→ 用户经自有可信渠道带到 B → B 粘贴/扫码，校验指纹后存 PSK（状态 paired）→ B 发 `peer_psk_confirm {psk_id, offerer_device_id}`（用 PSK 变体加密，解密成功本身即证明配对成立）→ A 收到并解密成功后置 paired。此后双向 PSK 变体。撤销 = 删除本地配对记录，回退旧 KDF（对端下次看到无 `key_id` 信封自动走旧路径）。

## 改动清单

### 1. Rust 核心（services-integrations，feature `remote-connect`）
- `remote_connect/device_crypto.rs`：新增 `derive_message_key_psk` + `psk_id`；测试：新旧 KDF 互异、对称性、低阶点、**新 KAT 固化向量**（旧向量 `6e8f5da...` 保持不动证明无回归）。
- 新文件 `remote_connect/psk_store.rs`：仿 `remote_ssh/password_vault.rs` 模式，`relay-v1.0.0/psk_vault.key`（随机 32B，0600）+ `psk_vault.json`（AES-256-GCM 条目 map，fs2 锁，原子写）。条目键 `sha256(relay_url|user_id|peer_device_id)`，值 `{psk, psk_id, status: offered|paired, created_at, peer_name}`。API：`set_offered/set_paired/remove/list/lookup`。
- `remote_connect/account.rs`：`peer_message_key` 增加按状态分派（paired→PSK 变体，否则旧路径）；`encrypt_for_peer` 返回值携带 `key_id`（改为小型 `PeerSealedMessage{data,nonce,key_id:Option<String>}`）；`decrypt_from_peer` 增加 `key_id` 入参并按规则 2/3 分派；新增 `PairingKeyMismatch` 明确错误文案常量；配对状态变更时清 `peer_keys` 缓存（复用现有 reconnect 清空逻辑）。
- 加解密 7 个调用点铺开 `key_id`：desktop `remote_connect_api.rs`（扇出 495、RPC 响应 649、事件循环 2724、发送 3078）、CLI `account.rs`（收 455、发 811、fanout.rs 837）。

### 2. 能力声明与操作注册表（contracts → 生成物）
- `remote_surface/capabilities.rs`：`PeerHostCapability` 新增 `PskPairingV1`（wire key `psk_v1`），进 `ALL`/`DESKTOP_CAPABILITIES`/`CLI_CAPABILITIES`。
- `remote_surface/table.rs`：新行 `op("peer_psk_confirm", Agnostic, HostControlPlane, HANDLED)` 且 `cli_peer: HANDLED`；desktop 新 Tauri 命令 `peer_psk_offer_create / peer_psk_complete / peer_psk_remove / peer_psk_list` 各加行（分类对齐 `account_device_rpc` 的先例，闭包测试会强制）。
- 运行 `cargo run --bin export_remote_surface_registry` + `pnpm run capabilities:generate` 再生成 web-ui 绑定（`remoteSurface.test.ts` digest 校验通过）。

### 3. Desktop（src/apps/desktop）
- `peer_host_invoke.rs`：`peer_mode_ping` 能力自动生效；新增 4 个 Tauri 命令（生码/完成配对/撤销/列表），完成配对时校验目录公钥指纹、写 psk_store、发 `peer_psk_confirm`。
- `remote_connect_api.rs`：`execute_local_remote_command` 增加 `peer_psk_confirm` 处理（解密成功 + psk_id 匹配 → 本地置 paired，向 webview 发状态刷新事件）。

### 4. CLI（src/apps/cli）
- `peer_host/dispatch.rs` `handle_control_plane`：处理 `peer_psk_confirm`（对称逻辑）。
- CLI 配对命令（如 `openbitfun account pair --offer <peer-device-id>` / `--complete <code>`，按 CLI 现有命令结构落位）：生码/输码走同一 psk_store，使桌面↔CLI 可配对。

### 5. 共享 TS 契约（src/shared/relay-transport）
- `DeviceEventEnvelope` 类型加可选 `key_id`；`HostStream.ts` 加 `PSK_KEY_MISMATCH_MESSAGE` 常量（仿 `UNSUPPORTED_HOST_MESSAGE`）；`RelayFailure.ts` 分类器加 `pairing-key-mismatch`。

### 6. mobile-web（TS 控制端）
- `E2EEncryption.ts`：新增 `deriveDeviceMessageKeyPsk` + `pskIdOf`（与 Rust 逐字节对等）；`account-login.test.mjs` 加**与 Rust 相同的新 KAT 向量**。
- 新 `PeerPskStore`（IndexedDB，按 `relayUrl|deviceId` 键，存 psk/psk_id/status）；`RelayHttpClient.deviceMessageKey` 按 same 规则分派、`sendDeviceRpc`/`receiveDeviceEvent` 读写 `key_id`、解密失败分类。
- `pairingLink.ts` 解析 `k` 参数；配对完成入口（DevicesPage 操作 + 配对链接落地提示），文案进 `messages.ts` 自有 i18n（不抄 Web UI）。

### 7. web-ui（桌面控制器 UI）
- `PeerConnectionManager.ts`：`PeerHostCapabilities`/`NO_CAPABILITIES`/`probeCapabilities` 加 `pskV1` 映射；`peerCapabilityResolution.ts` 加 `canUsePskPairing`。
- 新配对对话框（复用 `qrcode.react` 展示 QR + 可复制配对码 + 粘贴完成 + 已配对列表/撤销），入口放 `DeviceSurfaceSwitcher` 设备行菜单，仅对声明 `psk_v1` 的在线 peer 显示；配对状态徽标（offered/paired）。
- i18n：web-ui 三 locale（en-US/zh-CN/zh-TW）新增文案；仅用现有主题 token，不加新色。

### 8. 文档
- `docs/architecture/peer-device-mode.md`：新增"PSK 配对与信任模型"一节（规则 1–4、配对码格式、撤销语义、剩余威胁：配对码渠道安全 = 用户责任、relay 仍可 DoS、原生移动端暂走旧 KDF）。
- `src/web-ui/src/infrastructure/peer-device/README.md`：新增不变量"已配对解密失败不得回退旧 KDF；`key_id` 是变体唯一裁决"。

## 明确非目标
- relay-service：**零改动**（已核实转发不透明、登录忽略未知字段）。
- 原生 Kotlin/HarmonyOS 端不移植：不配对则永远旧 KDF，互通不受影响，文档注明。
- 不做全局"强制所有 peer 配对"开关（仅按设备对可选）。

## 测试与验证（按 owner 就近）
- `cargo test --locked -p openbitfun-services-integrations --no-default-features --features remote-connect --lib remote_connect::`（KAT、psk_store、account 分派/错误）
- `tests/remote_connect_contracts.rs` 新增：无 `key_id` 旧信封双端可解（升级兼容）、PSK 双向往返、psk_id 不匹配报错、老载荷 round trip
- `cargo test -p openbitfun-desktop --lib api::remote_connect_api` 与 `api::peer_host_invoke::tests`；`cargo test -p openbitfun-cli --bin openbitfun peer_host::`
- web-ui：`pnpm --dir src/web-ui run test:run`（PeerConnectionManager、remoteSurface、配对对话框）；mobile-web：`account-login.test.mjs` 等 node:test
- 收尾：`pnpm run capabilities:check`、`pnpm run fmt:rs`、`pnpm run check:core-boundaries`、web-ui `i18n:audit`

## 实施顺序
1. Rust KDF + psk_store + account.rs 分派与信封（含全部调用点）
2. 能力/注册表 + 重新生成绑定
3. desktop 命令与 confirm 处理 → CLI 命令与 confirm 处理
4. 共享 TS 契约 → mobile-web 加密/存储/入口
5. web-ui UI + i18n
6. 契约测试补齐 + 文档 + 收尾检查