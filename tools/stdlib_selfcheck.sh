#!/bin/bash
# Compile and run every snippet table of the `stdlib` strategy on the given toolchains (default: stable nightly).
# A table fails when its batch does not compile, a case panics (a panic in a snippet is a generator bug), or the output with
# overflow checks and debug assertions ON differs from the output with them off (a snippet must not depend on them).
#   tools/stdlib_selfcheck.sh [--lines N] [--seed S] [table ...]
set -u
BIN=${BIN:-./target/release/rs-compiler-fuzzer}
LINES=1500; SEED=1; TABLES=(); TOOLCHAINS=${TOOLCHAINS:-"stable nightly"}
while [ $# -gt 0 ]; do
  case $1 in
    --lines) LINES=$2; shift 2;;
    --seed) SEED=$2; shift 2;;
    *) TABLES+=("$1"); shift;;
  esac
done
[ ${#TABLES[@]} -eq 0 ] && read -ra TABLES < <("$BIN" list | sed -n 's/^stdlib tables: //p')
W=$(mktemp -d); fail=0
for t in "${TABLES[@]}"; do
  RSFUZZ_STDLIB_TABLE=$t "$BIN" gen --strategy stdlib --target-lines "$LINES" --seed "$SEED" --out "$W/$t.rs" >/dev/null 2>&1
  for tc in $TOOLCHAINS; do
    if ! rustc +"$tc" --edition 2021 -C debug-assertions=off -C overflow-checks=off --error-format=short "$W/$t.rs" -o "$W/$t.$tc" 2> "$W/$t.$tc.err"; then
      echo "FAIL compile  $t [$tc]"; grep -E "error" "$W/$t.$tc.err" | head -5 | cut -c1-300; fail=1; continue
    fi
    timeout 120 "$W/$t.$tc" > "$W/$t.$tc.out" 2>&1; code=$?
    rustc +"$tc" --edition 2021 -C debug-assertions=on -C overflow-checks=on --error-format=short "$W/$t.rs" -o "$W/$t.$tc.chk" 2> /dev/null
    timeout 120 "$W/$t.$tc.chk" > "$W/$t.$tc.chk.out" 2>&1
    if ! cmp -s "$W/$t.$tc.out" "$W/$t.$tc.chk.out"; then echo "FAIL checks   $t [$tc] output differs with overflow checks / debug assertions on:"; diff "$W/$t.$tc.out" "$W/$t.$tc.chk.out" | head -4; fail=1; fi
    p=$(grep -c PANIC "$W/$t.$tc.out"); n=$(wc -l < "$W/$t.$tc.out")
    if [ $code -ne 0 ] || [ "$p" -ne 0 ] || [ "$n" -lt 5 ]; then echo "FAIL run      $t [$tc] exit=$code panics=$p lines=$n"; fail=1; else echo "ok            $t [$tc] ($n cases)"; fi
  done
done
rm -rf "$W"
exit $fail
