#!/usr/bin/env bash
# Code export browser parity check (SPEC §10 M5, §11).
#
# Exports every layout fixture (crates/layout/tests/fixtures/*.json) and the
# export crate's code fixtures as HTML + CSS, loads each page in headless
# Chromium and compares every element's box with the geometry the layout
# engine computes (0.5px tolerance; 3px for text).
#
# The check is opt-in because a headless browser is a property of the
# machine, not of the project: plain `cargo test` skips it. This script
# runs:
#
#   CODE_PARITY=1 cargo test --offline -p export --test code_parity -- --nocapture
#
# Once opted in it is strict: it fails without Chromium, when Chromium
# produces no output for a page, when no boxes are checked, or on any
# mismatch. Set CHROMIUM=/path/to/chrome to pick the browser, and
# CODE_PARITY_KEEP=1 to keep the generated pages (their folder is printed).
set -euo pipefail
cd "$(dirname "$0")/.."
CODE_PARITY=1 cargo test --offline -p export --test code_parity -- --nocapture
