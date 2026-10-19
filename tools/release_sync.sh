#!/bin/bash
# Read-modify-write of the single package `findings.zip` in the `Nightly` release, safe against
# concurrent jobs doing the same thing (optimistic concurrency, no locks available on releases):
#
#   1. note which asset (id) is currently `findings.zip`, download and unpack it
#   2. run the mutator (idempotent!) on the unpacked directory
#   3. repack deterministically (sorted, fixed mtimes -> unchanged files give identical bytes)
#   4. upload as a temporary asset, then check `findings.zip` is still the asset we started from
#      - not the same any more: somebody else wrote in the meantime -> drop ours, back off, redo from 1
#      - same: delete the old asset, rename the temporary one to `findings.zip`
#   5. download the result again and check that the keys we wanted are in it (a writer that slipped
#      in between 4's check and the rename would have overwritten us) -> otherwise redo from 1
#
# usage: BIN=path/to/rs-compiler-fuzzer [KEYS_FILE=file] release_sync.sh '<mutator shell snippet, gets $PKG>'
# KEYS_FILE (optional): file the mutator fills with the finding keys it stored; they are verified at the end.
set -u
MUTATOR=${1:?mutator snippet}
REPO=${GITHUB_REPOSITORY:?}
TAG=${RELEASE:-Nightly}
NAME=findings.zip
BIN=${BIN:-./target/release/rs-compiler-fuzzer}
ATTEMPTS=${ATTEMPTS:-8}
WORK=$(mktemp -d); trap 'rm -rf "$WORK"' EXIT

asset_id() { # id of the asset called $1 (empty when missing)
  gh api "repos/$REPO/releases/tags/$TAG" --jq ".assets[] | select(.name==\"$1\") | .id" 2>/dev/null | head -n1
}

fetch() { # fetch <asset id> <dest dir>: unpack findings into dest
  local id=$1 dest=$2
  mkdir -p "$dest"
  [ -z "$id" ] && return 0
  gh api -H "Accept: application/octet-stream" "repos/$REPO/releases/assets/$id" > "$WORK/dl.zip" 2>/dev/null || return 1
  unzip -q -o "$WORK/dl.zip" -d "$dest" || return 1
}

pack_zip() { # pack_zip <dir> <zip>: deterministic archive
  (cd "$1" && find . -type f -print | LC_ALL=C sort | xargs -r touch -d '2000-01-01 00:00:00 UTC' && rm -f "$2" && find . -type f -print | LC_ALL=C sort | zip -q -X -@ "$2")
}

for attempt in $(seq 1 "$ATTEMPTS"); do
  echo "release_sync: attempt $attempt/$ATTEMPTS"
  rm -rf "$WORK/pkg" "$WORK/check"; mkdir -p "$WORK/pkg"
  start_id=$(asset_id "$NAME")
  if ! fetch "$start_id" "$WORK/pkg"; then echo "release_sync: download failed"; sleep $((attempt * 3)); continue; fi
  PKG="$WORK/pkg" bash -c "$MUTATOR" _ || { echo "release_sync: mutator failed"; exit 1; }
  pack_zip "$WORK/pkg" "$WORK/new.zip"
  tmp="$NAME.${GITHUB_RUN_ID:-0}.$$.tmp"
  cp "$WORK/new.zip" "$WORK/$tmp"
  gh release upload "$TAG" "$WORK/$tmp" --repo "$REPO" --clobber >/dev/null || { echo "release_sync: upload failed"; sleep $((attempt * 3)); continue; }
  tmp_id=$(asset_id "$tmp")
  now_id=$(asset_id "$NAME")
  if [ "$now_id" != "$start_id" ]; then
    echo "release_sync: $NAME changed meanwhile ($start_id -> $now_id), retrying"
    [ -n "$tmp_id" ] && gh api -X DELETE "repos/$REPO/releases/assets/$tmp_id" >/dev/null 2>&1
    sleep $(( (RANDOM % 7) + attempt * 2 ))
    continue
  fi
  [ -n "$now_id" ] && gh api -X DELETE "repos/$REPO/releases/assets/$now_id" >/dev/null 2>&1
  gh api -X PATCH "repos/$REPO/releases/assets/$tmp_id" -f name="$NAME" >/dev/null || { echo "release_sync: rename failed"; sleep $((attempt * 3)); continue; }
  # verify: the stored package really contains what we put in
  sleep 2
  final_id=$(asset_id "$NAME")
  if fetch "$final_id" "$WORK/check" && { [ -z "${KEYS_FILE:-}" ] || [ ! -s "${KEYS_FILE:-/dev/null}" ] || "$BIN" pack --pkg "$WORK/check" --contains "$KEYS_FILE"; }; then
    echo "release_sync: stored (asset $final_id, $(du -h "$WORK/new.zip" | cut -f1), $(ls "$WORK/check/findings" 2>/dev/null | wc -l) findings)"
    exit 0
  fi
  echo "release_sync: verification failed (someone overwrote us?), retrying"
  sleep $(( (RANDOM % 7) + attempt * 2 ))
done
echo "release_sync: giving up after $ATTEMPTS attempts"
exit 1
