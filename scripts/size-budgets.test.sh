#!/bin/sh
# Self-check for scripts/size-budgets.sh.
#
# Builds two throwaway Git work trees.
#
# 1. An over-budget tree with one 600-line file (on the advisory boundary, so it
#    must stay quiet), one 1,001-line file (over the 1,000-line hard budget), and
#    one 1,001-line file under src/v0/ (frozen legacy, so it is reported as
#    excluded but never warned or counted). Asserts exit 1, the annotation, the
#    counts, and the exclusion.
# 2. An under-budget tree whose largest file is exactly 600 lines. Asserts exit
#    0 and that no hard-budget annotation appears.

set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
budget="$script_dir/size-budgets.sh"

tmp=$(mktemp -d "${TMPDIR:-/tmp}/size-budgets-test.XXXXXX")
trap 'rm -rf "$tmp"' EXIT HUP INT TERM

over="$tmp/over"
clean="$tmp/clean"

fail() {
	printf 'size-budgets.test: %s\n' "$1" >&2
	printf '%s\n' "$output" >&2
	exit 1
}

run() {
	set +e
	output=$(sh "$budget" "$1" 2>&1)
	status=$?
	set -e
}

# --- Over-budget tree: a counted offender and a frozen v0 offender. ---

git -C "$tmp" -c init.defaultBranch=main init -q "$over"

# Exactly 600 lines is not "over 600"; the boundary stays quiet.
awk 'BEGIN { for (i = 0; i < 600; i++) print "// advisory boundary" }' > "$over/at-600.rs"
# 1,001 lines is over the 1,000-line budget by one.
awk 'BEGIN { for (i = 0; i < 1001; i++) print "// over budget" }' > "$over/over-1000.rs"

# src/v0/ is frozen legacy. This file is over budget but must stay excluded.
mkdir -p "$over/src/v0"
awk 'BEGIN { for (i = 0; i < 1001; i++) print "// frozen legacy" }' > "$over/src/v0/world.rs"

git -C "$over" -c core.autocrlf=false add at-600.rs over-1000.rs src/v0/world.rs

run "$over"

[ "$status" -eq 1 ] || fail "expected exit 1 for a counted file over 1,000 lines, got $status"

printf '%s\n' "$output" | grep -F '::warning file=over-1000.rs,line=1::' >/dev/null ||
	fail "missing warning annotation for over-1000.rs"

printf '%s\n' "$output" | grep -F 'hard limit' >/dev/null ||
	fail "annotation must state that the 1,000-line budget is a hard limit"

printf '%s\n' "$output" | grep -F 'failing: 1 file(s) over the 1,000-line hard budget' >/dev/null ||
	fail "missing the hard-budget failure line"

# src/v0/world.rs is over budget but must not be annotated as an offender.
if printf '%s\n' "$output" | grep -F '::warning file=src/v0/world.rs' >/dev/null; then
	fail "src/v0/ files must not produce warning annotations"
fi

printf '%s\n' "$output" | grep -F 'excluding src/v0/ (frozen legacy, deleted when the terrain demo is the default): 1 file(s), 1 over 1,000 and 1 over 600 lines' >/dev/null ||
	fail "missing src/v0 exclusion line with its counts"

printf '%s\n' "$output" | grep -F '3 tracked Rust files (1 under src/v0/ excluded from the budgets)' >/dev/null ||
	fail "missing tracked-file count with the v0 exclusion"

printf '%s\n' "$output" | grep -F '1 file(s) over 1,000 lines' >/dev/null ||
	fail "missing over-1,000 count"

printf '%s\n' "$output" | grep -F '1 file(s) over 600 lines' >/dev/null ||
	fail "missing over-600 count"

if printf '%s\n' "$output" | grep -F '::warning file=at-600.rs' >/dev/null; then
	fail "exactly 600 lines must not produce a warning annotation"
fi

# --- Under-budget tree: the advisory boundary alone must pass. ---

git -C "$tmp" -c init.defaultBranch=main init -q "$clean"

awk 'BEGIN { for (i = 0; i < 600; i++) print "// advisory boundary" }' > "$clean/at-600.rs"
awk 'BEGIN { for (i = 0; i < 250; i++) print "// small module" }' > "$clean/small.rs"

git -C "$clean" -c core.autocrlf=false add at-600.rs small.rs

run "$clean"

[ "$status" -eq 0 ] || fail "expected exit 0 for an under-budget tree, got $status"

if printf '%s\n' "$output" | grep -F '::warning file=' >/dev/null; then
	fail "an under-budget tree must not produce warning annotations"
fi

printf '%s\n' "$output" | grep -F 'no counted file exceeds the 1,000-line hard budget; exiting 0' >/dev/null ||
	fail "missing the under-budget pass line"

printf '%s\n' "$output" | grep -F '0 file(s) over 1,000 lines' >/dev/null ||
	fail "under-budget tree must report zero files over 1,000 lines"

echo "size-budgets.test: ok"
