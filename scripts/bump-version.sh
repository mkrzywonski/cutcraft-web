#!/usr/bin/env bash
# Raise CutCraft-Web's version: patch (1.0.0 → 1.0.1), minor (→ 1.1.0) or major (→ 2.0.0).
# Updates Cargo.toml and Cargo.lock and adds a heading to CHANGELOG.md for the notes.
set -euo pipefail
cd "$(dirname "$0")/.."

part=${1:-patch}
current=$(sed -n 's/^version = "\([0-9]*\.[0-9]*\.[0-9]*\)"/\1/p' Cargo.toml | head -n 1)
if [[ -z "$current" ]]; then
  echo "Could not read the version from Cargo.toml" >&2
  exit 1
fi
IFS=. read -r major minor patch <<<"$current"
case "$part" in
  patch) patch=$((patch + 1)) ;;
  minor) minor=$((minor + 1)); patch=0 ;;
  major) major=$((major + 1)); minor=0; patch=0 ;;
  *) echo "usage: $0 [patch|minor|major]" >&2; exit 1 ;;
esac
next="$major.$minor.$patch"

awk -v old="version = \"$current\"" -v new="version = \"$next\"" '
  !done && $0 == old { $0 = new; done = 1 }
  { print }
' Cargo.toml >Cargo.toml.new
mv Cargo.toml.new Cargo.toml
cargo update --workspace --offline --quiet

heading="## $next — $(date +%Y-%m-%d)"
awk -v heading="$heading" '
  !done && /^## / { print heading "\n\n- \n"; done = 1 }
  { print }
' CHANGELOG.md >CHANGELOG.md.new
mv CHANGELOG.md.new CHANGELOG.md

echo "$current → $next. Fill in the notes under \"$heading\" in CHANGELOG.md."
