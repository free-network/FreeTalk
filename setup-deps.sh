#!/bin/bash
# Setup patched dependencies for freetalk
# Run this before cargo build/check

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
STDLIB_DIR="/tmp/freenet-stdlib"
PATCH_FILE="$SCRIPT_DIR/new-patches/freenet-stdlib-fix-filename.patch"
MARKER="$STDLIB_DIR/.freetalk-patched"

if [ -f "$MARKER" ]; then
    exit 0
fi

echo "Setting up patched freenet-stdlib..."

if [ ! -d "$STDLIB_DIR" ]; then
    echo "Cloning freenet-stdlib..."
    git clone --depth 1 https://github.com/freenet/freenet-stdlib.git "$STDLIB_DIR"
fi

if [ -f "$PATCH_FILE" ]; then
    echo "Applying patch..."
    cd "$STDLIB_DIR"
    if ! git apply "$PATCH_FILE" 2>/dev/null; then
        if git apply --reverse --check "$PATCH_FILE" 2>/dev/null; then
            echo "Patch already applied"
        else
            echo "Failed to apply patch"
            exit 1
        fi
    fi
fi

touch "$MARKER"
echo "Done!"
