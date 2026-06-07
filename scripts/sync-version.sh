#!/usr/bin/env bash
# Sync the version from Cargo.toml to all packaging files.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"

VERSION=$(grep '^version' "${ROOT_DIR}/Cargo.toml" | head -1 | cut -d'"' -f2)

if [ -z "$VERSION" ]; then
    echo "Error: Could not read version from Cargo.toml" >&2
    exit 1
fi

echo "Syncing version ${VERSION} across packaging files..."

# flake.nix reads version from Cargo.toml via importTOML (no change needed)

# PKGBUILD
sed -i "s/^pkgver=.*/pkgver=${VERSION}/" "${ROOT_DIR}/packaging/aur/PKGBUILD"

# conda meta.yaml files
for meta in "${ROOT_DIR}/packaging/conda/"*/meta.yaml; do
    sed -i "s/{% set version = \".*\" %}/{% set version = \"${VERSION}\" %}/" "$meta"
done

echo ""
echo "Next steps:"
echo "  just pre-release     # sync version + source SHA256 (safe before release)"
echo "  just add-tag          # push tag, trigger GitHub release"
echo "  just post-release     # download binaries, update checksums, regenerate .SRCINFO"
