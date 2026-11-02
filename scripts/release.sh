#!/usr/bin/env bash
# Cuts a release end to end (D257): bump, prune, gate, commit, push, CI, tag,
# publish. Usage:
#
#   scripts/release.sh 1.9.4 "what this release is"
#
# CHANGELOG.md must already have its `## [1.9.4] - <date>` section. Tracked
# changes are committed with the bump; commit new files first. Set
# RELEASE_TRAILER to append a trailer line (e.g. a Co-Authored-By) to the
# commit message.
#
# The prune is why this exists: a version bump changes the build hash of every
# workspace crate, so the previous version's test binaries can never be used
# again — and cargo never deletes them. Dependencies are left alone, so
# nothing external rebuilds.
set -euo pipefail

die() { echo "release: $*" >&2; exit 1; }
usage='usage: scripts/release.sh <x.y.z> "<title>"'
v=${1:?$usage}
title=${2:?$usage}
cd "$(dirname "$0")/.."

[[ $v =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "not a version: $v"
[ "$(git branch --show-current)" = main ] || die "not on main"
grep -q "^## \[$v\]" CHANGELOG.md || die "CHANGELOG.md has no '## [$v]' section"
git rev-parse -q --verify "refs/tags/v$v" >/dev/null && die "tag v$v already exists"
old=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)
[ "$old" != "$v" ] || die "Cargo.toml is already at $v"

echo "== bump $old -> $v"
sed -i "s/^version = \"$old\"\$/version = \"$v\"/; s/version = \"$old\" }/version = \"$v\" }/" Cargo.toml
sed -i "s/^\*\*Status:\*\* $old,/**Status:** $v,/" README.md
cargo update -w -q

echo "== prune the previous version's workspace artifacts"
pkgs=(-p ren-core -p ren-platform -p ren-cli -p ren-gui)
cargo clean -q "${pkgs[@]}"
cargo clean -q "${pkgs[@]}" --release
cargo clean -q "${pkgs[@]}" --target x86_64-pc-windows-msvc

echo "== quality gate"
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --lib --bins --target x86_64-pc-windows-msvc -- -D warnings
cargo test --workspace
TZ=Asia/Kolkata cargo test --workspace
if command -v cargo-deny >/dev/null; then cargo deny check; else echo "cargo-deny not installed here; CI runs it"; fi
for t in x86_64-unknown-linux-gnu x86_64-pc-windows-msvc; do
  for p in wgpu colored; do
    out=$(cargo tree -e no-dev -i "$p" --target "$t" 2>/dev/null || true)
    [ -z "$out" ] || die "$p reached the shipped tree on $t (D14, D248)"
  done
done
p95() { grep -o 'p95 [0-9.]*ms' | head -1 | cut -d' ' -f2; }
plan=$(cargo run -q --release -p ren-core --example plan_budget | p95)
list=$(cargo run -q --release -p ren-gui --example spike_preview_headless -- 10000 100 | p95)
grid=$(cargo run -q --release -p ren-gui --example spike_preview_headless -- 10000 100 grid | p95)
bench=$(cargo bench -q -p ren-core --bench plan 2>/dev/null |
  awk '/^(plan\/(minimal|realistic|preset|music)|evaluate_all)\/10000 / {printf "%s%s %s %s", sep, $1, $5, $6; sep=", "}')

echo "== commit and push"
git add -u
msg="Release $v: $title

plan_budget p95 ${plan//ms/ ms}; frame p95 ${list//ms/ ms} (list), ${grid//ms/ ms} (grid).
cargo bench plan: $bench."
[ -z "${RELEASE_TRAILER:-}" ] || msg+=$'\n\n'"$RELEASE_TRAILER"
git commit -q -m "$msg"
git push -q origin main
sha=$(git rev-parse HEAD)

# A run for an exact commit, looked up by its SHA: `gh run list` lags the
# push by a few seconds, and the newest run on main may be someone else's.
run_for() {
  local workflow=$1 branch=$2 id=""
  for _ in $(seq 60); do
    id=$(gh run list --workflow "$workflow" --branch "$branch" --limit 10 \
      --json databaseId,headSha --jq ".[] | select(.headSha==\"$sha\") | .databaseId" | head -1)
    [ -n "$id" ] && { echo "$id"; return; }
    sleep 10
  done
  die "no $workflow run for $sha on $branch"
}

echo "== waiting for CI on $sha"
ci=$(run_for ci.yml main)
gh run watch "$ci" --exit-status --interval 60 >/dev/null || die "CI failed: gh run view $ci"

echo "== tag v$v"
# The commit this script made, not whatever HEAD is by now: a commit made
# while CI ran must never get the tag.
git tag -a "v$v" -m "$v" "$sha"
git push -q origin "v$v"
rel=$(run_for release.yml "v$v")
gh run watch "$rel" --exit-status --interval 60 >/dev/null || die "release build failed: gh run view $rel"

echo "== publish"
gh release edit "v$v" --draft=false --latest >/dev/null
gh release view "v$v" --json url --jq .url
