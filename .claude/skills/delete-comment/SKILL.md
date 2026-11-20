---
name: delete-comment
description: Remove tautological, stale and excuse comments from a part of the Rust code, keep the ones that record a reason, file issues for excuses, and lower the comment ratchet baseline. Use when asked to clean up comments in a crate, module or file, or to repeat the comment audit.
argument-hint: <crate, directory or file to clean, e.g. crates/ulttui>
---

# Delete comments

Plain comments (`//` and `/* */`, but not `///`, `//!` or `// SAFETY:`) are
held down by a ratchet: `scripts/comment-ratchet.py` fails CI when a file has
more plain comment lines than `scripts/comment-baseline.json` allows. This
skill lowers that baseline by deleting comments that carry no information.

The 2026-11-20 audit of a 251-comment sample found about 78% tautology, 21%
local decision records, 1% excuses and no stale comments. So expect to delete
most comments and keep about one in five.

## Scope

Work on what `$ARGUMENTS` names (a crate, directory or file). Do one crate per
pull request; `ultty` is large enough to split by module.

## Classify each comment

Treat a run of consecutive comment lines as one comment. Read the code around
it before you decide.

| Kind | What it looks like | Action |
|---|---|---|
| Tautology | Says what the next lines plainly say: "// Write content" above a write loop, section dividers, a trailing label that repeats a field or value name, test narration ("// Move down twice") that the test name and code already say | Delete |
| Stale | Names code, files, behaviour or issues that no longer exist or no longer match. Check by grepping before you call it stale | Delete |
| Excuse | Justifies a hack or unfinished behaviour: "for now", "temporary", "approximate", "workaround", "not handled yet" | File a Gitea issue (`tea issues create --repo trav/ultty`) describing the shortcut and the fix, then replace the comment with one line pointing at the issue (`// Not implemented yet: #N.`) or delete it if the code is self-evident |
| Decision record | Says why: an ordering or locking rule, an invariant, a spec, xterm or kitty reference, another program's quirk, a bug it prevents, a measured budget, an ADR pointer, a test comment saying why a case matters | Keep unchanged. If the decision is architectural (crosses modules, constrains the protocol or the design) and has no ADR yet, write the ADR in `docs/adr/` and replace the comment with a pointer to it |

When a comment mixes a restatement with a reason, keep it. When unsure, keep
it. Deleting a real reason costs more than leaving a tautology.

## Rules for the change

- Comments only. No code edits, no reformatting beyond what `cargo fmt` does.
- Never touch doc comments, `// SAFETY:` comments, attributes or string contents.

## Verify

1. Every removed diff line is a comment, and the only added lines are issue
   pointers:
   `git diff -U0 | grep '^[-+]' | grep -v '^[-+][-+]'`, and read what's left.
2. `cargo fmt --all -- --check`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo test -p <crate>`.
3. `scripts/comment-ratchet.py --update` to lower the baseline, then
   `scripts/comment-ratchet.py` to confirm it passes. Commit the new baseline
   with the deletions.
4. In the pull request, give before/after comment line counts, how many you
   deleted and kept, the issues you filed, and ten deleted and ten kept
   examples so a reviewer can check the judgement.
