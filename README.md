# Rust Compiler Fuzzer

A differential fuzzer that looks for **miscompilations** (and compiler crashes/hangs) in `rustc`/LLVM.
Program generation and orchestration are both written in Rust (std only, no dependencies).

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

- `ci.yml` (PR, push to master): `cargo test` and a short smoke run on nightly (`--profile fast`).
- `fuzz.yml`:
  - **push to master**: nightly only, `fast` profile, half-size batches, ~12 minutes - quick feedback;
  - **00:00 UTC daily**: nightly **and** stable, `full` profile, 2 hours per job (jobs are limited to ~2h50);
  - **manual dispatch**: duration, profile, scale, strategy groups, toolchains, seed are inputs.
  One job per (toolchain, strategy group): 7 groups x 2 toolchains = 14 jobs. Each job runs the canaries,
  fuzzes, then **triages** the new findings (`triage`: reproduces on stable/beta/nightly, reduces with the
  built-in passes + [treereduce-rust](https://github.com/langston-barrett/treereduce), checks the reduced
  program with Miri, searches rust-lang/rust issues with `gh`) and prints the result into the log, the step
  summary and an artifact. Every finding gets a `STATUS.md` with a verdict - `READY TO REPORT` /
  `ALMOST READY` (with the open items) / `NOT READY` (with the blockers) - and an `ISSUE.md` draft.
  Runners are `ubuntu-26.04`.
- **Storage = one asset**: the `Nightly` release holds a single `findings.zip` (`INDEX.md` + `findings/<strategy>__<config>__<hash>/`).
  `tools/release_sync.sh` does read-modify-write with optimistic concurrency: it remembers which asset was
  `findings.zip`, uploads the new archive under a temporary name, and only swaps it in if `findings.zip` is still
  the same asset; otherwise (another job wrote meanwhile) it backs off and redoes the merge on the fresh package.
  The merge (`pack`) is idempotent and the archive is deterministic, so unchanged findings produce no diff; at the end the
  keys that were stored are checked in the downloaded result.
- `verify.yml` (daily, or manual): per channel (nightly, beta, stable) re-runs every stored finding and records
  `repro.<channel>`, `last_verified` and `state` (`open`/`fixed`) in its `status.cfg`, via the same sync. Nothing is deleted.

## Reports

`reports/` holds minimised findings worth reporting upstream (currently: an LLVM `-O0` crash on i128 arithmetic).

## Reference material

Open `I-miscompile` issues are the model for what to generate:
<https://github.com/rust-lang/rust/issues?q=state%3Aopen%20label%3A%22I-miscompile%22>.
