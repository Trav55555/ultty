# Tailscale design research for Ultty and Ulttui

Research date: 2026-11-12. Repository inspected at `b75af7ac35423e7103a1a9065ce64f118ae0d428`. Research only. No client installation, network/account/policy change, experiment, code change, or deployment was performed.

## Decision

Use an existing Tailscale installation as optional infrastructure and keep ordinary SSH as the first remote transport. Ultty can improve remote terminal work through semantic UI, session diagnostics, and bounded local control. Building another mesh VPN would put the project into identity provisioning, routing, NAT traversal, relay operations, revocation, and cryptographic maintenance before improving terminal correctness.

The strongest first candidate is an Ulttui diagnostic application or small local adapter that explains which layer failed: local daemon, peer reachability, SSH authorization, PTY, native handshake, or application. It can work in ANSI and gain richer inspection inside Ultty. This is a proposal, not an implemented improvement or measured advantage.

External SSH is also the control against which an embedded transport must justify itself. A separate Go `tsnet` service remains a plausible later option for an application that actually needs its own network identity. Official Rust embedding now exists, but its current MSRV and missing features rule out direct adoption under the present Ultty contract. [T13][T17][T18]

## Evidence and scope

`CLAUDE.md`, `AGENTS.md`, `docs/NORTH_STAR.md`, `docs/PROTOCOL_SPEC.md`, `crates/ultty-client/src/detect.rs`, `crates/ultty-client/src/in_band.rs`, and the local control/MCP implementation were read. The north star prioritizes a correct, inspectable Unix terminal and dual-mode framework, and requires separate sprint authority for protocol/schema/API changes.

Brain-2 was searched before web research. The search found existing notes on WireGuard signaling limits, Serve versus Funnel audience, and Aperture's approval controls. Those were discovery leads. The technical findings below were refreshed against official documentation and upstream source. The local memory registry had no relevant Tailscale/Ultty hit.

Evidence labels used below:

- **Documented** means an official product/API statement was read. It does not prove behavior on the user's machines.
- **Source** means a particular upstream source or repository document was inspected. Main-branch URLs are moving references, not frozen or audited dependency snapshots.
- **Inference** means the proposed Ultty design follows from those observations.
- **Unverified** means a runtime or release gate remains open.

## Network architecture and connectivity

**Documented.** The coordination service distributes device identities, keys, configuration, and policy. Clients execute the data plane, including WireGuard encryption and filtering. Keeping control traffic separate from application traffic removes the coordination service from the normal terminal byte path. It still remains an important authority and availability dependency for management. [T01]

Current connectivity has three paths: direct UDP, a tailnet-owned peer relay, or a DERP relay. Direct usually provides better latency and throughput. Peer relays can reduce the performance cost of fallback. All three carry end-to-end encrypted WireGuard traffic. DERP is also involved in discovery and transitions, so a relay observation during connection startup does not establish the final path. `tailscale ping`, `status`, and `netcheck` answer different diagnostic questions. [T02][T03][T04]

**Inference.** Ultty should keep a connection's logical identity separate from its changing network endpoint. The UI should report observed path and observation time, rather than announce permanent direct connectivity. A terminal can survive a brief path change only if its transport, PTY, and application lifetimes survive as well. Tailscale connectivity alone does not establish durable shell sessions, transcript history, or resumable native scene state.

**Documented limits.** During coordination failure, locally held keys and cached policy can keep existing connectivity useful until key expiry. Enrollment, key refresh, policy changes, and revocation are unavailable. The broader control/data-plane overview describes continuing pre-established connectivity more conservatively than the outage FAQ. Do not turn either description into a guarantee that any fresh peer connection or cold start works offline. [T05][T01]

The client changelog reports improved cached-netmap startup in 1.104.1, lossy-network fixes for SSH/Serve/tsnet in 1.104.5, and a dormant-connection fix in 1.104.6 on November 10. These changes make client version part of a useful connection receipt. They do not qualify Ultty recovery behavior. [T06]

## Identity, authorization, and trust

Tailscale delegates user authentication to an identity provider and binds user or tag identity to nodes. Tagging a previously user-owned machine changes its identity and ownership semantics. Tags act as service identities, not cosmetic labels. Multiple local processes using a host's network identity are not individually authenticated people. [T07][T08]

Grants are the preferred current policy mechanism. They cover network permissions and application capabilities; ACLs remain supported. Rule evaluation is deny-by-default, but this is easy to misread: a new or uncustomized tailnet can retain an explicit permissive default policy. Network membership is therefore insufficient evidence of narrow access. Inspect the effective rules. [T09][T10][T11]

Application capabilities provide data an application can use for finer permissions. The destination must implement their semantics. The documented interfaces include LocalAPI, `tsnet`, Serve capability headers, and `whois` for debugging. A custom application should use a capability name under a domain its operator controls. [T12]

**Inference for Ultty.** Separate these decisions:

1. May this node connect to an adapter?
2. Which authenticated principal or service identity is the caller?
3. Which pane and operation may it access?
4. Has the local user authorized the resulting terminal action?

Reading text, taking screenshots, sending keys, running commands, changing overrides, and accessing a clipboard have different authority. A tailnet grant does not justify exposing every existing `ulttyctl` operation. A future remote adapter needs a deliberately smaller command set and a way to terminate active access. This requires a separate design and sprint gate.

Tailnet Lock adds customer-controlled signatures for node admission and reduces trust in the coordination service's public-key distribution. It uses an initial trust step and introduces signing-key and recovery-secret operations. It is not enabled by default, and device approval and Tailnet Lock are mutually exclusive in the current docs. Persistent state reduces startup dependence on fetching its authority history. [T14]

**Inference.** Signed admission and application permissions remain different layers. Tailnet Lock does not make a compromised authorized endpoint safe, grant pane-level authorization, or solve revocation distribution during a partition. Do not create a second Ultty key-authority system merely to imitate the mechanism.

## SSH and terminal sessions

Tailscale SSH handles tailnet SSH authentication/authorization in `tailscaled`, taking port 22 for incoming tailnet traffic. It requires both network permission and an SSH rule. Its check mode can request fresh identity-provider approval. Official docs describe policy revocation terminating existing connections once the update arrives. Traditional `sshd` over Tailscale remains available where its OS-user and key restrictions are a better match. [T15]

This distinction matters on shared hosts. A node's identity does not provide process-by-process user separation, and official SSH guidance warns about untrusted code and differing local users' access. Do not assume `authorized_keys` forced-command restrictions govern the built-in Tailscale SSH server. [T15]

Ultty already has a relevant transport design. The local Unix socket is tied to the owning tab/TTY and pane. When it is unavailable, the client probes the actual terminal and can carry frames as APC strings through SSH. It avoids probing known tmux/screen contexts and falls back to ANSI. In-band mode currently requires a single-pane tab, confirms a live reader after the handshake, and expires after ten seconds of silence. Clipboard reads are deliberately unavailable to in-band sessions. These are current local source/spec observations, not a live Tailscale qualification. [L01][L02][L03]

**Inference.** Preserve those restrictions over Tailscale. Forwarding the privileged local native/control socket to a remote machine would discard the reason for the existing in-band trust boundary. A long network pause can leave a shell alive while expiring its native session. Recovery must detect that state and re-handshake or continue in ANSI; it must not replay old input into a shell.

In-band base64url framing adds roughly one third to frame payload size before envelope and other transport overhead. This is an arithmetic cost, not a measured bottleneck. An embedded binary stream only earns its complexity if measured remote native workloads lose enough responsiveness or bandwidth to justify another transport.

## Naming, private services, and public sharing

MagicDNS maps device names and tailnet-qualified names, reducing dependence on remembered IPs. It is naming infrastructure, not proof that an application is healthy or a caller authorized. Machine renames change DNS names. [T19]

HTTPS adds browser-compatible TLS on top of the encrypted network path. Provisioned public certificates publish the machine's fully qualified name in Certificate Transparency. Sensitive hostnames therefore need review before certificate issuance. The naming disclosure is distinct from network access. [T20]

Serve exposes a local backend to the tailnet and obeys network access rules. HTTP proxying can forward user identity and selected application-capability headers. It strips incoming spoofed versions; tagged clients have no user-identity headers. Capability forwarding is documented since 1.92. Backends must validate those capabilities and prevent clients bypassing the proxy. Localhost binding reduces remote bypass but still trusts local processes unless another boundary is added. A port's last Serve/Funnel configuration determines whether it is private or public. [T21]

Funnel intentionally exposes a selected service to the public internet, including clients with no Tailscale account. It lacks Serve's caller identity headers. TLS does not supply application authorization. A public terminal would need its own authentication, session authorization, abuse limits, and recovery design. Funnel is currently documented as beta. [T22]

**Inference.** Serve is a reasonable optional front door for a future read-only diagnostic HTTP service. It is not necessary for SSH/APC, and the current Ultty native protocol is not an HTTP service. Do not turn the local control socket into a publicly reachable endpoint to obtain convenient URLs. Funnel should have no automatic role in terminal setup.

## Daemon and embedding choices

Routing and sharing features have different authority from a terminal session:

| Feature | Documented scope | Ultty implication |
|---|---|---|
| Subnet router | Makes approved IP prefixes reachable through a gateway, including devices without a client. Route approval and access rules are distinct setup steps. [T35] | May reach an existing SSH host; does not give the target a new authenticated terminal/session identity |
| Exit node | Routes public-internet/default-route traffic through a selected node; advertising, administrative approval and client selection are explicit steps. [T36] | Changes networking beyond the terminal; never silently select one to repair an SSH connection |
| App connector | Routes selected application-domain traffic rather than all internet traffic. [T37] | Infrastructure configuration, not a substitute for application authorization |
| Link-based device sharing | Gives a specific external Tailscale user access to a machine under access policy. Shared machines are quarantined by default and do not export their subnets. Current docs call link sharing beta and declarative cross-tailnet sharing alpha. [T38] | Machine reachability does not grant a pane, shell, or every service; use existing approved shares rather than create invitations |
| Tailscale Service | Stable internal service name/address can route to multiple approved tagged hosts and survive a host move. [T39] | Useful future discovery for a diagnostic service; host failover does not migrate a running PTY or native scene |

The normal Linux client installs host networking through a TUN interface. Userspace `tailscaled` instead offers SOCKS5/HTTP proxy access where a TUN device is unavailable. Other applications must deliberately use the proxy. This is not equivalent to adding a host-wide network interface or making every process tailnet-aware. [T16]

Go `tsnet` embeds a userspace Tailscale node. `Listen`/`Dial` return normal Go networking interfaces. Per-application nodes can have separate IPs and policies without a system daemon. This makes service identity an application deployment decision. [T13]

**Source.** Current `tsnet.go` exposes persistent storage, auth-key/bootstrap choices, and ephemeral registration. Existing enrolled state normally takes precedence over a new bootstrap auth key. Separate node instances need separate state and hostnames. It also exposes `ListenSSH` with an `SSHAuthCallback`; the comment says a nil callback admits sessions from peers permitted by TCP rules. Do not equate this API with the complete `tailscaled` SSH policy behavior. These observations require a frozen release/source review before implementation. [T17]

**Source, current Rust option.** Official `tailscale-rs` currently advertises the 0.7 line, BSD-3-Clause licensing, Rust 2024 and supported MSRV 1.97. It is experimental with no compatibility guarantee. TCP/UDP, public DERP and direct communication in many cases are implemented. NAT traversal remains in progress. Its unsupported list includes MagicDNS, private DERP, peer relays, application capabilities, automatic key rotation, Tailnet Lock, SSH, Serve/Funnel, and network-flow logs. That differs materially from a feature-complete host client. [T18]

The 0.7.1 main manifest deliberately sets a different Cargo compiler floor, 1.92.0. Its comment distinguishes that hard floor from the README's rolling support policy. Both exceed Ultty's 1.90 contract. This is a verified manifest mismatch, not evidence that 1.97 is the oldest compiler capable of building the crate. [T34]

Its architecture uses API bindings, a runtime actor/message bus, control client, and data-plane components. `smoltcp` implements userspace networking; transport and tunnel components are separate crates. These are useful separations to study, but copying that actor/crate structure into Ultty without matching concurrency needs would add navigation cost. [T23]

The September 0.6 changelog reports a third-party cryptography audit of `ts_tunnel`, remediation of security-relevant findings, and removal of the older security caveat/environment gate. The November 0.7.1 release also records ongoing netstack fixes. Describe it as an evolving library with a reported scoped audit, not as unaudited or mature parity with the Go client. [T24]

## Audit, observability, and operational limits

Configuration audit logs record configuration writes and policy diffs, with 90-day retention. Current docs exclude read-only actions and some support-initiated changes, and promise no maximum ingestion delay. This is not a complete command or terminal activity record. [T25]

Network-flow logs record successful network activity metadata, not contents or denied attempts. They are available for 30 days on specified plans and are not a live online/idle monitor. Therefore they cannot prove that an agent issued a particular shell command or that an attempted connection was denied. [T26]

SSH recording is separately configured, stores asciinema sessions, and fails open by default when recorders are unavailable unless enforcement is enabled. Required recording changes availability behavior. Ordinary ANSI playback also needs qualification for Ultty APC/native output before anyone calls it an accurate semantic UI recording. That last point is an inference and an open test. [T27]

Ultty can add application evidence that networking logs cannot supply: pane/session identifiers, selected transport, handshake state, command completion when shell integration supplies it, bounded diagnostic history, and exact failure time. The existing control server and MCP adapter already offer a local foundation. Capturing typed secrets or full terminal contents by default is not necessary for a useful health receipt. [L04]

## Broader product families

Tailscale PAM currently has beta architecture documentation. Connectors proxy services, inject credentials, and terminate sessions. Policy can condition access on identity, device state, resource and timing; session logs/recording are explicit product capabilities. This is a privileged-access system with its own service/session model. A future Ultty launcher could launch an already-authorized PAM SSH workflow and display its expiry/recording status, subject to a verified supported client contract. It should not reproduce credential issuance or approvals. [T28]

Aperture is now a separate AI gateway product. Its August GA announcement describes upstream credential isolation, controls, MCP/API proxying, shared projects, and approval-gated machine access. Current documentation distinguishes configuration, grants, and upstream credentials; reaching its network endpoint alone grants no model or MCP permissions. [T29][T30]

The transferable design is to separate tool availability, caller permission, and upstream credentials. Ultty's local MCP transport is not an AI gateway. An optional configured Aperture client belongs in an application or integration layer and needs its own evidence for approvals and logs. Neither PAM nor Aperture needs to become a dependency of the terminal or framework.

## Integration tradeoffs

| Approach | Job it fits | Benefits | Costs and limits | Verdict |
|---|---|---|---|---|
| Existing client plus traditional SSH | Remote shell and current Ulttui APC mode | No new network protocol, existing SSH tooling and OS-user controls, works with current Rust contract | User installs/enrolls client, network and SSH failures remain separate, no automatic durable session | First control and preferred starting point |
| Existing client plus Tailscale SSH | Identity-governed remote shell | Tailnet identity, check approval, centralized SSH policy | Different host/user trust semantics, destination/client variant restrictions, daemon/session/version coupling | Supported workflow to qualify separately |
| Serve in front of a narrow HTTP adapter | Read-only health/status or a deliberately scoped application | TLS, policy and trusted-proxy capability data | Requires new HTTP adapter and application enforcement, local bypass boundary, certificate naming disclosure | Only after a concrete HTTP use case |
| Go tsnet sidecar plus local IPC | App-specific identity without host VPN changes | Per-service enrollment/state, established Go networking API, process isolation from Rust renderer | Extra artifact/runtime, IPC permissions, independent enrollment and updates, persistent identity lifecycle | Later bounded prototype if host client is inadequate |
| Go tsnet embedded through FFI | One process containing Rust and Go | Avoids separately managed process | FFI/runtime/packaging/shutdown complexity and renderer/network crash coupling | Sidecar must fail a real constraint first |
| Official tailscale-rs | Native Rust embedded node | Tokio-facing sockets, no Go runtime, explicit network components | MSRV mismatch, experimental API, substantial current feature gaps | Watch or isolated out-of-workspace research only |
| Independent SSH/TCP networking | Existing hosts reachable through user-managed networks | No Tailscale account requirement, simple standard transports | User handles routing/identity; no automatic mesh/NAT reachability | Preserve as an ordinary supported path |
| Independent mesh VPN/control plane | Whole-device private networking | Maximum ownership in theory | Recreates identity, keys, relays, NAT, revocation and operations | Outside Ultty's job |

The first four rows solve different user problems. Comparing installation convenience alone would hide the authority and lifecycle costs.

## Reusable design lessons

- Keep the high-volume data path independent of configuration service availability. Give cached authority a visible age and expiry so offline continuity does not look like fresh authorization.
- Prefer a logical session identity over a transport endpoint. Network path migration, process restart, and app reconnect have different lifetimes.
- Make trust conversion explicit. A remote peer should become a bounded application session only after identity, operation permission, and local consent checks.
- Isolate enrollment and network state from the renderer when their lifecycle does not match. Recreating a terminal window should not silently create another network node.
- Make fallback real. ANSI fallback and ordinary SSH are valuable only when failed probing, timeouts and transport changes leave them usable.
- Observe the failed layer. A global green connected badge cannot distinguish node discovery, stale policy, SSH denial, or expired native UI.
- Preserve partial evidence. Configuration logs, packet-flow metadata, terminal bytes, and semantic application events answer different questions.

These are design inferences. Tailscale's implementation validates the value of examining the boundaries; it does not establish that these changes will improve Ultty without measurement.

## Proposed experiments and stop rules

These are unrun, bounded proposals. Any live enrollment, test-policy modification, relay setup, or new remote control endpoint needs its own authorized scope. No protocol redesign is required to start with the first two.

| Candidate | Expected payoff and incumbent | Cheapest decisive falsifier | Fixed first budget and acceptance | Stop/rollback |
|---|---|---|---|---|
| SSH/APC qualification | Establish whether existing transport already delivers remote semantic UI. Control is same app over ordinary SSH and ANSI fallback | On one current Linux peer, native probe fails or silently sends native input after the app exits | One pair of authorized test machines, one frozen client/repo version, 90 minutes. Check ordinary shell, one native example, resize, exit, interrupt, reconnect and tmux fallback; capture transport/pane evidence | Stop on misdirected input, unsafe fallback, or lost ordinary workflow. Fix existing transport before proposing networking |
| Connection diagnostics app | Faster explanation of remote failure. Control is current CLI/manual diagnosis | Two of a small fixed failure set remain indistinguishable, or diagnostic sampling stalls rendering | One workday. Recorded fixtures for absent daemon, login needed, peer unavailable, SSH denial, native timeout; separate approved live confirmation. Bounded subprocess timeouts and unknown-field handling | Delete adapter if it only repeats a green badge or requires admin credentials |
| Narrow Serve backend | Prove an actual app-permission use case. Control is SSH-run local diagnostic CLI | Spoofed header, direct backend access, or tagged caller obtains user privilege | Half-day in isolated test app. Allowed user, denied user, tagged service, forged headers, capability absent, recorder/policy update timing where relevant | Stop if trustworthy source identity cannot be established. Remove adapter/Serve config |
| tsnet sidecar | Remove host-wide networking only for users who need it. Control is installed daemon plus SSH | Reopening the terminal enrolls another node; restart loses identity; unauthorized caller reaches local control | One day, isolated process/package and state directory, no native protocol expansion. Measure artifact/RSS/startup and state persistence; exercise denial and cancellation | Keep it out of default build unless it meets a documented missing user need |
| tailscale-rs review | Evaluate whether a future Rust route avoids sidecar burden. Control is Go tsnet sidecar | Required MSRV or essential capability remains incompatible | Two-hour dependency/API/source assessment before any network run. Freeze version and capability requirements | Current assessment already rejects direct integration at MSRV gate; do not silently bump the workspace |
| Long-pause/path-change qualification | Check whether terminal/native states recover coherently. Control is current SSH without native UI | A pause longer than ten seconds resumes the shell but leaves a dead or input-stealing native scene | One approved test pair, 60 minutes, fixed direct/relay/pause cases and versions. Record shell, TCP, and native-session outcomes independently | End at the first unsafe state; preserve logs and reproduction, no blind retry loop |

Before any performance claim, freeze the workload and compare local socket, ordinary SSH/APC, direct tailnet SSH/APC, and observed relay conditions on the same app. Use input-to-visible response, bytes/frame, first-frame time, memory and failure recovery. Do not infer perceived terminal latency from bulk VPN throughput.

## Remaining uncertainty

No installed client, actual policy, IdP, remote host, user privilege model, recording deployment, offline startup, or network transition was inspected. No Ultty benchmark, packet capture, reconnect experiment, or threat-model review was run. Source pages were not frozen to dependency commits. Tailscale plan availability and beta status are dated observations. Rust cryptography audit coverage is an upstream report, not an independent review performed here.

The useful next gate is whether the existing SSH/APC path survives a small realistic terminal workflow matrix. A positive result favors better diagnostics and documentation before another transport.

## Primary source ledger

All web entries were accessed 2026-11-12. Product docs and release notes are upstream statements; repository entries are source/document inspection.

| ID | Primary source | Why it matters |
|---|---|---|
| T01 | [Control and data planes](https://tailscale.com/docs/concepts/control-data-planes) | Authority, local enforcement, offline wording |
| T02 | [Connection types](https://tailscale.com/docs/reference/connection-types) | Direct, DERP, peer relay, encryption and inspection |
| T03 | [Device connectivity](https://tailscale.com/docs/reference/device-connectivity) | NAT combinations and netcheck |
| T04 | [Troubleshoot DERP routing](https://tailscale.com/docs/reference/troubleshooting/network-configuration/derp-routing) | Relay-to-direct transitions and ping observations |
| T05 | [Coordination server unavailable](https://tailscale.com/docs/reference/coordination-server-down) | Cached keys/policy and unavailable revocation |
| T06 | [Client and service changelog](https://tailscale.com/changelog) | 1.104.1 caching, 1.104.5/6 connection fixes |
| T07 | [Tailscale identity](https://tailscale.com/docs/concepts/tailscale-identity) | Node, user, tag and LocalAPI identity |
| T08 | [Group devices with tags](https://tailscale.com/docs/features/tags) | Ownership changes, SSH limits and expiry defaults |
| T09 | [Access control](https://tailscale.com/docs/features/access-control) | Grants/ACL scope |
| T10 | [Grants vs. ACLs](https://tailscale.com/docs/reference/grants-vs-acls) | Current policy direction and coexistence |
| T11 | [Manage permissions using ACLs](https://tailscale.com/docs/features/access-control/acls) and [grant examples](https://tailscale.com/docs/reference/examples/grants) | Default allow-all policy qualification |
| T12 | [Application capabilities](https://tailscale.com/docs/features/access-control/grants/grants-app-capabilities) | Destination enforcement and integration APIs |
| T13 | [tsnet documentation](https://tailscale.com/docs/features/tsnet) | Embedded Go userspace node |
| T14 | [Tailnet Lock](https://tailscale.com/docs/features/tailnet-lock) | Signed admission, recovery, state and limitations |
| T15 | [Tailscale SSH](https://tailscale.com/docs/features/tailscale-ssh) | Network/SSH permission distinction and shared-host warning |
| T16 | [Userspace networking](https://tailscale.com/docs/concepts/userspace-networking) | Proxy use rather than host interface |
| T17 | [tsnet.go source](https://github.com/tailscale/tailscale/blob/main/tsnet/tsnet.go) | State/bootstrap and embedded SSH callback contract |
| T18 | [tailscale-rs README](https://github.com/tailscale/tailscale-rs) and [license](https://github.com/tailscale/tailscale-rs/blob/main/LICENSE) | Current maturity, MSRV, features and license |
| T19 | [MagicDNS](https://tailscale.com/docs/features/magicdns) | Name resolution and renaming |
| T20 | [Enabling HTTPS](https://tailscale.com/docs/how-to/set-up-https-certificates) | Certificate Transparency disclosure |
| T21 | [Tailscale Serve](https://tailscale.com/docs/features/tailscale-serve) | Proxy identity/capability headers, bypass and audience |
| T22 | [Tailscale Funnel](https://tailscale.com/docs/features/tailscale-funnel) | Public ingress and beta status |
| T23 | [tailscale-rs architecture](https://github.com/tailscale/tailscale-rs/blob/main/ARCHITECTURE.md) | Actor runtime, smoltcp and separated networking components |
| T24 | [tailscale-rs changelog](https://github.com/tailscale/tailscale-rs/blob/main/CHANGELOG.md) | Audit scope, version/state changes and recent fixes |
| T25 | [Configuration audit logging](https://tailscale.com/docs/features/logging/audit-logging) | Retention, contents, exclusions and timing |
| T26 | [Network flow logs](https://tailscale.com/docs/features/logging/network-flow-logs) | Metadata, plan/retention and denied-attempt exclusions |
| T27 | [Tailscale SSH session recording](https://tailscale.com/docs/features/tailscale-ssh/tailscale-ssh-session-recording) | Fail-open/closed behavior and asciinema |
| T28 | [PAM architecture and concepts](https://tailscale.com/docs/privileged-access-management/architecture-and-concepts) | Distinct proxy/credentials/session product |
| T29 | [Aperture GA, August 26, 2026](https://tailscale.com/blog/aperture-ga) | Gateway, MCP and approval claims |
| T30 | [Control AI access](https://tailscale.com/docs/aperture/control-access) and [Aperture grants](https://tailscale.com/docs/aperture/how-grants-work) | Gateway reachability versus tool/model authority |
| T31 | [Open source at Tailscale](https://tailscale.com/opensource) and [Go client license](https://github.com/tailscale/tailscale/blob/main/LICENSE) | Open client/DERP versus proprietary hosted control plane |
| T32 | [Security bulletins](https://tailscale.com/security-bulletins) and [October SSH findings/fixes](https://tailscale.com/blog/tailscale-ssh-findings-fixes) | Version-sensitive server and forwarding boundaries |
| T33 | [Tailscale CLI](https://tailscale.com/docs/reference/tailscale-cli) | `status --json` is useful but explicitly subject to format change |
| T34 | [tailscale-rs Cargo manifest](https://github.com/tailscale/tailscale-rs/blob/main/Cargo.toml) | 0.7.1, edition 2024 and hard rust-version 1.92.0 versus rolling supported MSRV |
| T35 | [Subnet routers](https://tailscale.com/docs/features/subnet-routers) and [devices without Tailscale](https://tailscale.com/docs/reference/devices-without-tailscale) | Gateway access, route approval and permissions |
| T36 | [Exit nodes](https://tailscale.com/docs/features/exit-nodes) | Public traffic routing and explicit selection |
| T37 | [Route traffic](https://tailscale.com/docs/route) | Subnet, exit and app-connector distinction |
| T38 | [Share machines with other users](https://tailscale.com/docs/features/sharing) | Quarantine, specific recipients, subnets and maturity |
| T39 | [Tailscale Services](https://tailscale.com/docs/features/tailscale-services) | Stable service identity and approved backend hosts |

Licensing/self-hosting qualification: upstream publishes core client and DERP code, while the hosted coordination service is proprietary. Headscale is an independent community-maintained control-plane implementation with its own scope and operations. Self-hosting DERP alone does not replace coordination. Before redistributing an embedded dependency, review the exact release's licenses, third-party notices, and service terms. This memo is not a legal compatibility determination. [T31]

Current security bulletins describe SSH and Serve fixes in 1.104.4. A future qualification should record an up-to-date patched client release; an old installation should not be treated as the product behavior described here. [T32]

Local references are relative to the repository revision above:

- L01: [Protocol transport and trust](../../docs/PROTOCOL_SPEC.md), sections 1.2 and 1.3.
- L02: [Terminal detection](../../crates/ultty-client/src/detect.rs).
- L03: [In-band transport](../../crates/ultty-client/src/in_band.rs).
- L04: [Control server](../../crates/ultty/src/control/server.rs), [control paths](../../crates/ultty/src/control/paths.rs), and [MCP adapter](../../crates/ultty/src/control/mcp.rs).

Brain-2 discovery references, not substitutes for current primary evidence:

- `03-resources/networking/wireguard-split-tunnel-and-overlapping-subnets.md`
- `03-resources/ai-ml/local-ai-and-agent-supervision-six-thread-follow-up.md`
- `03-resources/ai-ml/aperture-ga-model-tokens-mcp-controls-projects-and-custom-permissions.md`
- `03-resources/ai-ml/approval-gated-agent-automation.md`
