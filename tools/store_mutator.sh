#!/bin/bash
# Mutator for release_sync.sh: merge the finding directories in $ADD_DIRS (space separated) into $PKG
# and write the keys that were stored to $KEYS_FILE.
set -e
args=()
for d in $ADD_DIRS; do [ -d "$d" ] && args+=(--add "$d"); done
"$BIN" pack --pkg "$PKG" --toolchain "${TOOLCHAIN:-unknown}" "${args[@]}" | tee "${KEYS_FILE}.out"
sed -n 's/^PACKED //p' "${KEYS_FILE}.out" > "$KEYS_FILE"
