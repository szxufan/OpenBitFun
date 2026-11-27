**中文** | [English](AGENTS.md)

# ACP 协议入口指南

适用范围：`src/crates/interfaces/acp`。

`openbitfun-acp` 负责基于已组装产品 runtime 的 Agent Client Protocol 入口与 ACP client 行为。ACP protocol / client 细节留在这里或应用入口 adapter 中；跨层只共享稳定 capability facts。

本 crate 暴露两个可加性的角色 feature。`client` 负责 ACP 进程发现、配置、远程探测、会话传输和
tool-card 投影，选择 Core Agent Runtime 与具体 SSH 能力；`server` 负责 CLI 托管的 ACP 服务端，
通过 `DeliveryProfile::Acp` 投影 runtime，并精确选择该路径使用的工具、文档、订阅与外部来源 owner，
但不选择 SSH。兼容默认值必须精确等于 `client + server`；Desktop 只选择 `client`，CLI 选择两者。
两个角色保持加法语义，不得用 `product-full` 替代任一闭包。

## 护栏

- Remote ACP workspace 复用本地 ACP client 配置。修改 ACP client 行为时，必须保持 manager、remote shell probing、remote capability store 和 workspace menu availability 语义。
- ACP config persistence、remote probing、timeout policy 和 workspace surface selection 属于 ACP / app-surface 行为，不要移动到 `core-types`、`runtime-ports` 或 `agent-tools`。
- ACP external-agent tool 的命名、schema、validation、presentation 和 result shape 属于 `openbitfun-agent-tools` 的 portable contract；ACP 应调用这些 helper，不要在本层重复定义。
- ACP 标准输入输出、连接管理和协议通知投影留在本 crate。共享运行时事实可以经过 SDK 边界；ACP 协议请求、客户端选择和生命周期状态不得进入 SDK。
- 如果未来需要 contract，只表达观测事实：environment identity、capability facts、request / response DTO。

## 验证

```bash
cargo check -p openbitfun-acp --no-default-features --features client
cargo check -p openbitfun-acp --no-default-features --features server
cargo test -p openbitfun-acp
cargo test -p openbitfun-acp --no-default-features --features client,openbitfun-core/git --lib client::execution::tests
```

执行状态测试覆盖请求准入回执、按 Turn 绑定取消和实时事件与重放游标的一致性。
ACP owner 先记录执行事件，再由宿主发布；执行快照查询不获取长请求持有的协议会话锁。
Desktop 注入既有的会话事件日志，通过可选的 `executionProjection: "acp"` 恢复字段标明权威来源。

元数据并发验证：
`cargo test -p openbitfun-acp --no-default-features --features client,openbitfun-core/git --lib client::manager::tests::metadata_`。
长请求持有协议接收器。切换视图读取宿主的实时快照或已保存的会话选项。显式操作可以重新连接原会话并消费待处理的更新。由唯一的接收器发布模型、命令和配置变化。

使用 `client::manager::recovery_tests` 验证会话历史持久化、冷启动查看和响应前的 ACP 历史回放。宿主先将工作区会话的终态写入正式历史，再发布完成事件并清理日志。
