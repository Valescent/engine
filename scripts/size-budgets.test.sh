#!/bin/sh
# Self-check for scripts/size-budgets.sh.
#
# Builds a throwaway Git work tree with one 600-line file (on the advisory
# boundary, so it must not be reported) and one 1,001-line file (over the
# hard-warning budget). Asserts the annotation, the counts, and the zero exit.

set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
budget="$script_dir/size-budgets.sh"

tmp=$(mktemp -d "${TMPDIR:-/tmp}/size-budgets-test.XXXXXX")
trap 'rm -rf "$tmp"' EXIT HUP INT TERM

git -C "$tmp" -c init.defaultBranch=main init -q

# Exactly 600 lines is not "over 600"; the boundary stays quiet.
awk 'BEGIN { for (i = 0; i < 600; i++) print "// advisory boundary" }' > "$tmp/at-600.rs"
# 1,001 lines is over the 1,000-line budget by one.
awk 'BEGIN { for (i = 0; i < 1001; i++) print "// over budget" }' > "$tmp/over-1000.rs"

git -C "$tmp" -c core.autocrlf=false add at-600.rs over-1000.rs

set +e
output=$(sh "$budget" "$tmp" 2>&1)
status=$?
set -e

fail() {
	printf 'size-budgets.test: %s\n' "$1" >&2
	printf '%s\n' "$output" >&2
	exit 1
}

[ "$status" -eq 0 ] || fail "expected exit 0, got $status"

printf '%s\n' "$output" | grep -F '::warning file=over-1000.rs,line=1::' >/dev/null ||
	fail "missing warning annotation for over-1000.rs"

printf '%s\n' "$output" | grep -F '1 file(s) over 1,000 lines' >/dev/null ||
	fail "missing over-1,000 count"

printf '%s\n' "$output" | grep -F '1 file(s) over 600 lines' >/dev/null ||
	fail "missing over-600 count"

if printf '%s\n' "$output" | grep -F '::warning file=at-600.rs' >/dev/null; then
	fail "exactly 600 lines must not produce a warning annotation"
fi

echo "size-budgets.test: ok"
