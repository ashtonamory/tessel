#!/usr/bin/env bash
# Layout parity check (SPEC §10 M3, §11).
#
# Writes the HTML/CSS twin of every layout fixture
# (crates/layout/tests/fixtures/*.json) to crates/layout/tests/fixtures/html/,
# then runs the fixture tests. Each page lays the fixture out with the
# browser's flexbox and compares every box with the fixture's expected
# geometry (tolerance 0.5px).
#
# Manual browser comparison:
#   1. Run this script.
#   2. Serve the folder (iframes need http, not file://):
#        python3 -m http.server -d crates/layout/tests/fixtures/html 8000
#   3. Open http://localhost:8000/ in Chrome or Firefox. The heading says
#      "PASS: all N fixtures match" or lists the failing fixtures; each
#      fixture's table shows want/got per node. Open a fixture page on its
#      own to inspect its boxes (outlined in blue) with the dev tools.
#   4. A FAIL on a box means taffy and the browser disagree: fix the engine
#      or the fixture, never just the number. Text boxes are marked: their
#      size depends on the browser's font metrics, so small text differences
#      are expected; check positions around them instead.
#
# Automation hook: index.html sets window.parityResults (one entry per
# fixture: { name, failures, total }) on load.
set -euo pipefail
cd "$(dirname "$0")/.."
LAYOUT_PARITY_WRITE=1 cargo test --offline -q -p layout --test fixtures
echo
echo "Wrote $(ls crates/layout/tests/fixtures/html/*.html | wc -l) pages to crates/layout/tests/fixtures/html/."
echo "Serve them with:  python3 -m http.server -d crates/layout/tests/fixtures/html 8000"
echo "then open http://localhost:8000/ and check the summary says PASS."
