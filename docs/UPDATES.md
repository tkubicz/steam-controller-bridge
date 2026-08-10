# Update Center security and release operations

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
