# Rust Compiler Fuzzer

A differential fuzzer that looks for **miscompilations** (and compiler crashes/hangs) in `rustc`/LLVM.
Program generation and orchestration are both written in Rust (a handful of small crates: `log`, `handsome_logger`, `ctrlc`, two demanglers).

## Quick start

Needs `rustup` with a nightly toolchain (`rustup toolchain install nightly`); nothing else.

```bash
cargo install --git https://github.com/qarmin/rs-compiler-fuzzer --locked

# 10 minutes on nightly, all strategies; findings (if any) land in ./fuzz-out/findings/
rs-compiler-fuzzer fuzz --rustc "rustc +nightly" --duration 600 --profile fast
```

Exit code `0` = nothing new, `2` = new findings. Every finding is a directory with `case.rs` (a standalone reproducer),
`report.md` (commands and outputs) and `repro.cfg`. Smaller or longer runs: `--duration`, `--strategies arith,loops`, `--scale 50`;
`rs-compiler-fuzzer --help` lists everything. Already-known upstream bugs are filtered out (`known-bugs.txt`).

## Reduce a program that misbehaves (copy, paste, wait)

The reducer works on any single Rust file and needs no script: it compiles and runs the file and keeps what stands out.

```bash
# the compiler crashes (ICE, LLVM error, segfault), the compilation fails with an error, takes forever,
# or the program behaves differently at -C opt-level 0 and 1/2/3/s/z: the file says which, the log says what was found
rs-compiler-fuzzer reduce bug.rs --rustc "rustc +nightly"                          # result: bug.min.rs

# flags the bug needs
rs-compiler-fuzzer reduce bug.rs --rustc "rustc +nightly" --flags "-C opt-level=3 -Z mir-opt-level=4"

# say what to keep yourself
rs-compiler-fuzzer reduce bug.rs --rustc "rustc +nightly" --ice                    # the same crash (message + compiler source file)
rs-compiler-fuzzer reduce bug.rs --rustc "rustc +nightly" --error E0308            # that error, nothing else
rs-compiler-fuzzer reduce bug.rs --rustc "rustc +nightly" --oom 8                  # the compiler uses more than 8 GB
rs-compiler-fuzzer reduce bug.rs --rustc "rustc +nightly" --hang 60                # the compilation does not finish in 60 s
rs-compiler-fuzzer reduce bug.rs --rustc "rustc +nightly" --flags "-C opt-level=0" --vs "-C opt-level=3"   # miscompilation: the two builds differ
rs-compiler-fuzzer reduce bug.rs --rustc "rustc +nightly" --stdout "wrong value"   # the program prints this
rs-compiler-fuzzer reduce bug.rs --test ./check.sh                                # your own check: given the candidate, exit 0 while the bug shows
```

It uses every core (`--jobs`), stops after `--budget` seconds (default 600), writes the best program found so far to the output
file on every improvement, and Ctrl+C keeps it. [Shrink Ray](https://github.com/DRMacIver/shrinkray) (`pip install shrinkray`) and
[treereduce-rust](https://github.com/langston-barrett/treereduce) (`cargo install treereduce-rust`) are used when installed.
Before it touches anything it checks that the file shows the problem, so a wrong flag shows up at once. A project with several
files has to be put into one (`cargo expand`, or `rustc -Z unpretty=expanded`) first.

Details: [`rs-minimizer`](rs-minimizer/README.md).

## How it works

1. **Generate.** *Strategies* (below) emit many small, UB-free Rust programs ("cases"). Cases are packed
   into **one big file** (5k-150k lines, depending on the strategy): each case is a `mod case_N` with
   `pub fn run(h: &mut H)`, and `main` runs them one after another under `catch_unwind`, printing one line
   per case: `<id> <hash of everything the case observed>` (or `<id> PANIC`). One rustc invocation then
   costs e.g. 1 x 4 s instead of 20 x 1 s.
2. **Check.** A cheap `--emit=metadata` pass type/borrow-checks the batch. If the generator made an invalid
   case, rustc's error spans are mapped back to the case, the case is dropped (and saved to
   `generator-bugs/`) and the batch is re-assembled. Errors that only appear in codegen (e.g. a bad
   intrinsic immediate) are handled the same way after the first full compilation.
3. **Compile with many configurations** and run each binary, comparing `(exit code, signal, stdout)`:
   opt-levels 0/1/2/3/s/z, `overflow-checks`+`debug-assertions` on, `target-cpu=native` and
   `x86-64-v2/v3/v4` (only if the CPU can run them), 256 codegen units, FastISel on/off, one codegen unit,
   thin/fat LTO and, on nightly, `-Z mir-opt-level=0/3`, `-Z validate-mir`, `-Z inline-mir`,
   `-Z randomize-layout` and `O0/O1-mir-all` (explicitly enabling DestinationPropagation, DataflowConstProp, EarlyOtherwiseBranch, JumpThreading, GVN, ReferencePropagation, MatchBranchSimplification, SROA, CopyProp, Inline). The baseline is the first configuration that compiled and ran. A compile
   failure/ICE/crash in only some configurations is a finding too; compile *timeouts* are only logged.
4. **Minimise.** Because stdout is one line per case, differing lines name the culprit cases directly.
   A ddmin-style loop (bounded by `--minimize-secs`) then shrinks the batch to the cases needed to reproduce.
5. **Report.** A directory with `case.rs` (standalone reproducer incl. harness), `report.md` (commands,
   outputs, toolchain, seeds) and `repro.cfg` (used by `verify`).

### Profiles

- `--profile full` (default): every configuration.
- `--profile fast`: skips the slowest builds (fat/thin LTO, `codegen-units=1`).
- `--configs a,b,c` / `--config NAME=FLAGS` pick or add configurations; `rs-compiler-fuzzer list` shows all.

### Where findings go

| directory | meaning | fails CI? |
|---|---|---|
| `findings/` | **new**: not explained by anything known | yes |
| `known/<...>__rust-164297/` | matches already reported upstream issue(s); links in `report.md` and `known-issues.txt` | no |
| `triaged/<...>__unreported/` | understood here but not reported upstream yet (see `reports/`) | no (but stored in the release) |
| `generator-bugs/` | cases the generator got wrong (invalid code) | no |

"Known" is decided by a `// known-bug: owner/repo#N` marker in the generator snippet (all cases whose output
differs must carry one) or by a crash signature listed in [`known-bugs.txt`](known-bugs.txt) together with
the issue reference(s). Add to those when you triage something.

## Strategies

| name | what it generates | inspired by |
|---|---|---|
| `arith` | typed integer expression trees over all widths incl. 128-bit: wrapping/checked/saturating/overflowing ops, `abs_diff`, `isqrt`, `midpoint`, euclid division, rotates, shifts, casts, `matches!`/range patterns; `inline(never/always)` fns and accumulator loops | instcombine/GVN/SCCP |
| `select` | structs/tuples built by `if/else`/`match` over clamped ints with mixed int/float fields, boundary-value sweeps | #163728 (InstCombine select -> min/max) |
| `loops` | iterator pipelines, `buf[at..]` after a counter loop, nested `checked_*` loops, `%`/`min` chains, i128 score loops with `match k % M`/`is_power_of_two`/`rem_euclid`, `Vec` op sequences, array kernels | zxt5's reports #164295, #164498, #164499, #164298 |
| `control` | random structured control flow: nested `loop`/`for`/labeled blocks, `break`/`continue 'label`, `static mut` traces | #163311, #159618 |
| `recursion` | recursion combined with `max`/`min` after the call on `derive(Ord)` enums, accumulator and mutual recursion, boxed trees | #164469 (TailCallElim) |
| `enums` | pattern matching on enum tuples/guards/ranges/slices, variant remap/rebuild with shared payloads, niches | #164069, #164112, #164135, #159618 |
| `borrows` | reborrow chains, views with lifetimes, `split_at_mut`, list cursors, closures, `Cell`/`RefCell` (safe code only) | #163777, #132898, #160004 |
| `memory` | unions, `repr(packed)`, `MaybeUninit`, raw pointers, unaligned access, niches, provenance round-trips, ZST containers | #161898, #163782, #164297, #107975 |
| `traits` | `dyn`/generic dispatch, default-vs-override methods, const generics, const eval vs runtime, drop order, expression trees | #163856 |
| `asyncs` | async fns on a mini executor: state across `.await`, dropping suspended futures, join | #156008 |
| `generics` | generic fns over a numeric trait monomorphised at all 12 integer types, called directly / via fn pointers / closures, generic structs | MIR inlining and GVN across instantiations |
| `collections` | `VecDeque`/`BTreeMap`/`BinaryHeap`/`String`, sorting on many lengths/shapes, `DefaultHasher` | std code inlined per opt level |
| `floats` | IEEE arithmetic, saturating float<->int casts, fma, rounding, formatting/parsing (only operations with fully specified results) | float casts, compiler-builtins |
| `unwind` | panics (11 kinds), landing pads, drop guards, partially built arrays/vecs, nested `catch_unwind` | #144371 |
| `abi` | `extern "C"/"sysv64"/"win64"` calls, many argument/return kinds, fn pointers, values live across cross-ABI calls | #144371, #163911 |
| `simd` | SSE2/SSE4/AVX2 intrinsic chains, BMI2 `pdep`/`pext` (runtime-detected) | #164298, #164299 |

Every strategy documents its UB-freedom argument at the top of its file (`src/strategies/*.rs`). Rules:
wrapping/checked arithmetic only, masked shifts, non-zero divisors, bounded loops, inputs through `black_box`,
`unsafe` only in patterns argued valid. Adding a strategy = one file + one line in `strategies/mod.rs`; if you
cannot write a one-paragraph validity argument, do not add it. `cargo test` checks that every strategy
generates code that type-checks.

## Usage

```bash
cargo build --release

# 30 minutes on nightly, all strategies, skipping the slowest configs
./target/release/rs-compiler-fuzzer fuzz --rustc "rustc +nightly" --duration 1800 --profile fast

# one round, selected strategies and configs
./target/release/rs-compiler-fuzzer fuzz --rounds 1 --strategies loops,memory --configs O0,O3,O3-lto-fat

# reproduce a batch from the logs ("seed N" in the output), or a single case
./target/release/rs-compiler-fuzzer fuzz --batch-seed 13888278117878060724 --strategies arith --target-lines 12000 --rounds 1
./target/release/rs-compiler-fuzzer gen --strategy arith --case-seed 123 --out one.rs

# prove the detector works on known miscompilations (canaries/)
./target/release/rs-compiler-fuzzer canary --rustc "rustc +nightly" --configs O0,O3,O3-lto-fat

# re-run stored findings (dirs with case.rs + repro.cfg) on this toolchain
./target/release/rs-compiler-fuzzer verify --dir unzipped-findings/ --out verify-out
./target/release/rs-compiler-fuzzer triage --dir fuzz-out/findings/<finding> --budget 600   # status, reduction, Miri, issue search

./target/release/rs-compiler-fuzzer list      # strategies and configurations
```

Exit code: `0` nothing new, `2` new findings (or known ones with `--fail-on-known`), `1` usage/internal error.
Time control: `--duration` is a budget - no batch is started that would probably overrun it (it uses the
longest batch so far), and shrinking is capped by `--minimize-secs`.

A finding is a *candidate*: before reporting upstream, make sure the reproducer is UB-free (Miri) and shrink it
by hand. The `*-checks` configs and `PANIC` lines can also point at a generator bug.

## CI

- `ci.yml` (PR, push to master): `cargo test`, a short smoke run on nightly (`--profile fast`), an end-to-end reduction test and the
  `stdlib` self-check (every snippet of the std strategy compiles on stable and nightly, runs without a panic; API coverage >= 90 %).
- `fuzz.yml`:
  - **push to master**: nightly only, `fast` profile, half-size batches, 1 hour;
  - **00:00 UTC daily**: nightly **and** stable, `full` profile, 2 hours per job (jobs are limited to ~2h50);
  - **manual dispatch**: duration, profile, scale, strategy groups, toolchains, seed are inputs.
  One job per (toolchain, strategy group): 7 groups x 2 toolchains = 14 jobs. Each job runs the canaries,
  fuzzes, then **triages** the new findings (`triage`: reproduces on stable/beta/nightly, reduces with the
  [Shrink Ray](https://github.com/DRMacIver/shrinkray) (`pip install shrinkray`; by far the best on expression chains) after flattening the harness into a plain `fn main`, plus built-in expression passes and [treereduce-rust](https://github.com/langston-barrett/treereduce) when Shrink Ray is missing, checks the reduced
  program with Miri, searches rust-lang/rust issues with `gh`) and prints the result into the log, the step
  summary and an artifact. Every finding gets a `STATUS.md` with a **checklist** (what is done, what is open - see below) and an `ISSUE.md` draft (plus `ISSUE_TITLE.txt`) in the rust-lang format: the reduced program only, the shortest
  command lines (triage drops every flag the bug does not need: `--edition`, `-C debug-assertions`..., and tries a
  flag-less reference build), the full compiler output with demangled symbols, `rustc -vV`, and one table with the
  newest stable/beta/nightly plus the exact points of contact of the regression (e.g. `1.98.0 | no`, `1.99.0 | yes`,
  `nightly-2026-08-15 | no`, `nightly-2026-08-16 | yes`).
  Runners are `ubuntu-26.04`.
- **Storage = one asset**: the `Nightly` release holds a single `findings.zip` (`INDEX.md` + `findings/<strategy>__<config>__<hash>/`).
  `tools/release_sync.sh` does read-modify-write with optimistic concurrency: it remembers which asset was
  `findings.zip`, uploads the new archive under a temporary name, and only swaps it in if `findings.zip` is still
  the same asset; otherwise (another job wrote meanwhile) it backs off and redoes the merge on the fresh package.
  The merge (`pack`) is idempotent and the archive is deterministic, so unchanged findings produce no diff; at the end the
  keys that were stored are checked in the downloaded result.
- **Pipeline**: every fuzz job fuzzes for 45 minutes, stores the raw findings in the package, and then spends about an hour
  triaging what it found (drop unneeded cases, reduce with Shrink Ray, shortest flags, Miri, exact regression range, GitHub
  search), storing each result as soon as it is ready. `retriage.yml` (when a fuzz/miri run ends, and daily at 01:30 UTC, 4
  shards) continues with everything that has no result yet, an older `fuzzer_id`, or an unsettled minimisation; the first shard also
  verifies the stored reproducers. Schedule: fuzz ~00:00 Warsaw time (22:00 UTC), retriage 01:30 UTC, Miri 03:00 UTC.
- **Checklist instead of a verdict** (`src/steps.rs`): every stored finding says which steps are done and which are open -
  reproduced on stable/beta/nightly, **fully minimised**, shortest command line, Miri (UB) check, MIR-vs-LLVM, regression range
  (stable / nightly / the PR), upstream search, issue draft, and (manual) reported. `STATUS.md` shows it as `- [x]` / `- [~]` (started) /
  `- [ ]` / `- [!]` (failed), `INDEX.md` has "steps done" and the next open step, `finding.json` (schema 2) has the same as `steps`.
  - *Fully minimised* means: one run finished every pass before its deadlines (`complete once`) and the next run, started from that
    result, changed nothing (`converged`); at most 6 runs.
  - **Steps are reset when they should be**: every step has a version. When the tool gets better at it (a new reduction pass:
    `REDUCE_VERSION`, a better bisection ...) the version is bumped and the step is open again for every finding. Steps that depend
    on the program (everything except the minimisation itself) also carry a hash of the reduced program and its command line: as soon
    as a new minimisation changes the code, they are open again; if the minimisation did not touch the code, they stay done. The upstream
    search is repeated after 14 days.
  - `retriage.yml` (when a fuzz/miri run ends, and daily at 01:30 UTC, 4 shards) continues findings with open steps - closest to
    completion first, each at most once per job - and only runs what is open (a converged minimisation is not repeated, a known regression
    range is kept, `bisect.txt`/`search.txt` hold the details). Findings stored before the checklist existed get it derived from the fields
    they have.
- **`finding.json`**: the standard machine readable description of a stored finding (strategy, kind, minimal flags, reduced program hash,
  regression edges, reproduction per channel, checklist ...), rewritten whenever `status.cfg` changes. When two jobs triaged the same
  finding concurrently, the result with more finished steps wins; the same reduced program found twice is merged (`reduced_hash`).
- **Self-cleaning package**: the first shard of every `retriage.yml` run re-runs each stored reproducer on the newest nightly (the
  reference). A finding that no longer reproduces there is marked `not-reproducing`; the next check removes it (two in a row, so one odd
  run cannot delete anything). Removed findings are listed in `fixed.txt` and in the "Removed" table of `INDEX.md`, and a triage that
  was running at the time cannot bring them back. There is no separate verify workflow any more: the retriage run does the verification.

## Reports

`reports/` holds minimised findings worth reporting upstream (currently: an LLVM `-O0` crash on i128 arithmetic).

## Reference material

Open `I-miscompile` issues are the model for what to generate:
<https://github.com/rust-lang/rust/issues?q=state%3Aopen%20label%3A%22I-miscompile%22>.

## MIR or LLVM, and the offending PR

- Every reproducing finding is retried on nightly with `-Zmir-opt-level=0` for both configurations. The result is in `STATUS.md`
  ("MIR optimisations or LLVM?"), `status.cfg` (`mir_opt`) and the `ISSUE.md` draft: the difference remains = not a MIR optimisation (LLVM or
  codegen); it disappears = a MIR optimisation is involved.
- When the bisection over nightlies ends with two consecutive days, `cargo bisect-rustc` (installed by the fuzz and retriage jobs, uses
  `GITHUB_TOKEN`) narrows them down to the commit/PR, with its own 7 minute allowance. The PR links go to `ISSUE.md` and `status.cfg`
  (`regression_pr`); if it does not finish, the dates stay the answer.

## The `stdlib` strategy: the whole safe std API

`cmp::max` alone was enough to trigger a miscompilation, so `stdlib` calls (nearly) every safe, stable, deterministic function of
the standard library on edge-case inputs: integer methods of all 12 widths (checked/wrapping/saturating/overflowing/strict, bit
tricks, `ilog`, `isqrt`, `midpoint`, `NonZero`, `Wrapping`, `Saturating`), float methods with exact results (and the libm functions,
quantised so that last-bit differences cannot cause false findings), `char`/`str`/`String`, slices, arrays, `Vec`, `VecDeque`, every
`Iterator` adapter and consumer (also on a hand written iterator so the trait's default methods run), `Option`/`Result`, `cmp`,
`mem`, cells, `Rc`/`Arc`/`Box`/`Cow`/`Pin`, ranges, `Duration`, `BTreeMap`/`BTreeSet`/`BinaryHeap`/`LinkedList`/`HashMap`/`HashSet`
(order independent observations only), `fmt` (`Formatter`, the `debug_*` builders, all format traits on user types), in-memory `io`,
atomics, `Mutex`/`RwLock`/`OnceLock`/channels/threads used deterministically, `CString`/`CStr`, `ptr`.

- The snippets are tables of one-line Rust statements (`src/strategies/stdlib_*.rs`); a case is a prelude (edge-case values) plus a few
  random snippets, each in its own block. Left out: `unsafe` functions, I/O on the real system, time, addresses, hasher randomness.
- `tools/stdcov.py` measures the coverage from the rustdoc JSON of core/alloc/std (needs `rustup component add rust-docs-json --toolchain
  nightly`): every public, stable (in the current stable release), non-unsafe function or method is compared with the calls in the tables
  (by name; the output lists what is missing). Currently ~98 % of the in-scope names.
- `tools/stdlib_selfcheck.sh [table ...]` compiles and runs each table on stable and nightly (`RSFUZZ_STDLIB_TABLE=<name>` forces one
  table); `rs-compiler-fuzzer list` prints the table names. Run by the `stdlib` CI job.

## Regression search and issue draft details

- **No arbitrary search limits**: the stable search goes back to 1.56 and the nightly search to 2021-06-01 - the oldest compilers that can
  compile the (edition 2021) programs at all, not a chosen window. If the bug is already there, the finding is classified as
  *long-standing* (step "Regression or long-standing bug?") and the bisection steps are skipped; if the oldest compiler cannot
  compile the program, the table says so ("reference build does not compile") and a caveat is added.
- **The offending commit** (`cargo bisect-rustc`) is only searched for regressions. Not finding it is a result too: "regressed commit
  not found" (a timeout is not: that one is tried again).
- **Code minimisation and case reduction are separate steps** ("Batch reduced to the cases that matter", "Code minimised"); the reduced
  program uses `println!` (one value per line).
- **`ISSUE.md`** shows `rustc reduced.rs <flags>; ./reduced` and lists the same program at `-C opt-level=0/1/2/3/s/z`: which levels show
  the bug and which do not (step "Other optimisation levels tried").

## Minimising by hand on a strong machine (`just`)

```
just download                                   # findings.zip from the Nightly release -> ./findings, ./INDEX.md, ./fixed.txt
just open                                       # findings that still have open checklist steps
just minimize findings/arith__Oz__4a1a1f9e41    # all cores, up to 4 h per pass, repeated until two runs agree (max 12 h / 8 runs)
just minimize findings/X --jobs 20 --budget 7200 --bisect 3000 --search
just triage findings/X                          # the whole checklist again (short reduction, bisection, upstream search)
just bisect findings/X                          # regression search only
just issue findings/X                           # print ISSUE.md
just index                                      # rebuild INDEX.md
```

`minimize` reduces even a finding that already "converged" (a longer run on more cores may get further; `--no-force` skips those). The
smallest program that still shows the bug is written to `reduced.rs` the moment it is found, also while Shrink Ray works. **Ctrl+C once**
stops the reduction and writes everything up for what was reached (`reduced.rs`, `ISSUE.md`, `STATUS.md`, `status.cfg`, `finding.json`, `INDEX.md`;
the reproduction on the toolchains and the flag search still run, which can take a few minutes for big programs); **Ctrl+C twice** leaves at
once - `reduced.rs` is still the best program, and `just triage` or the next `just minimize` rewrites the documents. Running `minimize` again
continues from `reduced.rs`. Steps that depend on the program (flags, opt levels, Miri, bisection) are reset automatically when it changed.
The result is not uploaded: copy the directory into the release package (or open the issue from `ISSUE.md`).
