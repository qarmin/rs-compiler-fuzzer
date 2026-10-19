#!/bin/bash
# Interestingness test for treereduce-rust / creduce-style reducers.
#   env: TC (toolchain, default nightly), FA / FB (rustc flags of configuration A / B),
#        MODE = output (default: both compile and run, stdout or exit code differ)
#             | crash  (A compiles; B's compile fails and its stderr matches $SIG)
#        SIG  (regex, for MODE=crash), CHECKS=1 (default for MODE=output): the program must also run
#        without panicking when built with overflow-checks+debug-assertions at opt-level 0, which
#        filters out reductions that introduced overflow/UB-like behaviour.
#   arg: the candidate file (treereduce passes it via @@)
tc=${TC:-nightly}; mode=${MODE:-output}
d=$(mktemp -d); trap 'rm -rf "$d"' EXIT
f="$d/main.rs"; cp "$1" "$f"   # treereduce hands over files without an .rs extension
common="--edition 2021 --cap-lints allow -C debug-assertions=off -C overflow-checks=off"
# a reduction that does not even type-check is never interesting; also reject new warnings-as-errors-ish noise
timeout 120 rustc +$tc $common $FA "$f" -o "$d/a" 2>/dev/null || exit 1
if [ "$mode" = crash ]; then
  timeout 300 rustc +$tc $common $FB "$f" -o "$d/b" 2> "$d/b.err"; rc=$?
  [ $rc -ne 0 ] && grep -Eq "$SIG" "$d/b.err" && exit 0
  exit 1
fi
timeout 120 rustc +$tc $common $FB "$f" -o "$d/b" 2>/dev/null || exit 1
timeout 10 "$d/a" > "$d/a.out" 2>/dev/null; ra=$?
timeout 10 "$d/b" > "$d/b.out" 2>/dev/null; rb=$?
[ $ra -eq 124 ] || [ $rb -eq 124 ] && exit 1
if [ "${CHECKS:-1}" = 1 ]; then
  timeout 120 rustc +$tc --edition 2021 --cap-lints allow -C opt-level=0 -C debug-assertions=on -C overflow-checks=on "$f" -o "$d/c" 2>/dev/null || exit 1
  timeout 10 "$d/c" >/dev/null 2>&1 || exit 1
fi
if [ $ra -ne $rb ] || ! cmp -s "$d/a.out" "$d/b.out"; then exit 0; fi
exit 1
