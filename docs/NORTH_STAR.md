# Project North Star

## One-sentence promise

Make Ultty a credible Unix-first public-alpha terminal emulator: ordinary terminal workflows are correct and inspectable, native protocol/TUI features demonstrate real capability uplift, and every release claim is backed by reproducible evidence.

## Users and jobs

- Maintainers need a dogfoodable terminal they can run from source while debugging correctness, renderer, and native-runtime issues.
- Technical alpha users need installable prerelease builds with explicit limitations, observable runtime state, and reproducible smoke evidence.
- TUI developers need dual-mode apps that behave correctly in ordinary ANSI terminals and gain semantic capability inside Ultty-native mode.

## Success signals

- Core local checks pass: format, check/test, clippy with warnings denied, security audit when part of a release gate.
- GUI/live smoke and terminal-workflow smoke artifacts exist for release candidates.
- `ulttyctl` exposes useful runtime state such as tabs, cwd, screenshot, and protocol/session health.
- Public alpha checklist entries cite concrete commit-bound evidence rather than standing intentions.
- Known limitations are explicit: Unix-first runtime, alpha maturity, GUI/live-smoke requirements, and unsupported Windows runtime.

## Strategic bets

- Terminal correctness comes before visual richness.
- Runtime inspectability makes dogfood and external bug reports actionable.
- The native protocol carries semantic UI primitives, not arbitrary remote drawing commands.
- `ulttui` remains a dual-mode product commitment: ANSI mode is real, while Ultty-native mode adds capability uplift.
- Release readiness is evidence-led: claims should be tied to dated checks, artifacts, and known limitations.

## Non-goals

- Do not claim general-use readiness from unit tests or local polish alone.
- Do not freeze the pre-1.0 native protocol when a better semantic model is needed.
- Do not optimize native UI richness at the expense of ordinary terminal workflows.
- Do not expand Windows runtime support without a dedicated platform boundary.
- Do not turn the native protocol into a generic desktop GUI/remoting system.

## Constraints

- Unix-first runtime: Linux and macOS are supported targets; Windows requires future ConPTY/native transport work.
- No protocol/schema/API authority without an explicit sprint permission and acceptance criteria.
- CI and local validation should keep warnings meaningful; do not weaken checks to pass.
- Public release claims require concrete evidence: CI, security audit, package/install smoke, GUI/live smoke, performance notes, and known limitations.
- Agent work must respect repo instructions, approval gates, and dirty-tree hygiene.

## Off-track signals / kill criteria

- New feature work begins while a completed dirty sprint has not been reviewed, archived, or turned into commit candidates.
- Release notes make readiness claims without fresh evidence tied to a candidate commit.
- Native protocol or renderer changes regress ordinary shells, editors, pagers, resize behavior, tabs, splits, copy mode, or paste behavior.
- Abstractions hide terminal behavior instead of making it easier to test and inspect.
- The project accumulates broad uncommitted churn that cannot be explained as small vertical slices.

## Plan hierarchy

### Long horizon

Reach a credible Unix-first public alpha, then move toward general use only after repeated release evidence, broader terminal-workflow confidence, and external dogfood feedback.

### Mid horizon

Close readiness gaps in this order:

1. durable direction and repo hygiene;
2. validated dogfood/runtime polish;
3. GUI/live smoke evidence;
4. release-candidate checklist evidence;
5. terminal workflow smoke matrix;
6. runtime maintainability slices.

### Short horizon

Prefer closeout and evidence work before new features: preserve material work history, keep `.pi/work-loop.md` current, split dirty diffs into reviewable commit candidates, and run GUI/live smoke where available.

## Sprint decision filter

Choose work that best answers: does this make Ultty more correct, inspectable, dogfoodable, and evidence-backed without expanding protocol/runtime authority or dirty-tree risk beyond the approved sprint?
