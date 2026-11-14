#!/bin/sh
# Print the changelog section belonging to a version, without the heading.
#
# Usage: scripts/changelog-notes.sh <version|Unreleased>

set -eu

target="${1:-}"
[ -n "$target" ] || {
	echo "usage: $0 <version|Unreleased>" >&2
	exit 2
}

awk -v target="$target" '
	/^## \[/ {
		if (found) exit
		name = $0
		sub(/^## \[/, "", name)
		sub(/\].*$/, "", name)
		if (name == target) { found = 1; next }
	}
	found { print }
' CHANGELOG.md
