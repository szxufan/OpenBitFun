# Device Grants (cross-account shared hosts)

A device grant shares **one host device of an owner account** with specific
**grantee accounts**. A team runs a dedicated machine under a shared account,
offers grants to team members, and each member drives that machine from their
own account through Peer Device Mode, with the same controller experience as
their own devices.

Read [`peer-device-mode.md`](peer-device-mode.md) for the same-account data
plane this builds on, and [`remote-surface-contract.md`](remote-surface-contract.md)
for the registry rows below.

## Trust model and invariants

1. **Two enforcement layers.** The relay only decides *reachability*: a
   cross-account RPC or event routes exclusively through a relay-resolved
   grant row. The shared host is the authoritative policy point: grant
   revalidation, mandatory PSK pairing, and observe-scope verb filtering. A
   compromised or buggy relay never alone yields control.
2. **A grant is an account; trust is a pairing.** Grants bind to a grantee
   account, but every grantee controller device must complete out-of-band PSK
   pairing with the shared machine before admission. Cross-account key
   distribution never trusts the relay directory (PSK closes the swap gap).
3. **Minimal visibility.** No existing account-scoped route is widened:
   `GET /api/devices/{id}/key`, `patch`, and `delete` are untouched. Grants
   live under `/api/grants*` with their own scope; a grantee resolves exactly
   the shared machine's key, never another owner device.
4. **Zero credential sharing.** Members hold their own account tokens; owner
   tokens never leave the owner account; invite secrets are one-time, stored
   hashed, returned exactly once, and expire after 15 minutes.
5. **E2E preserved.** The relay still sees only ciphertext; grant rows are
   routing metadata only. Traffic bills to the token owner's budget: member
   calls to the member's account, host replies to the owner's.
6. **Degrade loudly.** A relay without `device_grants_v1` answers 404 on the
   grant routes and clients hide the shared roster with an explicit
   unsupported state. A host without `device_grant_v1` is not offered grant
   flows (`PeerHostCapability`, "missing = older host").

## Relay surface

- Storage: `device_grants` (`pending | active | revoked`, scope
  `drive | observe`, optional expiry), one active grant per
  `(owner, device, grantee)` enforced by a partial unique index, ≤32
  pending+active grants per device, no payload content ever.
- `POST /api/grants` — offer, **only with the shared machine's own device
  token** (the relay checks `auth.device_id == body.device_id`), so invites
  are minted on the machine that owns the access.
- `POST /api/grants/{id}/accept` — the named grantee redeems the one-time
  secret; state, secret hash, grantee, and TTL are conditions of one UPDATE.
- `GET /api/grants` — owned (management) and received (shared-with-me roster
  with the machine's directory projection).
- `GET /api/grants/{id}/peer-key?device_id=…` — grant-scoped public-key
  lookup for the pairwise handshake and PSK MITM check.
- `DELETE /api/grants/{id}` — revoke; any owner-account device token.
- Routing: `rpc-call` carries an optional `targetAccountId`; the relay
  derives the routing room only from a grant row it resolved itself (device
  ids are per-account, so a client claim alone can never pick a room), and
  stamps the server-derived `sourceAccountId` into every delivered request.
  `machine-event` gains the symmetric `targetAccountId` for host→grantee
  pushes. Payload downloads accept a grant between uploader and downloader
  accounts instead of account equality.
- Revocation: every socket records the grants it rode; the 5-second
  authorization monitor revalidates them and disconnects like a revoked
  token. `relay-admin list-grants` / `revoke-grant` give operators an
  out-of-band path.

## Host policy (Desktop and CLI peer hosts)

`openbitfun_services_integrations::remote_connect::device_grants` owns the
shared gate. `AccountSession::authorize_peer_command` runs on **every**
relay-derived inbound `RemoteCommand` — before the host-stream read,
`ExecuteOnDevice`, the generic dispatcher, dispatch-target verbs, and
attach — not only `HostInvoke`, so no control command bypasses it. A
[`PeerTarget`] (relay, owner/grantee account, host/controller device, grant id)
is resolved from the relay-derived `sourceAccountId` and the host's own roster,
never from a client-claimed field. Exemptions are narrow: `peer_mode_ping` may
probe capabilities before pairing but still needs a current grant and never
establishes a session; `peer_psk_confirm` requires a matching PSK proof.

- **Grant roster cache** is owned per `AccountSession` (relay + account scoped),
  refreshed at most every 60 s from `GET /api/grants`, with concurrent refresh
  merged. A positively fresh roster drives admission.
- **New admissions fail closed**: no reachable relay means no new cross-account
  session (`peer grant state unavailable`).
- **Grace is a fixed deadline**: an already-admitted (relay, host, controller
  device) session keeps working until 10 minutes after its last *successful
  revalidation* (`GRANT_SESSION_GRACE`); inbound requests never extend it, and
  an expired/no-longer-present entry evicts immediately. Sessions are keyed by
  device, so a same-account controller on another device cannot inherit one.
- **Trust is a per-message PSK proof**: a cross-account command is admitted only
  when this host holds a `Paired` PSK for that controller and the message was
  sealed with the matching `key_id`; a vault `Paired` flag alone is not proof.
- **Pairing bootstrap**: the shared host mints a grant-keyed pending offer
  (`psk_store::store_grant_offer`) before it learns the grantee's controller
  device; the grantee's first PSK-sealed `peer_psk_confirm` promotes it to a
  device-scoped paired entry (`promote_grant_offer`). The grantee resolves the
  host's authenticated key through `/api/grants/{id}/peer-key`, so a grantee
  never touches the account-scoped `/api/devices/{id}/key` directory.
- **Observe scope**: `drive` grants run the full registry verdict path;
  `observe` grants allow only operations the Product Operation Registry marks
  `observe_allowed` (read/control-plane hydrate, lists, streams, workspace
  facts) plus the read-only top-level `RemoteCommand` variants; every mutation
  and `ProvisionPeerDevice` (owner-credential minting) is refused. Refusals use
  the byte-stable constants `peer grant required` / `peer grant pairing required
  ` / `peer grant state unavailable` / `peer grant scope refused`.
- **Event direction**: a host→controller `DeviceEvent` is authorized by
  `authorize_delivery` only for an already-admitted controller, so a revoked or
  grace-expired grant stops content delivery, not just RPC.


## Attribution and audit

The host appends English-only JSONL records to
`<product-home>/device-grant-audit.jsonl`: `offered`, `accepted`, `revoked`,
`attach_admitted`, `attach_refused` (with the refusal reason),
`scope_refused`, `detached`. Relay-side attribution comes from the verified
member account on each socket.

## Product Operation Registry

`peer_grant_offer` is `OperatorOnly` (minted on the machine); `peer_grant_accept`,
`peer_grant_list`, and `peer_grant_revoke` are `ControllerLocal` relay-account
calls. All four are desktop Tauri commands and are refused on the peer path —
a driving member can never mint, list, or revoke grants on the shared machine.

## Remote scenario coverage

This feature *is* the Remote control / Peer Device Mode path extended across
accounts: member controller ↔ shared machine over the relay, end-to-end
encrypted, with revocation tearing down live sockets (relay tests) and the
host gate failing closed (unit tests in `device_grants.rs`).
