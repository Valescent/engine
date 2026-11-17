#!/bin/sh
# Regenerate docs/progress.md, the computed progress dashboard.
#
# Usage: scripts/progress.sh
#
# Every number is derived from git history, the roadmap, the changelog, and the
# source tree, so the page cannot drift from the repository. Run it after a
# release or any change worth showing.

set -eu

cd "$(dirname "$0")/.."

commits=$(git rev-list --count HEAD)
tests=$(grep -rh --include='*.rs' -c '#\[test\]' src | awk '{ n += $1 } END { print n + 0 }')
loc=$(find src -name '*.rs' -exec cat {} + | wc -l | tr -d ' ')
pages=$(find docs -name '*.md' | wc -l | tr -d ' ')
adrs=$(find docs/decisions -name '0[0-9][0-9][0-9]-*.md' | wc -l | tr -d ' ')

git log --date=short --format=%ad |
	awk -v commits="$commits" \
		-v tests="$tests" \
		-v loc="$loc" \
		-v pages="$pages" \
		-v adrs="$adrs" '
function days_from_civil(y, m, d,   era, yoe, doy, doe) {
	if (m <= 2) y -= 1
	era = int((y >= 0 ? y : y - 399) / 400)
	yoe = y - era * 400
	doy = int((153 * (m + (m > 2 ? -3 : 9)) + 2) / 5) + d - 1
	doe = yoe * 365 + int(yoe / 4) - int(yoe / 100) + doy
	return era * 146097 + doe - 719468
}
function commas(n,   s, out) {
	s = sprintf("%d", n)
	out = ""
	while (length(s) > 3) {
		out = "," substr(s, length(s) - 2) out
		s = substr(s, 1, length(s) - 3)
	}
	return s out
}
function bar(done, total, cls) {
	if (total == 0) return ""
	return sprintf("<div class=\"pg-bar %s\"><span style=\"width: %.0f%%\"></span></div>", \
		cls, 100 * done / total)
}
function rank(level) {
	if (level >= 13) return "Worldsmith"
	if (level >= 10) return "Architect"
	if (level >= 7) return "Cartographer"
	if (level >= 4) return "Stonecutter"
	return "Prospector"
}
FILENAME ~ /roadmap\.md$/ {
	if ($0 ~ /^## v[0-9]/) {
		sec++
		name = $0
		sub(/^## /, "", name)
		secname[sec] = name
	} else if (sec && $0 ~ /^\*\*Status:\*\*/) {
		st = $0
		sub(/^\*\*Status:\*\* */, "", st)
		sub(/ - .*$/, "", st)
		secstatus[sec] = st
	} else if (sec && $0 ~ /^- \[[ x]\]/) {
		sectotal[sec]++
		alltotal++
		if ($0 ~ /^- \[x\]/) {
			secdone[sec]++
			alldone++
		} else {
			item = $0
			sub(/^- \[ \] */, "", item)
			gsub(/`/, "\001", item)
			gsub(/\001[^\001]*\001/, "<code>&</code>", item)
			gsub(/\001/, "", item)
			openitem[sec, ++open_count[sec]] = item
		}
	}
	next
}
FILENAME ~ /CHANGELOG\.md$/ {
	if ($0 ~ /^## \[Unreleased\]/) { unreleased = 1; next }
	if (unreleased && $0 ~ /^## \[/) unreleased = 0
	if ($0 ~ /^## \[/ && $0 !~ /Unreleased/) {
		vname = $0
		sub(/^## \[/, "", vname)
		sub(/\].*$/, "", vname)
		release++
		current = vname
		next
	}
	if ($0 ~ /^### /) {
		group = $0
		sub(/^### /, "", group)
		next
	}
	if ($0 ~ /^- /) {
		entrytotal++
		if (current != "") changecount[current]++
		if (unreleased) {
			unreleased_total++
			if (group != "") unreleased_group[group]++
		}
	}
	next
}
{ active[$0]++ }
END {
	for (d in active) {
		split(d, p, "-")
		day = days_from_civil(p[1] + 0, p[2] + 0, p[3] + 0)
		epoch[d] = day
		if (first == "" || day < minday) { minday = day; first = d }
		if (day > maxday) { maxday = day; last = d; maxcount = active[d] }
		days++
		if (active[d] > busiest) { busiest = active[d]; busiest_day = d }
	}
	span = maxday - minday + 1
	run = 0
	best = 0
	for (i = minday; i <= maxday; i++) {
		hit = 0
		for (d in epoch) if (epoch[d] == i) { hit = 1; break }
		if (hit) {
			run++
			if (run > best) best = run
		} else {
			run = 0
		}
	}
	streak = run
	week = 0
	for (d in epoch) if (epoch[d] >= maxday - 6) week += active[d]

	xp = alldone * 25 + release * 250 + commits * 5 + tests * 2
	level = int(xp / 500) + 1
	into = xp - (level - 1) * 500

	print "# Progress"
	print ""
	print "<p class=\"pg-lede\">Every number on this page is computed from the repository — git"
	print "history, <code>docs/roadmap.md</code>, <code>CHANGELOG.md</code>, and the source"
	print "tree — by <code>scripts/progress.sh</code>. Regenerate it with"
	print "<code>scripts/progress.sh</code>; nothing here is typed by hand.</p>"
	print ""
	print "<div class=\"pg-hero\">"
	print "  <div class=\"pg-panel\">"
	printf "    <span class=\"pg-kicker\">Level %d · %s</span>\n", level, rank(level)
	printf "    <div class=\"pg-xp\"><span class=\"pg-xp-num\">%s</span><span class=\"pg-xp-unit\">XP</span></div>\n", commas(xp)
	printf "    %s\n", bar(into, 500, "pg-bar-lg")
	printf "    <span class=\"pg-mono\">%d XP to level %d</span>\n", 500 - into, level + 1
	print "  </div>"
	print "  <div class=\"pg-tiles\">"
	print "    <div class=\"pg-tile\">"
	print "      <span class=\"pg-mono\">Commits</span>"
	printf "      <span class=\"pg-num\">%s</span>\n", commas(commits)
	print "    </div>"
	print "    <div class=\"pg-tile\">"
	print "      <span class=\"pg-mono\">Tests</span>"
	printf "      <span class=\"pg-num\">%s</span>\n", commas(tests)
	print "    </div>"
	print "    <div class=\"pg-tile\">"
	print "      <span class=\"pg-mono\">Boxes ticked</span>"
	printf "      <span class=\"pg-num\">%d/%d</span>\n", alldone, alltotal
	print "    </div>"
	print "    <div class=\"pg-tile\">"
	print "      <span class=\"pg-mono\">Releases</span>"
	printf "      <span class=\"pg-num\">%d</span>\n", release
	print "    </div>"
	print "    <div class=\"pg-tile\">"
	print "      <span class=\"pg-mono\">Lines in src</span>"
	printf "      <span class=\"pg-num\">%s</span>\n", commas(loc)
	print "    </div>"
	print "    <div class=\"pg-tile\">"
	print "      <span class=\"pg-mono\">Decisions</span>"
	printf "      <span class=\"pg-num\">%d</span>\n", adrs
	print "    </div>"
	print "  </div>"
	print "</div>"
	print ""
	print "## Roadmap"
	print ""
	print "Each milestone counts its own checkboxes. Shipped versions are locked in; the"
	print "in-progress version is the only score that can move right now."
	print ""
	print "<div class=\"pg-rows\">"
	for (i = 1; i <= sec; i++) {
		printf "  <div class=\"pg-row\">\n"
		printf "    <span class=\"pg-row-name\">%s</span>\n", secname[i]
		printf "    <span class=\"pg-row-meta\">%d/%d · %s</span>\n", \
			secdone[i], sectotal[i], secstatus[i]
		printf "    %s\n", bar(secdone[i], sectotal[i], "")
		printf "  </div>\n"
	}
	print "</div>"
	print ""
	printf "**Overall: %d of %d boxes, %.0f%%.**\n", alldone, alltotal, 100 * alldone / alltotal
	print ""
	print "## Momentum"
	print ""
	print "Measured against the last commit, not today, so the page reads the same whenever"
	print "it is regenerated."
	print ""
	print "<div class=\"pg-tiles pg-tiles-wide\">"
	print "  <div class=\"pg-tile\">"
	print "    <span class=\"pg-mono\">Current streak</span>"
	printf "    <span class=\"pg-num\">%d day%s</span>\n", streak, (streak == 1 ? "" : "s")
	print "  </div>"
	print "  <div class=\"pg-tile\">"
	print "    <span class=\"pg-mono\">Longest streak</span>"
	printf "    <span class=\"pg-num\">%d day%s</span>\n", best, (best == 1 ? "" : "s")
	print "  </div>"
	print "  <div class=\"pg-tile\">"
	print "    <span class=\"pg-mono\">Days with commits</span>"
	printf "    <span class=\"pg-num\">%d of %d</span>\n", days, span
	print "  </div>"
	print "  <div class=\"pg-tile\">"
	print "    <span class=\"pg-mono\">Last 7 days</span>"
	printf "    <span class=\"pg-num\">%d commits</span>\n", week
	print "  </div>"
	print "</div>"
	print ""
	print "<div class=\"pg-days\">"
	for (i = 13; i >= 0; i--) {
		target = maxday - i
		hit = 0
		for (d in epoch) if (epoch[d] == target) { hit = 1; break }
		printf "  <span class=\"pg-day%s\"></span>\n", (hit ? " is-on" : "")
	}
	print "</div>"
	printf "<span class=\"pg-mono\">14 days ending %s · busiest day %s with %d commits</span>\n", \
		last, busiest_day, busiest
	print ""
	printf "Since %s: %s commits across %d active days, %s documented changelog entries, %d docs pages.\n", \
		first, commas(commits), days, entrytotal, pages
	if (unreleased_total > 0) {
		printf "In flight right now: %d changes waiting in [Unreleased]", unreleased_total
		sep = " ("
		for (g in unreleased_group) {
			printf "%s%s %d", sep, tolower(g), unreleased_group[g]
			sep = ", "
		}
		print ")."
	}
	print ""
	print "## Achievements"
	print ""
	print "<div class=\"pg-badges\">"
	print "  <div class=\"pg-badge is-unlocked\">"
	print "    <span class=\"pg-badge-mark\">01</span>"
	print "    <strong>First Light</strong>"
	print "    <span class=\"pg-mono\">The first commit landed.</span>"
	print "  </div>"
	printf "  <div class=\"pg-badge %s\">\n", (release >= 1 ? "is-unlocked" : "is-locked")
	print "    <span class=\"pg-badge-mark\">02</span>"
	print "    <strong>Cube on Screen</strong>"
	printf "    <span class=\"pg-mono\">%s</span>\n", (release >= 1 ? "v0.0.1 shipped." : "Ship v0.0.1.")
	print "  </div>"
	printf "  <div class=\"pg-badge %s\">\n", (release >= 3 ? "is-unlocked" : "is-locked")
	print "    <span class=\"pg-badge-mark\">03</span>"
	print "    <strong>Chunk Work</strong>"
	printf "    <span class=\"pg-mono\">%s</span>\n", (release >= 3 ? "v0.0.3 shipped." : "Ship v0.0.3.")
	print "  </div>"
	printf "  <div class=\"pg-badge %s\">\n", (release >= 4 ? "is-unlocked" : "is-locked")
	print "    <span class=\"pg-badge-mark\">04</span>"
	print "    <strong>Playable</strong>"
	printf "    <span class=\"pg-mono\">%s</span>\n", (release >= 4 ? "v0.0.4 shipped." : "Ship v0.0.4.")
	print "  </div>"
	printf "  <div class=\"pg-badge %s\">\n", (release >= 5 ? "is-unlocked" : "is-locked")
	print "    <span class=\"pg-badge-mark\">05</span>"
	print "    <strong>Smooth Operator</strong>"
	printf "    <span class=\"pg-mono\">%s</span>\n", (release >= 5 ? "v0.0.5 shipped." : "Ship v0.0.5.")
	print "  </div>"
	printf "  <div class=\"pg-badge %s\">\n", (release >= 6 ? "is-unlocked" : "is-locked")
	print "    <span class=\"pg-badge-mark\">06</span>"
	print "    <strong>Atmosphere</strong>"
	printf "    <span class=\"pg-mono\">%s</span>\n", (release >= 6 ? "v0.0.6 shipped." : "Ship v0.0.6.")
	print "  </div>"
	printf "  <div class=\"pg-badge %s\">\n", (commits >= 100 ? "is-unlocked" : "is-locked")
	print "    <span class=\"pg-badge-mark\">07</span>"
	print "    <strong>Century Club</strong>"
	printf "    <span class=\"pg-mono\">%s</span>\n", (commits >= 100 ? "100 commits landed." : sprintf("%d of 100 commits.", commits))
	print "  </div>"
	printf "  <div class=\"pg-badge %s\">\n", (tests >= 100 ? "is-unlocked" : "is-locked")
	print "    <span class=\"pg-badge-mark\">08</span>"
	print "    <strong>Test Wall</strong>"
	printf "    <span class=\"pg-mono\">%s</span>\n", (tests >= 100 ? "100 tests pass." : sprintf("%d of 100 tests.", tests))
	print "  </div>"
	printf "  <div class=\"pg-badge %s\">\n", (best >= 7 ? "is-unlocked" : "is-locked")
	print "    <span class=\"pg-badge-mark\">09</span>"
	print "    <strong>Week Streak</strong>"
	printf "    <span class=\"pg-mono\">%s</span>\n", (best >= 7 ? "Seven days in a row." : "Commit seven days in a row.")
	print "  </div>"
	printf "  <div class=\"pg-badge %s\">\n", (adrs >= 10 ? "is-unlocked" : "is-locked")
	print "    <span class=\"pg-badge-mark\">10</span>"
	print "    <strong>Cartographer</strong>"
	printf "    <span class=\"pg-mono\">%s</span>\n", (adrs >= 10 ? "Ten decision records." : sprintf("%d of 10 decision records.", adrs))
	print "  </div>"
	printf "  <div class=\"pg-badge %s\">\n", (100 * alldone >= 50 * alltotal ? "is-unlocked" : "is-locked")
	print "    <span class=\"pg-badge-mark\">11</span>"
	print "    <strong>Halfway</strong>"
	printf "    <span class=\"pg-mono\">%s</span>\n", (100 * alldone >= 50 * alltotal ? "Half the roadmap ticked." : sprintf("%d of %d boxes.", alldone, alltotal))
	print "  </div>"
	printf "  <div class=\"pg-badge %s\">\n", (release >= 5 ? "is-unlocked" : "is-locked")
	print "    <span class=\"pg-badge-mark\">12</span>"
	print "    <strong>Five Releases</strong>"
	printf "    <span class=\"pg-mono\">%s</span>\n", (release >= 5 ? sprintf("%d tags pushed.", release) : sprintf("%d of 5 releases.", release))
	print "  </div>"
	print "</div>"
	print ""
	print "## Next up"
	print ""
	inprogress = 0
	for (i = 1; i <= sec; i++) if (secstatus[i] == "In progress") inprogress = i
	if (inprogress == 0)
		for (i = 1; i <= sec; i++) if (secstatus[i] == "Planned") { inprogress = i; break }
	if (inprogress > 0 && open_count[inprogress] > 0) {
		printf "The open boxes in **%s** — the only score that can move right now:\n\n", secname[inprogress]
		print "<ul class=\"pg-next\">"
		for (j = 1; j <= open_count[inprogress]; j++)
			printf "  <li>%s</li>\n", openitem[inprogress, j]
		print "</ul>"
	} else {
		print "No open boxes in the current milestone. Pick the next one from the [roadmap](roadmap.md)."
	}
	print ""
	print "See the [roadmap](roadmap.md) for the full milestone text."
	print ""
	print "## How the score works"
	print ""
	print "| Source | Weight |"
	print "|---|---:|"
	print "| Roadmap box ticked | 25 XP |"
	print "| Release tagged | 250 XP |"
	print "| Commit | 5 XP |"
	print "| Test in `src/` | 2 XP |"
	print ""
	print "500 XP per level. The weights favour finished, verifiable work over raw churn."
}
' docs/roadmap.md CHANGELOG.md - >docs/progress.md.tmp

mv docs/progress.md.tmp docs/progress.md
echo "wrote docs/progress.md"
