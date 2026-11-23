#!/usr/bin/env bash
# TLS-117: the probe's slot magic must occur exactly once in every build of the probe, the test
# build included (the owner's run at 9e8bf18f found the test build holding it twice, so every
# stamp there refused). Cross-builds both for x86_64-pc-windows-gnu and counts. Exit 0 only when
# both counts are 1. Run from anywhere; honours CARGO_TARGET_DIR.
set -euo pipefail
cd "$(dirname "$0")/../.."
magic='TLS117-DATA-SLOT'
fail=0
count() {
    local exe=$1 n
    n=$(grep -c -a "$magic" "$exe" || true)
    echo "check-magic: $exe: $n"
    [ "$n" = 1 ] || fail=1
}
exe=$(cargo build --target x86_64-pc-windows-gnu --example tls117_probe --message-format=json \
    | jq -r 'select(.reason == "compiler-artifact" and .target.name == "tls117_probe") | .executable // empty' \
    | tail -n 1)
count "$exe"
test_exe=$(cargo test --no-run --target x86_64-pc-windows-gnu --example tls117_probe --message-format=json \
    | jq -r 'select(.reason == "compiler-artifact" and .target.name == "tls117_probe" and .profile.test) | .executable // empty' \
    | tail -n 1)
count "$test_exe"
exit $fail
