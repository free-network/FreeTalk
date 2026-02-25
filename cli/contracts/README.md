# River CLI Contracts

This directory contains the WASM contracts used by the River CLI.

## board_contract.wasm

This is a copy of the board contract from `../ui/public/contracts/board_contract.wasm`.

**Important:** This file MUST be kept in sync with the UI version and committed to the repository for crates.io publishing.

To update:
```bash
cp ../ui/public/contracts/board_contract.wasm contracts/
```

The build.rs script will use this file when building from a crates.io package, and will use the UI version when building from the workspace.