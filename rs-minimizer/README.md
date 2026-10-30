# rs-minimizer

Test-case reducer for Rust programs. It does what generic reducers (Shrink Ray, treereduce, C-Vise) do badly on generated or machine-renamed code; all passes are built in, no external reducer is used. In a terminal it shows a dashboard (`--ui auto|tui|basic`).

```
rs-compiler-fuzzer reduce INPUT.rs --test ./check.sh -o small.rs
```

`check.sh` gets the path of a candidate as its last argument and exits with 0 while the problem (miscompilation, ICE, a lint, ...) is still there. The result is also the checkpoint: it is rewritten on every improvement, Ctrl+C once stops and keeps it, twice leaves at once.

## Flow

Cheap deletions come first (comments, items together with the code that mentions them, items, lines), repeated while a round removes a tenth of the program, and nothing is expanded or inlined yet. The passes that grow or probe the program (unroll, structure, inlining, peephole) wait until it has fewer than 2000 lines or 30 seconds have passed; the many-oracle-call passes (peephole 800 lines, tokens 4000 tokens) only run on small programs. Every line of the log names the pass that produced it: `[rs-minimizer: lines]`.

## Passes (roughly in the order they run)

1. **Unroll** - `macro_rules!` with one rule are expanded in place (statement, item and `$(...)*` forms, several macros of the same name), `catch_unwind(AssertUnwindSafe(|| X)).is_ok()` becomes `{ X; true }`, `set_hook` goes. Every step is kept only if the oracle still accepts the program, and the longer plain program becomes the baseline.
2. **Modules** - top-level `mod x { .. }` wrappers are hoisted into the crate root with their `use` and path prefixes. When names collide (a renamed program has `mod a`, `trait a` and `fn a` side by side) the colliding items are renamed first, with `rustc` as the name resolver: the declaration is renamed and every use the compiler reports as unresolved is fixed (`rename.rs`).
3. **Items** - whole functions, impls, structs, `use`, ... and attributes as deletion units; comments and blank lines.
4. **Lines** - ddmin with several candidates tested at once.
5. **Dead code** - one `rustc` run lists unused items and imports, they go together with the `impl` blocks that mention them.
6. **Brackets** - hollowing out a bracket pair, removing the brackets, lifting the contents of a block, deleting lexemes (the ideas of Shrink Ray, rewritten here and checked by the oracle after every step).
7. **Traits** - taken apart piece by piece: `Trait::f(..)` pointed at the only impl (`Type::f(..)`), generic parameters replaced by the type their trait is implemented for (`fn e<T: Num>` -> `fn e` with `i128`; this is what lets the methods be inlined), a single trait method deleted together with its copies in the impls, a single bound deleted (`T: A + B + C` -> `T: A + C`), a whole trait with all its impls. Growing the program is fine here: simpler code lets the other passes shrink it.
8. **Inlining** - calls of functions and methods are replaced by their bodies (`f(a)` -> `{ let x = a; BODY }`, `r.m(a)` -> `{ let (__s, x) = (&r, a); BODY }`), so the function, its impl, its trait and the generic parameter that only existed for it can be deleted. Duplicate function names are made distinct first; bodies with `return`/`?` are never pasted.
9. **Structure** - a newtype struct replaced by its field (`W(x)` -> `(x)`, `W` -> `T`, `x.0` -> `x` or `*x` depending on how `x` was declared), function bodies stubbed with `unimplemented!()`, unused parameters removed together with the matching argument of every call (also of `extern`/`async` functions and ones with `return`, `?` or recursion: nothing is pasted there), unused struct fields removed together with their initialisers, enum variants removed together with the match arms that name them.
10. **Peephole** (C-Vise `peep`/`balanced`, Shrink Ray `replace_identifiers_with_zero`) - every call, identifier, parenthesised group and block tried as `0`, one operand kept of every binary expression, `let mut` -> `let`, `&mut` -> `&`, `+=` -> `=`. Also: `extern "win64" fn` -> `extern "C" fn` -> `fn`, an expression with a comparison or `&&`/`||` tried as `true`/`false`, two literals compared (`0 == 0`) folded, a long unsigned literal tried as `uN::MAX`.
11. **Tokens** (C-Vise `clex`) - windows of 1..8 consecutive tokens deleted.
12. **Expressions** - literals to `0`/`1`, calls replaced by an argument, `.method()` segments dropped, `if` replaced by a branch, redundant parentheses; then `rustfmt` if the program stays interesting.

A result is handed back in place of the input only when it has fewer tokens than the input (layout and line count do not decide, inlining may add lines).

Callers that know how their programs were produced plug in through `Hook` (see `rs-compiler-fuzzer`'s `src/reduce.rs`: batch harness handling).

## Tests

`cargo test -p rs-minimizer`. The tests never depend on a compiler bug: the oracle is "compiles and prints exactly X", so they stay valid after the bugs are fixed. They need `rustc` on PATH.
