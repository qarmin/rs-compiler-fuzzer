#!/bin/bash
# Mutator for release_sync.sh: re-run every stored finding on $TOOLCHAIN, record the result in its status.cfg
# (repro.<toolchain>, last_verified), rebuild INDEX.md. On nightly a finding that does not reproduce twice in a row is
# removed from the package (moved to old_findings/).
set -e
mkdir -p verify-out
"$BIN" verify --rustc "rustc +$TOOLCHAIN" --dir "$PKG/findings" --out verify-out --toolchain "$TOOLCHAIN" 2>&1 | tee verify.log
"$BIN" pack --pkg "$PKG"
