#!/bin/bash
set -euo pipefail

# Usage: ./release.sh [--dry-run] [patch|minor|major] [release notes]

DRY_RUN=false
if [[ "${1:-}" == "--dry-run" ]]; then
  DRY_RUN=true
  shift
fi

BUMP="${1:-patch}"
NOTES="${2:-}"

# Validate bump type
if [[ ! "$BUMP" =~ ^(patch|minor|major)$ ]]; then
  echo "Error: invalid bump type '$BUMP'. Use: patch, minor, or major"
  exit 1
fi

# Read current version from Cargo.toml
CURRENT_VERSION=$(grep '^version = ' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')
if [[ -z "$CURRENT_VERSION" ]]; then
  echo "Error: could not read version from Cargo.toml"
  exit 1
fi

# Parse version components
IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT_VERSION"

# Bump version
case "$BUMP" in
  patch) PATCH=$((PATCH + 1)) ;;
  minor) MINOR=$((MINOR + 1)); PATCH=0 ;;
  major) MAJOR=$((MAJOR + 1)); MINOR=0; PATCH=0 ;;
esac

NEW_VERSION="${MAJOR}.${MINOR}.${PATCH}"

# Check for uncommitted changes
if [[ -n $(git status --porcelain) ]]; then
  echo "Error: uncommitted changes detected. Commit or stash before releasing."
  exit 1
fi

echo "Bumping version: $CURRENT_VERSION -> $NEW_VERSION ($BUMP)"

if $DRY_RUN; then
  echo ""
  echo "Dry run — nothing will be changed:"
  echo "  Would update Cargo.toml: $CURRENT_VERSION -> $NEW_VERSION"
  echo "  Would run: git commit -m \"chore: bump version to $NEW_VERSION\""
  echo "  Would run: git push origin main"
  echo "  Would run: cargo publish --allow-dirty"
  echo ""
  echo "Run without --dry-run to execute."
  exit 0
fi

# Update Cargo.toml
sed -i '' "s/version = \"$CURRENT_VERSION\"/version = \"$NEW_VERSION\"/" Cargo.toml

# Commit, push, publish
git add Cargo.toml
git commit -m "chore: bump version to $NEW_VERSION"
git push origin main

echo "Publishing to crates.io..."
cargo publish --allow-dirty

echo "Done! muthr $NEW_VERSION published to crates.io."
