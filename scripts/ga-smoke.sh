#!/usr/bin/env bash
set -Eeuo pipefail
umask 0022

cd "$(dirname "$0")/.."

echo "[SMOKE] Commencing production sanity and state validation..."

echo "[SMOKE] Flushing volatile local cache metrics..."
rm -rf "$HOME/.cache/muthr"
mkdir -p "$HOME/.cache/muthr"

echo "[SMOKE] Evaluating core rust optimization gates..."
cargo clippy --all-targets --all-features -- -D warnings
cargo test

MOCK_WORKSPACE="$(mktemp -d)"
trap 'rm -rf "$MOCK_WORKSPACE"' EXIT

MOCK_PROJECT="${MOCK_WORKSPACE}/smoke-project-alpha"
mkdir -p "$MOCK_PROJECT"

export MUTHR_WORKSPACE_ROOT="$MOCK_WORKSPACE"
export MUTHR_SERVER_PORT="19091"

echo "[SMOKE] Project workspace context isolated at: ${MOCK_PROJECT}"

echo "[SMOKE] Verifying CLI argument and state dispatch parsing layers..."
cargo run -- sandbox delete --force --yes --dry-run
cargo run -- shutdown --verbose --yes --dry-run

echo "[SMOKE] Validating configuration translation behaviors..."
if ! cargo run -- config show >/dev/null; then
    echo "[FAIL] Internal resolution matrix crashed on absolute evaluations." >&2
    exit 1
fi

echo "[ OK ] GA Smoke Matrix evaluation executed successfully. System state is pristine."
