#!/usr/bin/env bash
set -Eeuo pipefail
umask 0022

cd "$(dirname "$0")/.."
REPO_ROOT="$(pwd)"

echo "[SMOKE] Commencing production sanity and state validation..."

echo "[SMOKE] Flushing volatile local cache metrics..."
rm -rf "$HOME/.cache/muthr"
mkdir -p "$HOME/.cache/muthr"

echo "[SMOKE] Evaluating core rust optimization gates..."
cargo clippy --all-targets --all-features -- -D warnings
cargo test

MOCK_WORKSPACE="$(mktemp -d "$HOME/.cache/muthr-smoke.XXXXXX")"
MOCK_PROJECT="${MOCK_WORKSPACE}/smoke-project-alpha"
mkdir -p "$MOCK_PROJECT"

cleanup() {
    local exit_code=$?

    if command -v container >/dev/null 2>&1 && command -v mlxcel-server >/dev/null 2>&1; then
        (
            cd "$MOCK_PROJECT" >/dev/null 2>&1 || true
            cargo run --manifest-path "$REPO_ROOT/Cargo.toml" -- shutdown --yes --verbose >/dev/null 2>&1 || true
        )
    fi

    rm -rf "$MOCK_WORKSPACE"
    exit "$exit_code"
}
trap cleanup EXIT

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

if ! command -v container >/dev/null 2>&1; then
    echo "[SMOKE] Skipping full lifecycle integration (container CLI not available)."
    echo "[ OK ] GA Smoke Matrix evaluation executed successfully. System state is pristine."
    exit 0
fi

if ! command -v mlxcel-server >/dev/null 2>&1; then
    echo "[SMOKE] Skipping full lifecycle integration (mlxcel-server not available)."
    echo "[ OK ] GA Smoke Matrix evaluation executed successfully. System state is pristine."
    exit 0
fi

echo "[SMOKE] Running full lifecycle integration test (run -> sandbox -> shutdown)..."

pushd "$MOCK_PROJECT" >/dev/null

echo "[SMOKE] 1. muthr run"
cargo run --manifest-path "$REPO_ROOT/Cargo.toml" -- run --verbose

echo "[SMOKE] 2. sandbox start --profile base"
cargo run --manifest-path "$REPO_ROOT/Cargo.toml" -- sandbox start --profile base

echo "[SMOKE] 3. sandbox ls"
cargo run --manifest-path "$REPO_ROOT/Cargo.toml" -- sandbox ls

echo "[SMOKE] 4. services status --output json"
cargo run --manifest-path "$REPO_ROOT/Cargo.toml" -- services status --output json >/dev/null

echo "[SMOKE] 5. shutdown"
cargo run --manifest-path "$REPO_ROOT/Cargo.toml" -- shutdown --yes --verbose

popd >/dev/null

echo "[SMOKE] Verifying post-shutdown cleanup..."
SANDBOX_LS_OUTPUT="$(cargo run --manifest-path "$REPO_ROOT/Cargo.toml" -- sandbox ls 2>&1 || true)"
if printf '%s\n' "$SANDBOX_LS_OUTPUT" | grep -q 'muthr-smoke-project-alpha[[:space:]].*running'; then
    echo "[FAIL] Sandbox container still running after shutdown" >&2
    printf '%s\n' "$SANDBOX_LS_OUTPUT" >&2
    exit 1
fi

unset MUTHR_WORKSPACE_ROOT
unset MUTHR_SERVER_PORT

echo "[ OK ] GA Smoke Matrix evaluation executed successfully. System state is pristine."
