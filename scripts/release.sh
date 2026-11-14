#!/bin/sh
# Roll [Unreleased] into a version, bump the manifest, commit, tag, and push.
#
# Usage: scripts/release.sh <version> [--dry-run]
#
# Expects a clean working tree on main.

set -eu

version="${1:-}"
dry_run=false
[ "${2:-}" = "--dry-run" ] && dry_run=true

[ -n "$version" ] || {
	echo "usage: $0 <version> [--dry-run]" >&2
	exit 2
}

printf '%s' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || {
	echo "error: version must look like 1.2.3" >&2
	exit 2
}

tag="v$version"

if [ -n "$(git tag -l "$tag")" ]; then
	echo "error: tag $tag already exists" >&2
	exit 1
fi

if [ -n "$(git status --porcelain)" ]; then
	echo "error: working tree is not clean" >&2
	exit 1
fi

notes=$(scripts/changelog-notes.sh Unreleased)
[ -n "$notes" ] || {
	echo "error: [Unreleased] has no entries" >&2
	exit 1
}

date=$(date -u +%Y-%m-%d)
awk -v version="$version" -v date="$date" '
	!done && $0 == "## [Unreleased]" {
		print "## [Unreleased]"
		print ""
		print "## [" version "] - " date
		done = 1
		next
	}
	{ print }
' CHANGELOG.md >CHANGELOG.md.tmp
mv CHANGELOG.md.tmp CHANGELOG.md

sed -i.bak "0,/^version = \"[^\"]*\"/s//version = \"$version\"/" Cargo.toml
rm -f Cargo.toml.bak

# Refresh the lockfile's package version without touching dependencies.
cargo metadata --format-version 1 --offline >/dev/null

git add Cargo.toml Cargo.lock CHANGELOG.md
git commit -m "chore(release): $tag"

if [ "$dry_run" = true ]; then
	echo "dry run: $tag is committed but not tagged or pushed"
	exit 0
fi

git tag -a "$tag" -m "$tag"
git push origin HEAD:main
git push origin "$tag"
