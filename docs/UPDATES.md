# Updater security and release operations

The menu app checks the latest stable GitHub release at most once every 24
hours. It downloads only the manifest and signature envelope during that check.
Artifact URLs are derived from the fixed repository, signed tag, and signed
asset names. `/usr/bin/curl` uses HTTPS-only redirects, fixed connect/overall
timeouts, temporary files, and hard byte limits.

The `release-updater` workspace crate verifies an Ed25519 signature over the raw
manifest before parsing it. The signed manifest binds the application version,
minimum macOS version, release notes, application identity, firmware/app
compatibility, protocol and device-info formats, XIAO board and UF2 family IDs,
post-flash USB identity, and every artifact's exact size and SHA-256. Cached
metadata cannot move backward in either application version or firmware
revision.

## Production setup

Create a protected GitHub Actions environment named `release-signing`, restrict
it to the release workflow, and configure:

- environment secret `UPDATE_SIGNING_PRIVATE_KEY_B64`: the base64 encoding of a
  raw 32-byte Ed25519 private seed;
- optional environment secret `UPDATE_SIGNING_ADDITIONAL_PRIVATE_KEYS`:
  semicolon-separated `key-id=base64-private-seed` entries for transition
  releases.

Configure these non-secret repository or organization Actions variables so the
separate macOS build job can embed and validate the same trust anchor:

- `UPDATE_SIGNING_PRIMARY_PUBLIC_KEY_B64`: the primary raw 32-byte public key
  in base64;
- `UPDATE_SIGNING_KEY_ID`: a stable identifier for that primary key;
- `UPDATE_SIGNING_PUBLIC_KEYS`:
  `key-id=base64`, or semicolon-separated entries during rotation.

The primary private key is used only by the protected `publish` job. Public keys
are injected at compile time into the release app. The job refuses missing or
mismatched primary key material, validates the UF2 structure/family, generates
and signs the manifest, includes the manifest and envelope in
`SHA256SUMS.txt`, and uploads only after the app, firmware, workspace, dependency,
draft-body, and signature gates pass.

The public release contains the UF2 but not the developer serial-DFU zip. The
DFU zip remains an internal developer and CI artifact produced by
`make artifacts`.

## Firmware installation

Firmware revision 2 advertises automatic UF2 entry and installation receipt
capabilities. App Center requests UF2 mode, waits up to 2 seconds for the
correlated readiness response, then waits up to 15 seconds for the mounted
volume. Unsupported or failed automatic entry opens a 60-second manual recovery
window for RST/GND entry. Revision 1 therefore needs one final manual migration.

After copying and syncing the signed UF2, App Center waits up to 30 seconds for
the application device. It requires the exact target revision and a `Pending`
receipt marker. It then supplies the current UTC Unix time, an OS-random 128-bit
installation ID, and the `AppCenter` source. Success requires the correlated
acknowledgement and a second read of the identical committed receipt. A normal
runtime that first sees `Pending` after a developer flash records the same shape
of receipt with source `FirstObserved`.

## Key rotation

First ship an app embedding both the current and next public keys while signing
with the current key. For transition releases, keep both public keys embedded
and set the additional private-key secret so the envelope carries both
signatures. After enough users have installed a dual-key app, make the next key
primary. Retire the old embedded key only in a later release. Unknown key IDs
are ignored, but at least one embedded key must verify the exact manifest.

Source builds intentionally embed no trust anchor unless their builder sets
`SC_BRIDGE_UPDATE_PUBLIC_KEYS`. Fixture keys belong only in tests and pull
request validation; never place the production private seed in the repository,
artifacts, logs, or ordinary repository secrets.

## Local signed development source

Debug builds can replace the fixed GitHub release source with one explicitly
selected local directory. The development path preserves the production trust
boundary: metadata still needs an embedded trusted Ed25519 key, the raw
manifest signature is verified before parsing, and every copied artifact must
match its signed name, size, and SHA-256. Local files cannot escape the selected
directory through path components or symlinks.

Prepare a firmware-only signed catalog from the current workspace:

```sh
tools/prepare-local-update.py
```

The helper builds and validates the current UF2, creates a throwaway local
signing key under the gitignored
`temp/steam-controller-bridge-local-update` directory, generates the signed
manifest, and prints the exact launch command. Quit any already running menu
app before using that command. The application entry is pinned to the workspace
version and is an intentional placeholder, so this catalog can test firmware
installation but cannot stage an application replacement.

The printed command sets both required variables:

- `SC_BRIDGE_UPDATE_PUBLIC_KEYS` embeds the throwaway public key at compile
  time;
- `SC_BRIDGE_LOCAL_UPDATE_DIR` selects the directory at runtime.

Local catalogs refresh on every check and use an isolated cache below the
system temporary directory. They never overwrite the production update cache.
The Updates page displays the active local path. Code compiled without debug
assertions does not include the local source and ignores
`SC_BRIDGE_LOCAL_UPDATE_DIR`; production builds remain locked to the GitHub
release repository.

## Local UI preview

Preview the unified window on its Updates tab with an available update, without
trusted keys, updater network access, release assets, or connected hardware:

```sh
cargo run -p sc-bridge-menu -- app-center --demo --tab updates
```

Preview the fully up-to-date state, including the collapsed current release
notes and firmware reinstall action:

```sh
cargo run -p sc-bridge-menu -- app-center --demo current --tab updates
```

The same fixture can open the other tabs directly:

```sh
cargo run -p sc-bridge-menu -- app-center --demo --tab about
cargo run -p sc-bridge-menu -- app-center --demo current --tab changelog
```

Demo actions only advance the preview state. They never download files, open
hardware, suspend the bridge, or quit the application.
