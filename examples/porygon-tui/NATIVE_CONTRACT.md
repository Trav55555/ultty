# Porygon daemon connection

The native client requires `PORYGON_TOKEN` and accepts `PORYGON_URL` (default `http://127.0.0.1:18789`). Remote URLs require HTTPS. Supply the token through the environment; do not put it in the URL. The client authenticates `/status` before accepting a prompt.

Build this excluded example with:

```sh
cargo build --locked --manifest-path examples/porygon-tui/Cargo.toml
cargo test --locked --manifest-path examples/porygon-tui/Cargo.toml
cargo clippy --locked --manifest-path examples/porygon-tui/Cargo.toml --all-targets -- -D warnings
```

Each explicit new turn gets a client-generated run ID, displayed in the transcript before sending. Submission uses `/agent/stream`, with authentication in the first frame. The client never automatically resends a prompt after an uncertain outcome. Run IDs are not daemon idempotency keys.

`/reconnect` checks authentication and reads the known run. `/attach <run_id>` reads an explicitly selected run without starting work. Neither action reattaches to live token output. A parked `awaiting_input` run can receive an explicit reply. Stop remains requested until the daemon confirms the disposition. A local turn generation fences older stream events and HTTP responses.

`/approvals` opens the current decision or fetches the durable approval queue. Sending a websocket decision does not prove the tool completed. An uncertain durable decision cannot be submitted again: refresh only reads the pending queue. Full arguments remain available; missing risk and reversibility are labelled unknown.

The guarded `--verify-native-contract` mode is for the Porygon repository's explicit `tests/native_contract_e2e.py` fixture. That test starts its own loopback daemon with synthetic credentials and a deterministic session, and sets `PORYGON_CONTRACT_FIXTURE=1`. The probe sends two fixture prompts and cancels one; do not run it against a normal daemon.

Current qualification covers transport and presentation-state tests, including a compiled client against real daemon routes. Native/ANSI rendering, keyboard focus, resize, SSH/tmux, actual tool admission, and a live model are separate gates. This client does not yet persist an attach journal: retain the displayed run ID for recovery after a client restart. Legacy auxiliary commands and status widgets remain outside this contract milestone.
