#!/bin/sh
# Builds a downstream repository against a given optfix source tree
# (docs/PLAN-improvements.md, milestone 15).
#
#   tools/downstream.sh varswap   <optfix-dir> <work-dir> <out.md>
#   tools/downstream.sh metalbook <optfix-dir> <work-dir>
#
# Clones the repository at its pin from ci/downstream.env into <work-dir> (which must not
# exist), rewrites its workspace `optfix = "..."` requirement to `optfix = { path = ... }`,
# refreshes only that lockfile entry and fails unless `cargo tree -i optfix` shows the path
# source. varswap: runs `make bench` and copies the regenerated CU table to <out.md>; the bench
# loads the ELF of this checkout (`program/tests/common/mod.rs` resolves it from
# CARGO_MANIFEST_DIR), whose SHA-256 is printed. metalbook: runs the optfix pricer's tests
# (`metalbook-maker`, off chain: no CU).
set -eu
repo=$1
optfix=$(cd "$2" && pwd)/optfix
work=$3
here=$(cd "$(dirname "$0")/.." && pwd)
# shellcheck source=/dev/null
. "$here/ci/downstream.env"
case $repo in
varswap) url=$VARSWAP_REPO sha=$VARSWAP_SHA ;;
metalbook) url=$METALBOOK_REPO sha=$METALBOOK_SHA ;;
*) echo "downstream: unknown repository '$repo'"; exit 2 ;;
esac
[ -f "$optfix/Cargo.toml" ] || { echo "downstream: no optfix crate at $optfix"; exit 2; }
[ ! -e "$work" ] || { echo "downstream: $work already exists"; exit 2; }

git init -q "$work"
git -C "$work" fetch -q --depth 1 "$url" "$sha"
git -C "$work" checkout -q FETCH_HEAD
cd "$work"

# Exactly one `optfix = ...` line, in [workspace.dependencies].
python3 - "$optfix" <<'EOF'
import re, sys
path = sys.argv[1]
text = open("Cargo.toml").read()
pattern = re.compile(r'^optfix = .*$', re.M)
if len(pattern.findall(text)) != 1:
    sys.exit("downstream: expected exactly one `optfix = ...` line in Cargo.toml")
open("Cargo.toml", "w").write(pattern.sub(f'optfix = {{ path = "{path}" }}', text))
EOF
cargo update -q -p optfix
src=$(cargo tree -q -i optfix -e normal --depth 0)
echo "downstream: $repo $sha uses $src"
case $src in
*"($optfix)"*) ;;
*) echo "downstream: $repo does not use the optfix at $optfix"; exit 1 ;;
esac
dirty=$(git -C "$optfix" status --porcelain --untracked-files=no -- . | head -1)
echo "downstream: optfix commit $(git -C "$optfix" rev-parse HEAD)${dirty:+ plus uncommitted changes}"

case $repo in
varswap)
    out=$4
    make bench
    for so in target/deploy/varswap_program.so target/deploy/varswap_program_no_events.so; do
        echo "downstream: $(sha256sum "$so" 2>/dev/null || shasum -a 256 "$so")"
    done
    cp docs/benchmarks.md "$out"
    ;;
metalbook)
    cargo test --locked -p metalbook-maker
    ;;
esac
