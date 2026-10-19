# Findings worth reporting upstream

| file | what | status |
|---|---|---|
| `llvm-eflags-o0-crash.rs` | `rustc-LLVM ERROR: Unable to copy EFLAGS physical register!` at `-C opt-level=0` (i128 `overflowing_*`/`checked_*` arithmetic). Found by the `arith` strategy, minimised automatically from 205 cases to 1 (rustc 1.101.0-nightly 8006091eb, also seen on stable 1.99.1). The file is a full harness: compile with `rustc --edition 2021 -C opt-level=0 file.rs`. Still needs hand-reduction. | not yet reported (no matching issue found when searching rust-lang/rust) |
| `llvm-select-nan-canonicalisation.rs` | `if x.is_nan() { 0x7ff8_… } else { x.to_bits() }` returns `0xfff8_…` at `-C opt-level=2/3` after inlining (LLVM folds `select(isnan(x), +qnan, x)` to `x`). Found by `floats`, 12-line reproducer, stable 1.97 and nightly 2026-10-17. Arguably NaN payload/sign is unspecified, but the program returns an explicit constant for NaN. | not reported; a quick search of rust-lang/rust and llvm-project issues found no match |
| `rustc-oz-arith-divergence.rs` | i32 expression chain gives a different result at `-C opt-level=z` than at `-O0/s/1/2/3` (stable 1.99.1 and nightly 2026-10-17). Found by `arith`, auto-reduced to 43 lines (`triage`: own passes + treereduce-rust), sensitive to unrelated code (inlining/layout dependent). **Verified UB-free with Miri** (Miri's output equals the `-O0` result). Worth hand-reducing and reporting. | not reported |

`tools/reduce.py in.rs out.rs "<flags A>" "<flags B>" <toolchain>` is the greedy line/`let` reducer used to shrink such reproducers (the fuzzer itself only shrinks at case granularity). Check candidates with `cargo +nightly miri run` before reporting.

Regression range of `rustc-oz-arith-divergence.rs` (from `triage --bisect`): first affected stable release **1.99.0** (1.98.0 is fine); first affected nightly **nightly-2026-08-06** (nightly-2026-08-05 is fine) - `cargo bisect-rustc --start 2026-08-05 --end 2026-08-06` pins down the PR.
