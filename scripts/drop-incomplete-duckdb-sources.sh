#!/usr/bin/env bash
# Removes unpacked DuckDB source folders that have lost their `.complete` marker.
#
# libduckdb-sys unpacks into <target>/[<triple>/]duckdb-sources/<hash> and marks
# the folder with `.complete`. When a Rust cache is saved, rust-cache's cleanup
# deletes every file below any target folder it does not recognise as a profile,
# so the cached folder has its subfolders and no files. A later build script run
# then finds no marker, unpacks again, and fails to rename onto the non-empty
# folder with DirectoryNotEmpty. The folder holds nothing a build can use, so it
# is safe to remove. See "DirectoryNotEmpty" in docs/agent-notes.md.
#
# Usage: drop-incomplete-duckdb-sources.sh <target dir>
set -euo pipefail

target="${1:?usage: drop-incomplete-duckdb-sources.sh <target dir>}"

for dir in "$target"/duckdb-sources/*/ "$target"/*/duckdb-sources/*/; do
  # A glob that matches nothing stays as its own literal text.
  [ -d "$dir" ] || continue
  if [ -f "${dir}.complete" ]; then
    echo "kept $dir"
  else
    echo "removed $dir (no .complete)"
    rm -rf "$dir"
  fi
done
