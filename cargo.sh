#!/bin/bash
# Cargo wrapper that ensures patched dependencies are set up
# Usage: ./cargo.sh build, ./cargo.sh check, etc.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Setup patched dependencies
"$SCRIPT_DIR/setup-deps.sh"

# Run cargo with all arguments
exec cargo "$@"
