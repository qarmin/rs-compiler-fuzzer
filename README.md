# Rust Compiler Fuzzer

A differential fuzzer that looks for **miscompilations** in `rustc`/LLVM. Everything - program
generation and orchestration - is written in Rust (std only, no dependencies).

## How it works

1. **Generate.** Several *strategies* (see below) emit many small, UB-free Rust programs ("cases").
   Cases are packed into **one big file** (~20 000 lines by default): each case is a `mod case_N`
   with `pub fn run(h: &mut H)`, and `main` runs them one after another under `catch_unwind`,
   printing one line per case: `<id> <hash of everything the case observed>` (or `<id> PANIC`).
   One rustc invocation then costs e.g. 1 x 4 s instead of 20 x 1 s.
2. **Check.** A cheap `--emit=metadata` pass type/borrow-checks the batch. If the generator made
   an invalid case, rustc's error spans are mapped back to the case, the case is dropped (and saved to
   `generator-bugs/`) and the batch is re-assembled.
3. **Compile with many configurations** (opt-levels 0/1/2/3/s/z, `codegen-units=1`, fat LTO,
   `target-cpu=native`, overflow-checks/debug-assertions on, and on nightly `-Z mir-opt-level=0/3`,
   `-Z validate-mir`), run each binary, compare `(exit code, signal, stdout)`. The first configuration
   (`O0`) is the baseline. A compile failure/ICE/hang in only some configurations is a finding too.
4. **Minimise.** Because stdout is one line per case, differing lines name the culprit cases directly.
   The orchestrator then shrinks the batch with a ddmin-style loop (re-compiling only the baseline and
   the diverging configuration) until only the cases needed to reproduce remain.
5. **Report.** `fuzz-out/findings/<ts>_<strategy>_<config>_<n>/` gets `case.rs` (a standalone
   reproducer with the harness), and `report.md` (exact commands, outputs, toolchain, seeds).

## Strategies

| name | what it generates | inspired by |
|---|---|---|
| `arith` | typed integer expression trees over all int widths incl. 128-bit (wrapping/checked/saturating/overflowing, rotates, shifts, casts, `if`), wrapped in `inline(never/always)` fns and accumulator loops | simple programs; instcombine/GVN/SCCP bugs |
| `loops` | iterator pipelines (`step_by`, `skip_while`, `scan`, `rev`, `cycle().take`, `try_fold`...), `buf[at..]` after a counter loop, nested `checked_*` loops, `%`/`min` chains, `Vec` op sequences, fixed-size array kernels (auto-vectorisation) | zxt5's reports (#164295, #164498, #164499, #164185...) |
| `control` | random structured control flow: nested `loop`/`for`/labeled blocks, `break`/`continue 'label`, early `return`, bool state machines, `static mut` traces | #163311, #159618 (SimplifyCfg/EarlyOtherwiseBranch/JumpThreading) |
| `enums` | pattern matching over enums with data: tuple scrutinees, guards, ranges, `@` bindings, or-patterns, slice patterns | #159618 (EarlyOtherwiseBranch), match-tree MIR opts |
| `borrows` | reborrow chains through nested structs, views with explicit lifetimes + `split_at_mut`, `Option<Box<Node>>` cursors, closures capturing by ref/mut/move, `Cell`/`RefCell`, two-phase borrows, `mem::{swap,replace,take}` - **all safe code** | #163777, #132898, #160004 (ReferencePropagation/GVN) |
| `memory` | unions with sibling writes, `repr(packed)` consts/statics, `MaybeUninit` stacks, raw-pointer walks, unaligned reads, niches (`NonZero`, `Option<Box>`), `transmute`, exposed-provenance round trips | #161898, #163782, #164297, #163825, #107975 |
| `traits` | default-vs-override methods through `dyn`/fn pointers/generics, associated consts, const generics, `const fn` evaluated at compile time **and** runtime, drop-order scenarios (shadowing, temporaries, forget, closures), boxed expression trees with an ownership-consuming simplifier | #163856, const-eval vs codegen |
| `asyncs` | async fns/blocks on a mini executor: locals and borrows across `.await`, drop guards (also of half-polled futures), nested calls, `join` | coroutine transform, #156008 |
| `collections` | `VecDeque`/`BTreeMap`/`BinaryHeap`/`String` operation sequences | inlined/monomorphised std code |

Every strategy documents its UB-freedom argument at the top of its file (`src/strategies/*.rs`).
Rules: wrapping/checked arithmetic only, masked shifts, non-zero divisors, bounded loops, inputs via
`black_box`, `unsafe` only in patterns argued valid. Adding a strategy = one file + one line in
`strategies/mod.rs`; if you cannot write a one-paragraph validity argument, do not add it.

## Usage

```bash
cargo build --release

# fuzz for 30 minutes with the default configs, nightly toolchain
./target/release/rs-compiler-fuzzer fuzz --rustc "rustc +nightly" --duration 1800

# a single round, selected strategies and configs, smaller batches
./target/release/rs-compiler-fuzzer fuzz --rounds 1 --strategies loops,memory \
    --configs O0,O3,O3-lto-fat --target-lines 5000

# custom configuration
./target/release/rs-compiler-fuzzer fuzz --config "O2-znver3=-C opt-level=2 -C target-cpu=znver3"

# prove the detector works on known miscompilations (canaries/)
./target/release/rs-compiler-fuzzer canary --rustc "rustc +nightly" --configs O0,O3,O3-lto-fat

# print a batch / reproduce a single case
./target/release/rs-compiler-fuzzer gen --strategy arith --seed 1 --target-lines 2000 --out batch.rs
./target/release/rs-compiler-fuzzer gen --strategy arith --case-seed 123 --out one.rs

./target/release/rs-compiler-fuzzer list
```

Exit code: `0` nothing found, `2` findings were written, `1` usage/internal error.
Seeds are printed at the start; the same seed regenerates the same batches.

A finding is a *candidate*: the `*-checks` configs and `PANIC` lines can also expose a bug in the
generator. Before reporting upstream, verify the reproducer is UB-free (Miri) and shrink it by hand.

## CI

- `ci.yml` (push/PR): `cargo test` (determinism + every strategy type-checks) and a short smoke run on nightly.
- `fuzz.yml` (00:00 and 12:00 UTC, or manual dispatch with `duration_minutes`, `scale`, `strategies`,
  `toolchains`, `seed`): one job per *(toolchain, strategy)* - by default nightly and stable x 9 strategies.
  Batches are 25k-150k lines depending on how expensive a strategy is to compile (`--scale` changes that).
  Each job runs the canaries, fuzzes, prints reports into the log, uploads artifacts, and zips every
  **new** finding into the **`Nightly` release** (`<toolchain>__<strategy>__<config>__<hash>.zip`; the hash is of the crash signature for compiler crashes, otherwise of the reproducer, so reruns replace instead of piling up).
  Findings that match a known upstream bug (`// known-bug:` marker in the generator) go to `known/` and
  do not fail the job.
- `verify.yml` (daily, or manual): downloads the zips from the `Nightly` release, re-runs each
  reproducer on the current toolchain (`rs-compiler-fuzzer verify`) and reports which still reproduce
  and which got fixed. With the `prune` input it deletes the fixed ones from the release.

Local equivalent of the verify step: `rs-compiler-fuzzer verify --dir unzipped-findings/ --out verify-out`.

## Reference material

Open `I-miscompile` issues are the model for what to generate:
<https://github.com/rust-lang/rust/issues?q=state%3Aopen%20label%3A%22I-miscompile%22>.
`TODO/` holds the earlier Python prototype (templates, notes on known issues) the Rust implementation
was derived from.
