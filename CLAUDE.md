# rs-compiler-fuzzer: project rules

## The flow is a list of steps (`src/steps.rs`, `STEPS`)

The whole application is organised around one ordered list of unique steps that every finding goes through, always, in the same order. A step that does not apply to a finding is recorded as `n/a` with a reason (an exception, not the rule). `INDEX.md` prints the list as `k/N - what exactly is done` (`steps::list_md`), and the `steps done` column counts them.

- **Nothing happens outside a step.** Work (a check, a lookup, a verdict, a column in `INDEX.md`, a section of `STATUS.md`) that is not the result of exactly one step in `STEPS` must not exist. If something new has to be verified, add a step for it (with a version), do not bolt a value onto the output.
- **Every `INDEX.md` column is derived from the stored result of a step.** A value such as `?` means no step computed it, which is a bug: the step must be open (counted as not done), never `done` without a verdict.
- **A step is `done` only when it has its verdict.** "Could not check", "tool missing", "trunk not tested" = `todo` with the reason in the note, so the next retriage run does it again. `failed` is a negative verdict that needs a human.
- **Long work is split into steps with a measurable end**, e.g. minimisation is `minimize_10m`, `minimize_20m`, `minimize_40m`, `minimize_60m`, then `minimized` (converged). Each is done when its mark is reached, so progress is visible and an interrupted job loses at most one stage.
- **Every step has `max_secs`, its worst-case time.** A job (retriage in CI: 6 h minus setup and the closing 20 minutes) starts a step only when `max_secs` fits into the time left (`steps::fits`); a finding is picked only when one of its open steps fits (`stale_findings`). A step that does not fit stays open and is done by the next run. Set `max_secs` realistically when adding a step.
- **Steps have versions.** When the tool gets better at a step (or the list changes) bump `version` (`REDUCE_VERSION` for the minimisation steps); the step is open again for every finding. Steps with `code: true` are also open again when the reduced program changes. Update `README.md` when the list changes.
- **A reset never throws away minimisation.** After every restart of the steps (version bump, changed number of steps) the base of the next minimisation is the stored `reduced.rs`; only when that no longer reproduces the bug is the original `case.rs` the fallback. Counters (time spent, runs) restart, the program does not.

## Tests and CI are always green

- `cargo test --workspace` (debug and `--release`), `cargo clippy` and every CI job must pass. A failing test is never "not mine" or "pre-existing": if it fails, fix it (or the code it found), even when an earlier change did not cause it. Do not report a task as finished with a red test.
- A job that fails because the tool did its work (the fuzzer found a bug) is a workflow bug: map that exit code to success.
- CI jobs that run compilers in parallel install `earlyoom` (see `fuzz.yml`, `retriage.yml`): a lost runner ("lost communication with the server") is out-of-memory, not bad luck.

## Layout

- `src/fuzz.rs`, `src/strategies/*`: generation and differential testing, findings are stored with `case.rs` + `repro.cfg`.
- `src/triage.rs`: runs the steps for one finding and stores each result into `status.cfg` through `steps::set`.
- `src/pack.rs`: merges findings into the package and writes `INDEX.md`, `finding.json`.
- `src/steps.rs`: the step list, versions, completion rules, checklist.
- `tools/llvm_issue.py`: the `llvm_issue` step (culprit LLVM pass verified on release and trunk, prints `<status>|<note>`).
- `rs-minimizer/`: the reducer used by the `minimize_*` steps.
- `README.md`: user-facing description of the pipeline, CI and storage.
