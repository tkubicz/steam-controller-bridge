# Updater security and release operations

The menu app checks the latest stable GitHub release at most once every 24
hours. It downloads only the manifest and signature envelope during that check.
Artifact URLs are derived from the fixed repository, signed tag, and signed
asset names. A private `reqwest` client uses Rustls with TLS 1.2 or newer,
system proxy discovery, HTTPS-only redirects, fixed connect/overall timeouts,
cancellable streamed reads, RAII temporary files, and hard byte limits.
**Check Again** bypasses only the 24-hour freshness throttle; signatures,
rollback protection, locking, cache validation, and artifact policy are unchanged.

The `release-updater` workspace crate verifies an Ed25519 signature over the raw
manifest before parsing it. The signed manifest binds the application version,
minimum macOS version, release notes, application identity, firmware/app
compatibility, protocol and device-info formats, firmware target,
target-specific board and UF2 family IDs, post-flash USB identity, and every
artifact's exact size and SHA-256. Cached
metadata cannot move backward in either application version or firmware
revision.

Signed manifest schema v1 deliberately retains one `firmware` entry. The app
resolves that entry through the schema-versioned `firmware-targets.json`
catalog embedded in the application before accepting the metadata or installing
anything. Rust code contains no board-specific updater constants. The catalog currently contains one
target: Seeed Studio XIAO nRF52840/Sense (`seeed-xiao-nrf52840`), with its
minimum compatible revision, application/factory/bootloader USB identities,
accepted board IDs, UF2 family, recovery instructions, and UF2 installer
strategy. A multi-firmware manifest is deferred until another artifact exists.
Catalog parsing is fail-closed: unknown fields, unsupported schemas or
installers, malformed hexadecimal identities, duplicates, empty required
values, and inconsistent primary board IDs disable updater discovery and
installation without affecting normal bridge operation.

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

The current XIAO firmware is revision 3 and reports target
`seeed-xiao-nrf52840`. Revision 2 introduced automatic UF2 entry and
installation receipts but did not report a target. App Center uses automatic
UF2 entry and target-specific update recommendations only when the running
firmware reports the exact catalog target. It waits up to 2 seconds for the
correlated readiness response, then up to 15 seconds for the mounted volume.

Targetless legacy firmware, malformed target identity, and a different valid
target remain usable for bridging and receive no automatic firmware prompt or
bootloader command. A user may still choose the explicitly labeled
**Install or Recover XIAO Firmware** action. For unidentified application
firmware the app releases the serial device and opens a 60-second manual
recovery window asking for two quick presses of the reset button beside the
USB-C connector.

Before writing, the manual and automatic paths both require exactly one
accepted XIAO/Sense board ID and the catalog UF2 family. Multiple devices or a
wrong board fail closed.

After copying and syncing the signed UF2, App Center waits up to 30 seconds for
the application device. It requires the exact returned target ID and revision
plus a `Pending` receipt marker. It then supplies the current UTC Unix time, an
OS-random 128-bit installation ID, and the `AppCenter` source. Success requires the correlated
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

Preview a fresh firmware installation or an application-current/firmware-update
combination directly:

```sh
cargo run -p sc-bridge-menu -- app-center --demo firmware-install
cargo run -p sc-bridge-menu -- app-center --demo firmware-update
```

The demo catalog covers every distinct steady Updates-page branch without
contacting update services or hardware:

- application: `available`, `application-update`, `application-staged`,
  `application-newer`, and `current`;
- firmware: `firmware-install`, `firmware-update`, `firmware-newer`, and
  `firmware-newer-format`;
- target association: `target-unreported`, `target-malformed`, and
  `target-different`;
- installation receipt: `receipt-unavailable`, `receipt-pending`,
  `receipt-invalid`, and `receipt-first-observed`;
- catalog: `catalog-stale`, `catalog-error`, and `catalog-checking`.
- operations: `application-downloading`, `replacement-waiting`,
  `firmware-looking`, `firmware-requesting-bootloader`,
  `firmware-waiting-for-bootloader`, `firmware-manual-recovery`,
  `firmware-writing`, `firmware-reconnecting`, `firmware-recording-receipt`,
  and `firmware-verifying-receipt`.

Running with `--demo` and no value remains an alias for the combined
application-and-firmware update preview. Demo **Check Again**, download,
installation, reveal, and replacement actions mutate only the in-memory
preview.

The same fixture can open the other tabs directly:

```sh
cargo run -p sc-bridge-menu -- app-center --demo --tab about
cargo run -p sc-bridge-menu -- app-center --demo current --tab changelog
```

Demo actions only advance the preview state. They never download files, open
hardware, suspend the bridge, or quit the application.
