# WASM File Safety — CRITICAL

## Never use `git add -A`, `git add .`, or `git add -u` in this repo

These commands will pick up rebuilt WASM binaries from `ui/public/contracts/`.
WASM builds are **non-reproducible** — rebuilding produces different binaries
with different hashes, which changes delegate/contract keys and breaks board
data access.

**Always add files by name:**
```bash
# RIGHT
git add ui/src/components/conversation.rs ui/src/util.rs

# WRONG — will pick up any rebuilt WASMs in the working tree
git add -A
git add .
git add -u
```

## Never commit WASM files without migration

If `git status` shows modified `.wasm` files in `ui/public/contracts/`,
**do not commit them** unless you have added a migration entry to
`ui/src/components/app/chat_delegate.rs` in the `LEGACY_DELEGATES` array.

If you see modified WASMs and did NOT intentionally change them (e.g., a build
rebuilt them as a side effect), restore them:
```bash
git checkout HEAD -- ui/public/contracts/
```

## Delegate Key Continuity

The delegate key is computed from the WASM binary hash. If the WASM changes,
users lose access to their board data stored under the old delegate key.
Adding migration entries allows the UI to read from old delegate keys.
