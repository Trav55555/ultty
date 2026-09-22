# Release Candidate Evidence

Committed evidence for release candidate checklists, one directory per run
date: `docs/releases/evidence/<YYYY-MM-DD>/`.

A release candidate checklist may cite only two kinds of evidence:

1. A full CI run URL plus the name of the uploaded artifact.
2. A repository-relative path under this directory, with the files committed.

Scratch paths such as `/tmp/...` or `$RUNNER_TEMP/...` are not evidence. They
are cleared, so nobody can check the claim afterwards.

When a gate is run locally, copy the summary file, the relevant logs, and the
screenshots you intend to cite into a dated directory here and commit them.
Keep it small: cite the specific files that support the claim, not every byte
the smoke produced.

Screenshots are normally ignored by the repository `.gitignore`. PNG files
under this directory are exempt so evidence captures can be committed.
