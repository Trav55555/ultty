# Porygon workspace qualification receipt

Final client `897e26e` and daemon `b9d524dc` pass all six ANSI gates at 80/160 columns and the native GPU scenario. Exact hashes, metrics and limits are in summary.json. Independent verdicts are in review/INDEPENDENT_REVIEW.md.

The provider-free tests use actual authenticated FastAPI routes and SQLite stores. They cover session drafts/restart, older-session paging, work result/evidence, bound conversation attach, approval inspection followed by one exact ID/version denial, and 10,000-message navigation/editing. Native checks also use actual system clipboard paste, raw reply copy and parsed review JSON copy through a fresh shell. Three final screenshots were visually inspected.

The frozen incumbent fails the same final six ANSI80 gates. Candidate 2 history/pixel evidence is documented separately; final approval acceptance is candidate 3. Original failures and complete raw captures remain in the original qualification directory. This compact bundle excludes binaries, databases, private keys and authentication-token logs.

The integration agent added build logs and Terra's final SSH receipts under
`build/` and `ssh/`. SSH exercised authenticated OpenSSH against a private
loopback server at both widths. Its preserved runner uses the original local
paths and a separate environment containing Paramiko 3.5.1; it is not one of the
portable wrappers below. The PostgreSQL log records the isolated migration and
claim SQL check. `SHA256SUMS` covers the final combined evidence directory.

Top-level fixtures match the tested frozen copies. tested-fixtures/ also preserves those copies and the exact original runners. The top-level wrappers were made portable after the final run and passed bash syntax checks. They accept environment paths:

```sh
export PORYGON_SOURCE_ROOT=/path/to/porygon-ai
export PORYGON_VERIFIER_PYTHON=/path/to/prepared/venv/bin/python
export PORYGON_ARTIFACT_ROOT=/path/to/fresh-results
export TIKTOKEN_CACHE_DIR=/path/to/prepared/tokenizer-cache
bash run.sh replay /path/to/frozen/porygon-tui 80
bash run.sh replay /path/to/frozen/porygon-tui 160
bash run-native.sh replay /path/to/frozen/porygon-tui /path/to/frozen/ultty /path/to/frozen/ulttyctl
```

Requirements are uv, the project test dependencies, a prepared tokenizer cache, Linux PTY/local sockets, Xvfb, xdotool, and a working Vulkan backend. No repository build runs. Labels are exclusive and never overwrite an earlier receipt. Native Unix sockets use a short private /tmp runtime path. Source defaults remain local convenience values; provide the environment paths when replaying elsewhere.

Timings contain test-driver and polling overhead. They are neither frame-latency measurements nor a human task trial. The daemon review notes optional legacy version binding, at-most-once consumption rather than exactly-once side effects, and live OFFSET pages rather than immutable snapshots. PostgreSQL migration/claim SQL was checked separately; the full PostgreSQL daemon was not exercised.
