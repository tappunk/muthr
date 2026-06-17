#!/bin/bash
set -euo pipefail

# Configuration
BIN_NAME="muthr"
TARGET_ARCH="macos-arm64"
RUST_TARGET="aarch64-apple-darwin"

DRY_RUN=false
if [[ "${1:-}" == "--dry-run" ]]; then
  DRY_RUN=true
  shift
fi

BUMP="${1:-patch}"
NOTES="${2:-}"

# --- 1. PRE-FLIGHT VALIDATIONS & QUALITY GATES ---
if [[ ! "$BUMP" =~ ^(patch|minor|major)$ ]]; then
  echo "[ERR] Invalid bump type '$BUMP'. Use: patch, minor, or major"
  exit 1
fi

if [[ -n $(git status --porcelain) ]]; then
  echo "[ERR] Uncommitted changes detected. Stash or commit before releasing."
  exit 1
fi

if [[ $(git branch --show-current) != "main" ]]; then
  echo "[ERR] You must be on the 'main' branch to cut a release."
  exit 1
fi

# Ensure local main is synchronized with remote upstream
git fetch origin
if [[ -n $(git log HEAD..origin/main --oneline) ]]; then
  echo "[ERR] Local 'main' is behind 'origin/main'. Pull latest changes first."
  exit 1
fi

# Enforce strict code quality gates locally before making any changes
echo "[PROC] Executing strict code quality gates..."
cargo fmt --check || {
  echo "[ERR] Code formatting violations found. Run 'cargo fmt'."
  exit 1
}
cargo clippy -- -D warnings || {
  echo "[ERR] Clippy warnings detected. Fix them before releasing."
  exit 1
}
cargo test || {
  echo "[ERR] Test suite execution failed."
  exit 1
}

# --- 2. VERSION DETERMINATION ---
CURRENT_VERSION=$(grep -m 1 '^version = ' Cargo.toml | sed 's/version = "\(.*\)"/\1/')
if [[ -z "$CURRENT_VERSION" ]]; then
  echo "[ERR] Could not read current version from Cargo.toml"
  exit 1
fi

IFS='.' read -r MAJOR MINOR PATCH <<<"$CURRENT_VERSION"
case "$BUMP" in
patch) PATCH=$((PATCH + 1)) ;;
minor)
  MINOR=$((MINOR + 1))
  PATCH=0
  ;;
major)
  MAJOR=$((MAJOR + 1))
  MINOR=0
  PATCH=0
  ;;
esac
NEW_VERSION="${MAJOR}.${MINOR}.${PATCH}"

echo "Preparing Apple Silicon Release: v$CURRENT_VERSION -> v$NEW_VERSION ($BUMP)"
if $DRY_RUN; then
  echo "[INFO] Dry run complete. Code is pristine and ready for release."
  exit 0
fi

# --- 3. TRANSACTION MANAGEMENT (ROLLBACK PROTECTION) ---
# If any step fails past this point, revert the workspace to prevent broken states
INITIAL_COMMIT=$(git rev-parse HEAD)
rollback() {
  echo ""
  echo "[CRIT] Release pipeline interrupted! Commencing local rollback..."
  git reset --hard "$INITIAL_COMMIT"
  if git rev-parse "v$NEW_VERSION" >/dev/null 2>&1; then
    git tag -d "v$NEW_VERSION"
  fi
  echo "[ OK ] Rollback successful. Local repository state restored cleanly."
}
trap rollback ERR

# --- 4. ASSET COMPILATION ---
echo "[PROC] Updating versioning configuration..."
# Native macOS perl one-liner ensures robust, boundary-safe string swapping
perl -pi -e "s/version = \"$CURRENT_VERSION\"/version = \"$NEW_VERSION\"/ and \$done=1 if !\$done" Cargo.toml

echo "[PROC] Compiling optimized release binary for Apple Silicon..."
cargo build --release --target "$RUST_TARGET"

# --- 5. PACKAGING ---
echo "[PROC] Packaging distribution archives..."
ARCHIVE_NAME="${BIN_NAME}-${NEW_VERSION}-bin-${TARGET_ARCH}.tar.gz"
CHECKSUM_NAME="${ARCHIVE_NAME}.sha256"
STAGING_DIR="$(mktemp -d)"

# Stage binary alongside core documentation
mkdir -p "${STAGING_DIR}/${BIN_NAME}"
cp "target/${RUST_TARGET}/release/${BIN_NAME}" "${STAGING_DIR}/${BIN_NAME}/"
cp README.md LICENSE "${STAGING_DIR}/${BIN_NAME}/" 2>/dev/null || true

# Compress and generate SHA-256 validation mapping
tar -czf "$ARCHIVE_NAME" -C "$STAGING_DIR" "${BIN_NAME}"
shasum -a 256 "$ARCHIVE_NAME" >"$CHECKSUM_NAME"
rm -rf "$STAGING_DIR"

# --- 6. ATOMIC COMMITS AND TAGGING ---
echo "[PROC] Recording version changes to Git history..."
git add Cargo.toml Cargo.lock
git commit -m "chore: release v$NEW_VERSION [skip ci]"
git tag -a "v$NEW_VERSION" -m "Release v$NEW_VERSION"

# --- 7. DEPLOYMENT (POINT OF NO RETURN) ---
# Disable the local rollback hook now that we are pushing to remote endpoints
trap - ERR

echo "[PROC] Synchronizing changes with remote origin..."
git push origin main
git push origin "v$NEW_VERSION"

echo "[PROC] Deploying GitHub Release and assets..."
if [[ -n "$NOTES" ]]; then
  gh release create "v$NEW_VERSION" "$ARCHIVE_NAME" "$CHECKSUM_NAME" \
    --title "v$NEW_VERSION" \
    --notes "$NOTES"
else
  gh release create "v$NEW_VERSION" "$ARCHIVE_NAME" "$CHECKSUM_NAME" \
    --title "v$NEW_VERSION" \
    --generate-notes
fi

# Local asset cleanup MUST happen before cargo publish
echo "[PROC] Cleaning up local packaging assets..."
rm -f "$ARCHIVE_NAME" "$CHECKSUM_NAME"

echo "[PROC] Publishing crate package to crates.io..."
cargo publish

echo "[ SUCCESS ] Release v$NEW_VERSION fully deployed!"
