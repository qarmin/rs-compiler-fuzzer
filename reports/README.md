# Findings worth reporting upstream

| file | what | status |
|---|---|---|
| `llvm-eflags-o0-crash.rs` | `rustc-LLVM ERROR: Unable to copy EFLAGS physical register!` at `-C opt-level=0` (i128 `overflowing_*`/`checked_*` arithmetic). Found by the `arith` strategy, minimised automatically from 205 cases to 1 (rustc 1.101.0-nightly 8006091eb, also seen on stable 1.99.1). The file is a full harness: compile with `rustc --edition 2021 -C opt-level=0 file.rs`. Still needs hand-reduction. | not yet reported (no matching issue found when searching rust-lang/rust) |
