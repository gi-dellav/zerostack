# Justfile
# https://github.com/casey/just

[private]
default:
    @just --list

build:
    cargo build --release

build-all:
    cargo build --release --all-features
    # cargo build --release --features "acp,loop,git-worktree,mcp"

run *args:
    cargo run -- {{ args }}

fmt:
    cargo fmt
    cargo clippy --all-targets --all-features -- -D warnings
    # @command -v shear >/dev/null 2>&1 || cargo install shear
    # cargo shear --fix

check:
    cargo fmt --check
    cargo clippy --all-targets --all-features -- -D warnings

install-hook:
    #!/usr/bin/env bash
    cat > .git/hooks/pre-commit << 'EOF'
    #!/bin/sh
    set -e
    echo "Running pre-commit quality checks..."
    just check
    EOF
    chmod +x .git/hooks/pre-commit
    echo "Pre-commit hook installation confirmed."

remove-hook:
    rm .git/hooks/pre-commit
    echo "Pre-commit hook uninstallation confirmed."

add-tag:
    #!/usr/bin/env bash
    set -euo pipefail
    git push origin dev
    VERSION=$(grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)
    git tag -a "v${VERSION}" -m "Release v${VERSION}"
    git push origin "v${VERSION}"
    echo "Created and pushed tag v${VERSION}"

# `just remove-tag v0.0.0` or `just remove-tag (fzf)`
remove-tag VERSION="":
    #!/usr/bin/env bash
    set -e
    tag="{{ VERSION }}"
    if [ -z "$tag" ]; then
        tag=$(git tag | sort -V | fzf --prompt="Select tag to remove: ")
    fi
    if [ -z "$tag" ]; then
        echo "No tag selected"
        exit 1
    fi
    git tag -d "$tag" || {
        echo "Local tag not found"
        exit 1
    }
    git push --delete origin "$tag" # git push origin ":refs/tags/$tag"
    echo "Removed tag $tag"

# Run unit tests
test: fmt
    cargo test
