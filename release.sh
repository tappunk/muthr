#!/bin/bash
set -euo pipefail

# Configuration
BIN_NAME="muthr"
TARGET_ARCH="macos-arm64"

DRY_RUN=false
if [[ "${1:-}" == "--dry-run" ]]; then
  DRY_RUN=true
  shift
fi

BUMP="${1:-patch}"
NOTES="${2:-}"

# 1. Validation Checks
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

# Ensure remote is reachable and we are up to date
git fetch origin
if [[ -n $(git log HEAD..origin/main --oneline) ]]; then
  echo "[ERR] Local 'main' is behind 'origin/main'. Pull latest changes first."
  exit 1
fi

for cmd in cargo gh shasum tar; do
  if ! command -v "$cmd" &>/dev/null; then
    echo "[ERR] Required command '$cmd' is not installed."
    exit 1
  fi
done

# 2. Version Parsing
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

echo "Preparing release: v$CURRENT_VERSION -> v$NEW_VERSION ($BUMP)"

if $DRY_RUN; then
  echo ""
  echo "Dry run — nothing will be executed:"
  echo "  Would test and build release binary."
  echo "  Would update Cargo.toml to $NEW_VERSION."
  echo "  Would commit and tag as v$NEW_VERSION."
  echo "  Would package ${BIN_NAME}-${NEW_VERSION}-bin-${TARGET_ARCH}.tar.gz and checksum."
  echo "  Would push tag to GitHub and publish to crates.io."
  exit 0
fi

# 3. Compilation & Safety Gates
echo "[PROC] Running tests..."
cargo test

echo "[PROC] Bumping version in Cargo.toml..."
# Modifies only the first occurrence to avoid hitting dependency versions
sed -i '' "1,/version =/s/version = \"$CURRENT_VERSION\"/version = \"$NEW_VERSION\"/" Cargo.toml

echo "[PROC] Building release binary (and updating Cargo.lock)..."
# This simultaneously checks if the build succeeds and updates Cargo.lock with the new version
cargo build --release

# 4. Git Operations
echo "[PROC] Committing version bump and tagging..."
git add Cargo.toml Cargo.lock
git commit -m "chore: release v$NEW_VERSION"
git tag "v$NEW_VERSION"

# 5. Asset Packaging
echo "[PROC] Packaging assets..."
ARCHIVE_NAME="${BIN_NAME}-${NEW_VERSION}-bin-${TARGET_ARCH}.tar.gz"
CHECKSUM_NAME="${ARCHIVE_NAME}.sha256"
STAGING_DIR="$(mktemp -d)"

mkdir -p "${STAGING_DIR}/${BIN_NAME}"
cp "target/release/${BIN_NAME}" "${STAGING_DIR}/${BIN_NAME}/"
cp README.md LICENSE "${STAGING_DIR}/${BIN_NAME}/" 2>/dev/null || true

tar -czf "$ARCHIVE_NAME" -C "$STAGING_DIR" "${BIN_NAME}"
shasum -a 256 "$ARCHIVE_NAME" >"$CHECKSUM_NAME"
rm -rf "$STAGING_DIR"

# 6. Publishing
echo "[PROC] Pushing to GitHub..."
git push origin main
git push origin "v$NEW_VERSION"

echo "[PROC] Creating GitHub Release..."
if [[ -n "$NOTES" ]]; then
  gh release create "v$NEW_VERSION" "$ARCHIVE_NAME" "$CHECKSUM_NAME" \
    --title "v$NEW_VERSION" \
    --notes "$NOTES"
else
  # Auto-generates release notes from merged PRs if no notes are provided
  gh release create "v$NEW_VERSION" "$ARCHIVE_NAME" "$CHECKSUM_NAME" \
    --title "v$NEW_VERSION" \
    --generate-notes
fi

echo "[PROC] Publishing to crates.io..."
cargo publish

echo "[PROC] Cleaning up local assets..."
rm "$ARCHIVE_NAME" "$CHECKSUM_NAME"

echo "[ OK ] Successfully released v$NEW_VERSION!"
