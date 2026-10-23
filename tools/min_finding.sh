#!/usr/bin/env bash
# Reduce one stored finding with the standalone rs-minimizer (the oracle is the fuzzer's own `interesting` subcommand).
#   tools/min_finding.sh FINDING_DIR [BUDGET_SECS] [extra rs-minimizer options]
# Output: tmp/min/<finding>/min.rs and run.log
set -eu
dir=$1
budget=${2:-300}
shift 2 || shift $#
name=$(basename "$dir")
out=tmp/min/$name
mkdir -p "$out/work"
get() { grep "^$1=" "$dir/repro.cfg" | head -1 | cut -d= -f2-; }
sig=""
[ -s "$dir/signature.txt" ] && sig=$(head -1 "$dir/signature.txt")
cat > "$out/interest.cfg" <<CFG
rustc=rustc +nightly
a_name=$(get base_name)
a_flags=$(get base_flags)
b_name=$(get bad_name)
b_flags=$(get bad_flags)
sig=$sig
compile=120
run=10
work=$PWD/$out/work
CFG
printf '#!/bin/sh\nexec %s/target/release/rs-compiler-fuzzer interesting --cfg %s/%s/interest.cfg --file "$1"\n' "$PWD" "$PWD" "$out" > "$out/test.sh"
chmod +x "$out/test.sh"
start=$(date +%s)
./target/release/rs-compiler-fuzzer reduce "$dir/case.rs" --test "$PWD/$out/test.sh" -o "$out/min.rs" --budget "$budget" --jobs "$(nproc)" --rustc "rustc +nightly" --log "$out/run.log" "$@" > "$out/stdout.log" 2>&1 || echo "FAILED: $name" >> tmp/min/summary.txt
printf '%s: %s -> %s lines, %ss\n' "$name" "$(wc -l < "$dir/case.rs")" "$(wc -l < "$out/min.rs" 2>/dev/null || echo '?')" "$(( $(date +%s) - start ))" >> tmp/min/summary.txt
