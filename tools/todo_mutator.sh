#!/bin/bash
# Mutator for release_sync.sh (ASSET=todo.zip): remove the NAME.rs / NAME.txt pairs listed in $DONE_FILE
# (one path without extension per line, relative to the root of todo.zip) from $PKG.
set -e
while IFS= read -r stem; do
  [ -z "$stem" ] && continue
  case "$stem" in *..*|/*) echo "todo_mutator: refusing $stem"; continue;; esac
  rm -f "$PKG/$stem.rs" "$PKG/$stem.txt"
done < "$DONE_FILE"
find "$PKG" -mindepth 1 -type d -empty -delete 2>/dev/null || true
