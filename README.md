# Rust Compiler Fuzzer

A differential fuzzer that looks for **miscompilations** (and compiler crashes/hangs) in `rustc`/LLVM, plus a reducer and a triage pipeline that turns every finding into a ready-to-review upstream issue draft. Everything is written in Rust.

## Quick start

Needs `rustup` with a nightly toolchain and nothing else.

```bash
cargo install --git https://github.com/qarmin/rs-compiler-fuzzer --locked

# 10 minutes on nightly, all strategies; findings land in ./fuzz-out/findings/
rs-compiler-fuzzer fuzz --rustc "rustc +nightly" --duration 600 --profile fast
```

Exit code `0` = nothing new, `2` = new findings, `1` = error. A finding is a directory with `case.rs` (standalone reproducer), `report.md` (commands and outputs) and `repro.cfg`. Useful options: `--duration`, `--strategies arith,loops`, `--scale`, `--profile fast|full`, `--configs`; `rs-compiler-fuzzer --help` and `rs-compiler-fuzzer list` show everything.

## How it works

1. **Generate.** Strategies emit many small, UB-free programs, packed into one big file (one `mod case_N` per case, one output line per case) so a single rustc call checks many cases.
2. **Compile with many configurations** (opt-levels, overflow checks, target CPUs, codegen units, FastISel, LTO, MIR opt levels and passes, `i686`) and compare `(exit code, signal, stdout)`. Any difference, or a crash/ICE in only some configurations, is a finding.
3. **Shrink the batch** to the cases that matter, then **report** the finding.
4. **Triage** (see below) reproduces, minimises and classifies every finding and writes `ISSUE.md`, and for LLVM culprits `LLVM_ISSUE.md`.

Strategies (`rs-compiler-fuzzer list`): `arith`, `select`, `loops`, `control`, `recursion`, `enums`, `borrows`, `memory`, `traits`, `asyncs`, `generics`, `collections`, `floats`, `unwind`, `abi`, `simd`, `shapes` (recent upstream reports), `stdlib` (nearly the whole safe std API). Each strategy documents its UB-freedom argument at the top of `src/strategies/*.rs`; adding one is a single file plus one line in `strategies/mod.rs`.

## Reducing your own program

The reducer works on any single Rust file and needs no script. In a terminal it shows a dashboard; details in [`rs-minimizer`](rs-minimizer/README.md).

```bash
rs-compiler-fuzzer reduce bug.rs --rustc "rustc +nightly"                       # auto-detects ICE / error / hang / opt-level difference, result: bug.min.rs
rs-compiler-fuzzer reduce bug.rs --rustc "rustc +nightly" --flags "-C opt-level=3"
rs-compiler-fuzzer reduce bug.rs --rustc "rustc +nightly" --ice                 # or --error E0308, --oom 8, --hang 60, --stdout "text"
rs-compiler-fuzzer reduce bug.rs --rustc "rustc +nightly" --flags "-C opt-level=0" --vs "-C opt-level=3"
rs-compiler-fuzzer reduce bug.rs --test ./check.sh                              # your own check, exit 0 while the bug shows
```

## Triage: the list of steps

Every finding goes through the same ordered list of steps (`src/steps.rs`, printed at the top of `INDEX.md`): reproduction on stable/beta/nightly, minimisation (10/20/40/60 minutes, then convergence), shortest command line, opt-level and MIR-level matrix, Miri (UB) check, MIR-or-LLVM, culprit pass (bisection), regression range and offending commit, `LLVM_ISSUE.md` and the `ISSUE.md` draft. A step is `done`, `n/a` (with a reason), `failed` (needs a human) or open. Steps have versions and a time budget (`max_secs`); a new version reopens the step for every finding. Rules for changing the list are in `CLAUDE.md`.

Findings of the Miri job (`miri.yml`: the output of Miri differs from the native build, or Miri reports undefined behaviour) go through the same steps. Miri is one more way of running a program (`Config::miri`, the `miri` driver called directly, `src/miri.rs`): reproduction and minimisation compare the native build with Miri on nightly (stable and beta are `n/a`), the steps that vary compiler options vary the native build against Miri's output, a report of Miri itself does not depend on them (`n/a`), and the regression range is `n/a` (it would need Miri on every searched nightly). The verification on the newest nightly archives a Miri finding that is gone, like any other.

`LLVM_ISSUE.md` (`tools/llvm_issue.py`) re-runs the culprit pass with the previous LLVM release, the one rustc uses and LLVM trunk (binaries from apt.llvm.org) and says `FIXED ON TRUNK` or `STILL WRONG ON TRUNK`. Machine passes `opt` cannot run (`stack-coloring`) are compared with `llc` and the pass switched off. Then `llvm_reduce` (`tools/llvm_reduce.py`) shrinks the IR with `llvm-reduce` while the correct build keeps its output, and `trunk_fix` (`tools/llvm_fix_search.py`) lists merged llvm-project PRs that could have fixed a `FIXED ON TRUNK` bug (candidates only). Check the IR on <https://alive2.llvm.org/ce/> before filing.

The tool does not search upstream for duplicates (that could hide real, different bugs). Findings with the same kind, target, culprit pass and regressing commit are folded into one row of `INDEX.md`; the rest stay in `findings/`.

## CI and storage

- `ci.yml`: tests, a smoke fuzz run, a reduction test and the `stdlib` self-check.
- `fuzz.yml`: fuzzing on nightly (daily ~22:00 UTC, on push, or manually), one job per strategy group plus `i686` jobs, then triage of what was found.
- `retriage.yml`: continues findings with open steps, verifies stored reproducers on the newest nightly and moves fixed ones to `old_findings/`.
- `miri.yml`, `import.yml`: Miri over generated programs, and import of hand-made reproducers (`todo.zip` in the `Nightly` release: `NAME.rs` + `NAME.txt` with the rustc command line).
- Storage is one asset: the `Nightly` release holds `findings.zip` (`INDEX.md` + `findings/<strategy>__<config>__<hash>/`), updated by `tools/release_sync.sh` with optimistic concurrency.
- Findings are sorted into `findings/` (new), `known/` (matches `known-bugs.txt` or a `// known-bug:` marker), `triaged/` (understood, not reported) and `generator-bugs/`.

Each finding directory also has `STATUS.md` (checklist), `status.cfg` and `finding.json` (machine readable).

## Working by hand (`just`)

```bash
just download                       # findings.zip -> ./findings, ./old_findings, ./INDEX.md
just open                           # findings with open steps
just minimize findings/<name>       # long reduction on all cores, continues from reduced.rs
just triage findings/<name>         # the whole checklist again
just fuzz-lto | just canary | just miri | just stdlib-selfcheck
```

`reports/` holds minimised findings worth reporting upstream, `canaries/` known miscompilations that prove the detector works.
