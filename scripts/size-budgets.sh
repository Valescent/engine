#!/bin/sh
# Report Rust file size budgets. Findings are warnings, never failures.
#
# Design: docs/design/modular-structure.md §5.6 and §8. Files over 1,000 lines
# are reported individually as GitHub warning annotations. Files over 600 lines
# are the advisory tier and are summarised as a count. The script exits 0 even
# when it finds offenders; the CI step becomes a hard gate only once the tree is
# under budget.
#
# Usage: scripts/size-budgets.sh [repo-root]
#
# The optional root defaults to the current directory. It only needs to be a
# Git work tree; the script reads tracked paths and never modifies the tree.

set -eu

LC_ALL=C
export LC_ALL

root=${1:-.}

if ! top=$(git -C "$root" rev-parse --show-toplevel 2>/dev/null); then
	echo "size-budgets: $root is not a Git work tree" >&2
	exit 2
fi
root=$top

work=$(mktemp -d "${TMPDIR:-/tmp}/size-budgets.XXXXXX")
trap 'rm -rf "$work"' EXIT HUP INT TERM

list="$work/files"
records="$work/records"

# Tracked Rust files only. Build output, the rendered book, and world saves are
# not source and never count against the budget.
git -C "$root" ls-files '*.rs' | grep -Ev '(^|/)(target|book|world)/' > "$list" || true

: > "$records"
while IFS= read -r file; do
	[ -n "$file" ] || continue
	# awk counts a final unterminated line; wc -l does not.
	lines=$(awk 'END { print NR }' "$root/$file")
	printf '%s\t%s\n' "$lines" "$file" >> "$records"
done < "$list"

total=$(wc -l < "$list" | tr -d ' ')
over_1000=$(awk -F '\t' '$1 > 1000' "$records" | wc -l | tr -d ' ')
over_600=$(awk -F '\t' '$1 > 600' "$records" | wc -l | tr -d ' ')
advisory=$((over_600 - over_1000))

echo "Size budgets: $total tracked Rust files; $over_1000 over 1,000 lines; $over_600 over 600 lines"

# Every file over the hard-warning budget gets a line annotation. The annotation
# is a warning so it appears in the CI summary without failing the run.
awk -F '\t' '$1 > 1000 {
	printf "::warning file=%s,line=1::%s has %s lines; the 1,000-line file budget is a warning until the tree is under budget\n", $2, $2, $1
}' "$records"

echo "size-budgets: $over_1000 file(s) over 1,000 lines"
if [ "$advisory" -gt 0 ]; then
	echo "size-budgets: $advisory more file(s) between 601 and 1,000 lines; $over_600 file(s) over 600 lines total"
else
	echo "size-budgets: $over_600 file(s) over 600 lines total"
fi

echo "size-budgets: largest files:"
sort -k1,1nr -k2,2 "$records" | head -n 5 | awk -F '\t' '{ printf "  %5s %s\n", $1, $2 }'

echo "size-budgets: warnings only; exiting 0 (the gate tightens once the tree is under budget)"
exit 0
