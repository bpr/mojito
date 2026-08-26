#!/bin/bash
# Regenerate conformance/pliron-parity.tsv from the complete fixture inventory.
#
# This is the intentionally expensive parity run. For every eligible fixture,
# it compares the VM oracle with native O0 and O1 executables and runs the
# required sanitizer lane. UPDATE_EXPECT rewrites the manifest before the
# test's coverage ratchets are evaluated, so a stale ratchet may fail after a
# successful regeneration and must then be updated to the reviewed final count.
#
# Do not set MOJITO_PARITY_ONLY here: focused parity runs are for iteration and
# are deliberately forbidden from updating the checked-in manifest.
#
# Treat fixture compilation failures, native exit/category mismatches,
# stdout/stderr divergence, and sanitizer diagnostics as real failures rather
# than accepting the regenerated row mechanically. After reviewing the diff,
# run ./scripts/check-pliron to verify the manifest and the rest of the Pliron
# gate without UPDATE_EXPECT.

cd "$(dirname "$0")"
UPDATE_EXPECT=1 CARGO_WORKSPACE_DIR=$PWD cargo nextest run --features backend-pliron parity_exe_manifest
