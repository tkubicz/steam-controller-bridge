# Rust `IOHIDUserDevice` Virtual Gamepad Implementation Plan

Status: implemented through the automated and packaging gates. A disposable
lowered-security VM proves creation, raw reports, GameController registration,
and browser visibility for the fixed Xbox-style contract at `045e:028e`.
Earlier cumulative experiments showed that `cafe:4001` remains invisible with
the same descriptor, so selectable profiles have been removed. The full
Linux-workspace cross-Clippy command is environment-blocked by the missing
`x86_64-linux-gnu-gcc`; the new virtual-HID crate passes that target directly.

Last updated: 2026-08-13.

This document is the implementation source of truth for adding an experimental
software gamepad output to Steam Controller Bridge. Follow the phases in order.
Do not substitute CoreHID, DriverKit, a system extension, or Swift without a new
architecture decision.

## 1. Goal

Add an opt-in virtual gamepad output that does not require the external XIAO USB
bridge. The implementation must:

- stay entirely in Rust;
- create the virtual device with the public IOKit `IOHIDUserDevice` API;
- isolate the restricted entitlement in a nested Rust helper executable;
- preserve the existing XIAO serial backend as the default and proven path;
- support safe runtime switching between XIAO and virtual HID;
- neutralize and release every output before stop, quit, sleep, update, or
  backend replacement is acknowledged;
- compile and test without the restricted entitlement;
- provide a documented disposable-VM path for the first live experiment; and
- make no XIAO firmware changes.

The expected data flow is:

```text
Steam Controller -> BridgeEngine -> GamepadOutput
                                    |-> SerialOutput -> XIAO -> USB gamepad
                                    `-> VirtualHidOutput -> versioned stdio
                                                           -> Rust helper
                                                           -> IOHIDUserDevice
                                                           -> macOS HID clients
```

## 2. Binding architectural decisions

These are decisions, not suggestions:

1. The first provider is Rust `IOHIDUserDevice`, not CoreHID.
2. No Swift source, Swift package, Swift compiler step, or Swift ABI binding is
   added.
3. The product and Rust types must say `VirtualHid`, not `CoreHid`. Do not claim
   that this implementation uses CoreHID.
4. The existing minimum remains macOS 13. `IOHIDUserDevice` is available before
   that; this feature does not justify raising the product minimum to macOS 15.
5. The main menu app remains unentitled. Only the nested helper carries
   `com.apple.developer.hid.virtual.device`.
6. XIAO remains the default for all new and migrated installations.
7. Backend choice is explicit. Never silently fall back from virtual HID to
   XIAO or from XIAO to virtual HID.
8. The first virtual milestone is input-only. Raw host get/set-report activity
   is diagnosed, but it is not translated into rumble.
9. The virtual backend has one fixed USB/Xbox-style descriptor and 20-byte
   report contract. Its default identity is `045e:028e`, the only tested
   combination macOS publishes through GameController and the same workaround
   used by XIAO. Development callers may override VID and PID only as a pair;
   no descriptor/profile selector is retained.
10. Live entitlement proof is a manual gate. Compilation, dry-run integration,
    packaging, and VM validation must be reported separately.

Why not CoreHID: the current CoreHID public surface exposes
`HIDVirtualDevice` as a Swift actor with Swift-concurrency methods. There is no
public C or Objective-C header that Rust can bind normally. Calling mangled
Swift symbols directly would be an unsupported ABI dependency. The lower-level
`IOHIDUserDevice` C API is public, creates a virtual HID device, and explicitly
requires the same restricted entitlement.

Relevant Apple sources:

- [`HIDVirtualDevice`](https://developer.apple.com/documentation/corehid/hidvirtualdevice)
- [`IOHIDUserDeviceCreateWithProperties`](https://developer.apple.com/documentation/iokit/3334952-iohiduserdevicecreatewithpropert)
- [Apple engineer guidance on virtual HID entitlements](https://developer.apple.com/forums/thread/820708)
- [Apple inside-out code-signing guidance](https://developer.apple.com/documentation/xcode/creating-distribution-signed-code-for-the-mac/)

Before production promotion, ask Apple whether `IOHIDUserDevice` remains an
accepted public entry point for the managed entitlement. If Apple explicitly
requires CoreHID, keep the IPC contract and replace only the helper provider in
a separately approved change.

## 3. Non-goals

Do not add any of the following in this work:

- CoreHID or Swift;
- DriverKit, HIDDriverKit, a kernel extension, or a system extension;
- legacy private APIs or direct calls to Swift ABI symbols;
- `CGEvent` keyboard/mouse emulation as a virtual-gamepad fallback;
- XIAO firmware or serial-protocol changes;
- virtual rumble mapping before a real client contract is captured;
- alternate selectable descriptor, transport, or report profiles;
- a custom virtual-machine application;
- automatic modification of SIP, AMFI, NVRAM, or host security settings;
- automatic selection of a writable helper from `PATH`; or
- production-readiness claims based only on dry-run or lowered-security proof.

## 4. Repository changes at a glance

Create one new package containing a library and the helper binary:

```text
crates/macos-virtual-hid/
  Cargo.toml
  src/
    lib.rs                 # public config, errors, output type, diagnostics
    contract.rs            # descriptor, report encoder, IPC types and limits
    client.rs              # helper process and bounded worker
    platform.rs            # cfg-selected platform facade
    platform/
      macos.rs             # IOHIDUserDevice owner; narrow unsafe boundary
      unsupported.rs       # non-macOS live-provider error
    bin/
      sc-virtual-hid-helper.rs
  tests/
    dry_run_ipc.rs
    fixtures/
      create.jsonl
      ready.jsonl
      input_report.jsonl
      applied.jsonl
      shutdown.jsonl
```

Add these packaging files:

```text
packaging/macos/VirtualHidHelper.Info.plist
packaging/macos/VirtualHidHelper.entitlements
```

Add or update these documents and test assets during implementation:

```text
docs/VIRTUAL_HID.md
docs/VIRTUAL_HID_IPC.md
docs/VIRTUAL_HID_FEASIBILITY.md
tools/gamepad-api-tester.html
```

The implementation will also touch:

- `Cargo.toml`
- `Cargo.lock`
- `crates/bridge-output/src/lib.rs`
- `crates/bridge-runtime/src/api.rs`
- `crates/bridge-runtime/src/runtime.rs`
- `crates/bridge-runtime/src/supervisor/mod.rs`
- `crates/bridge-runtime/src/supervisor/commands.rs`
- `crates/bridge-runtime/src/supervisor/discovery.rs`
- `crates/bridge-runtime/src/supervisor/controller/session.rs`
- `crates/bridge-runtime/src/supervisor/active_session.rs`
- bridge-runtime tests under `crates/bridge-runtime/src/tests/`
- `apps/sc-bridge/src/cli.rs`
- `apps/sc-bridge/src/main.rs`
- `apps/sc-replay/src/main.rs`
- `apps/gamepad-simulator/src/main.rs`
- `apps/sc-bridge-menu/src/macos.rs`
- `apps/sc-bridge-menu/src/macos/support.rs`
- `apps/sc-bridge-menu/src/macos/tray.rs`
- `apps/sc-bridge-menu/src/macos/tests.rs`
- `apps/sc-bridge-menu/src/model.rs`
- `tools/build-macos-app.py`
- `packaging/macos/Info.plist` only if comments need clarification; keep `13.0`
- `README.md`
- `docs/ARCHITECTURE.md`
- `docs/TESTING.md`
- `docs/TECHNICAL_GUIDE.md`
- CI/release workflows if new gates are not already reached by workspace tests

Do not start by editing the menu. Implement and prove the report contract and
dry-run helper first.

## 5. New package design

### 5.1 Cargo configuration

Add `crates/macos-virtual-hid` to the workspace. The package must be
`publish = false` and use the workspace version, edition, and license.

Platform-neutral dependencies:

- `bridge-output`
- `gamepad-state`
- `serde`
- `serde_json`
- `thiserror`

macOS-only dependencies:

- `objc2-core-foundation` with `CFData`, `CFDictionary`, `CFNumber`, and
  `CFString` features, plus `CFBundle` if bundle metadata is read through Core
  Foundation;
- `objc2-io-kit` with `block2`, `dispatch2`, `hid`, `hidsystem`, `libc`, and
  `std` features;
- `block2`;
- `dispatch2`; and
- `libc`, for `mach_absolute_time()`.

Add `block2` and `dispatch2` to `[workspace.dependencies]` using versions
compatible with the already locked `objc2` family. Keep all Apple dependencies
under `target.'cfg(target_os = "macos")'.dependencies` so Linux workspace
builds do not link Apple frameworks.

This package is a narrow unsafe FFI boundary, like `macos-power-monitor`. Do not
inherit the workspace's `unsafe_code = "forbid"` unchanged. Instead use:

```toml
[lints.rust]
unsafe_op_in_unsafe_fn = "deny"

[lints.clippy]
all = "warn"
pedantic = "warn"
undocumented_unsafe_blocks = "deny"
```

Every unsafe block must have a concrete ownership, lifetime, pointer-validity,
or callback-threading safety comment. No raw IOKit type may escape the platform
module.

### 5.2 Public library interface

Export these names from `macos_virtual_hid`:

```rust
pub const HELPER_PROTOCOL_VERSION: u16 = 3;
pub const DEFAULT_VENDOR_ID: u16 = 0x045e;
pub const DEFAULT_PRODUCT_ID: u16 = 0x028e;
pub const INPUT_REPORT_LEN: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualHidConfig {
    pub helper_path: PathBuf,
    pub queue_capacity: usize,
    pub startup_timeout: Duration,
    pub acknowledgement_timeout: Duration,
    pub shutdown_timeout: Duration,
    pub dry_run: bool,
    pub vendor_id: u16,
    pub product_id: u16,
}

impl VirtualHidConfig {
    pub fn new(helper_path: PathBuf) -> Self;
    pub fn dry_run(helper_path: PathBuf) -> Self;
    pub fn with_identity(self, vendor_id: u16, product_id: u16) -> Self;
}

pub struct VirtualHidOutput { /* private fields */ }

impl VirtualHidOutput {
    pub fn open(config: VirtualHidConfig) -> Result<Self, VirtualHidError>;
    pub fn helper_metadata(&self) -> VirtualHidHelperMetadata;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualHidHelperMetadata {
    pub protocol_version: u16,
    pub vendor_id: u16,
    pub product_id: u16,
    pub bundle_identifier: Option<String>,
    pub signing_identifier: Option<String>,
    pub entitlement_present: Option<bool>,
    pub dry_run: bool,
}
```

Recommended lifecycle defaults:

- queue capacity: 32 reports;
- startup timeout: 5 seconds;
- acknowledgement timeout: 1 second;
- shutdown timeout: 2 seconds.

Reject zero capacities and zero durations before spawning a child. Identity
overrides are only an escape hatch; they never select a different descriptor,
transport, encoder, or validator.

### 5.3 Error model

`VirtualHidError` must preserve a machine-readable class and a safe display
message. At minimum, distinguish:

```rust
pub enum VirtualHidErrorClass {
    UnsupportedPlatform,
    MissingHelper,
    InvalidConfiguration,
    SpawnFailed,
    StartupTimeout,
    HelperExited,
    ProtocolMismatch,
    ProtocolViolation,
    EntitlementMissing,
    EntitlementRejected,
    DeviceCreationFailed,
    DispatchFailed,
    AcknowledgementTimeout,
    QueueOverflow,
    CancellationTimeout,
}
```

Add `VirtualHidError::is_permanent_configuration_failure()`. Return `true` for
unsupported platform, missing helper, invalid configuration, protocol mismatch,
protocol violation, missing/rejected entitlement, and deterministic device
creation rejection. Child I/O loss and an isolated helper crash are transient.

Do not include a full helper path in normal status or copied diagnostics. It is
acceptable for an explicit CLI error to include the path supplied by the CLI.

## 6. HID descriptor and input report

Define the descriptor once in `contract.rs`. Both the parent-side encoder and
the helper tests must use that constant. Do not duplicate descriptor bytes in
the helper binary.

Use the 212-byte Xbox-style HID descriptor pinned by
`contract::GAMEPAD_REPORT_DESCRIPTOR`. It is one indivisible contract with USB
transport and the following 20-byte input report:

| Byte(s) | Meaning | Encoding |
| --- | --- | --- |
| 0 | packet type | always `0` |
| 1 | packet size | always `20` (`0x14`) |
| 2-3 | D-pad and buttons | little-endian XInput mask |
| 4 | left trigger | unsigned 8-bit |
| 5 | right trigger | unsigned 8-bit |
| 6-7 | left X | signed 16-bit little-endian |
| 8-9 | left Y | signed 16-bit little-endian |
| 10-11 | right X | signed 16-bit little-endian |
| 12-13 | right Y | signed 16-bit little-endian |
| 14-19 | reserved | always zero |

Encoding rules:

- Call `GamepadState::validate()` first. Never sanitize silently.
- Convert axes with `(value * 32767.0).round() as i16`. Therefore `-1.0`
  maps to `-32767`, neutral to `0`, and `1.0` to `32767`.
- Convert triggers with `(value * 255.0).round() as u8`.
- Encode all 16-bit fields little-endian.
- Map hat directions to the XInput D-pad bits and the supported bridge buttons
  to their pinned XInput masks.
- Reject button bits above bit 15 if a future caller can construct them.
- `GamepadState::neutral()` must encode exactly as:

```text
00 14 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
```

Tests must pin all 212 descriptor bytes and every report offset. The device
defaults to `045e:028e`. `VirtualHidConfig::with_identity` may change only those
two numbers, and all CLIs must require the vendor and product overrides as a
pair. Do not reintroduce selectable profiles or expose identity tuning in the
menu. See `docs/VIRTUAL_HID_IPC.md`.

## 7. Helper protocol

### 7.1 Transport

- Parent writes one compact JSON object plus `\n` to inherited stdin.
- Helper writes one compact JSON object plus `\n` to stdout.
- Helper stderr is for human-readable logs only and is inherited by the parent.
- UTF-8 is required.
- Maximum line length is 65,536 bytes including the newline.
- Maximum raw report payload is 4,096 bytes.
- Use `#[serde(deny_unknown_fields)]` and tagged enums.
- Every message contains `protocol: 3`.
- Malformed JSON, unknown fields, an oversized line, an oversized report, a
  wrong protocol, or an invalid sequence is fatal.

Share the request and response types in `contract.rs`; the two processes must
not maintain separate JSON definitions.

### 7.2 Parent-to-helper messages

Create must be first:

```json
{"type":"create","protocol":3,"vendor_id":1118,"product_id":654}
```

Input reports use a monotonically increasing `u64` sequence:

```json
{"type":"input_report","protocol":3,"sequence":1,"report":[0,20,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]}
```

Shutdown also has a sequence:

```json
{"type":"shutdown","protocol":3,"sequence":2}
```

### 7.3 Helper-to-parent messages

Ready is emitted only after activation and successful initial-neutral dispatch:

```json
{"type":"ready","protocol":3,"vendor_id":1118,"product_id":654,"dry_run":true,"bundle_identifier":null,"signing_identifier":null,"entitlement_present":null}
```

Every accepted input or shutdown sequence receives exactly one applied reply:

```json
{"type":"applied","protocol":3,"sequence":1}
```

Set-report events are observations, not rumble claims:

```json
{"type":"set_report","protocol":3,"event_sequence":1,"report_type":"output","report_id":1,"report":[0,0]}
```

Get-report events record the request. Milestone one returns
`kIOReturnUnsupported` to IOKit and does not invent report contents:

```json
{"type":"get_report","protocol":3,"event_sequence":2,"report_type":"feature","report_id":1,"max_size":64}
```

Fatal errors are structured:

```json
{"type":"fatal","protocol":3,"class":"entitlement_rejected","message":"virtual HID device creation was rejected"}
```

The fatal message must be bounded and must not expose a helper path. Add golden
fixtures for all examples above and round-trip them in tests.

### 7.4 Sequence rules

- Parent starts at sequence `1` after ready.
- Sequence zero is invalid.
- Input and shutdown share one strictly increasing sequence space.
- Helper rejects duplicates, gaps, and decreasing sequences.
- Parent rejects applied responses for an unknown, already acknowledged, or
  future sequence.
- Host get/set-report `event_sequence` is a separate helper-owned monotonic
  counter and has no parent acknowledgement.

## 8. Helper implementation

The binary accepts only these flags:

```text
sc-virtual-hid-helper [--dry-run] [--self-test]
```

- No flag means live IOKit mode.
- `--dry-run` exercises the exact IPC and sequencing logic without calling
  IOKit or requiring the entitlement.
- `--self-test` validates constants and exits without reading stdin.
- Any other flag is an error.

### 8.1 Live creation order

Implement this exact order in `platform/macos.rs`:

1. Build a `CFData` from `GAMEPAD_REPORT_DESCRIPTOR`.
2. Build a `CFDictionary` with:
   - report descriptor;
   - vendor ID `0xcafe`;
   - product ID `0x4001`;
   - product `Steam Controller Bridge Virtual Gamepad`;
   - manufacturer `Lynxware`;
   - transport `Virtual`;
   - primary usage page `0x01` (Generic Desktop);
   - primary usage `0x05` (Game Pad).
3. Call `IOHIDUserDevice::with_properties` with
   `IOHIDUserDeviceOptions::CreateOnActivate.0`.
4. Before creation, inspect the current process with the public Security C
   functions `SecTaskCreateFromSelf`, `SecTaskCopySigningIdentifier`, and
   `SecTaskCopyValueForEntitlement`. Confine their raw declarations and CF
   ownership to the same macOS unsafe boundary. This provides the ready-message
   signing identifier and whether the virtual-device entitlement is effective.
5. If creation returns `None`, combine that inspection with the creation result:
   report `entitlement_missing` when the effective value is absent/false, and
   otherwise report `entitlement_rejected` or `device_creation_failed` without
   claiming more precision than the APIs provide. Emit one fatal message and
   exit nonzero.
6. Create one private serial `DispatchQueue`.
7. Register set-report and get-report blocks. The blocks copy report bytes
   immediately; they must never retain an IOKit buffer pointer.
8. Register a cancellation handler that signals a Rust channel or semaphore.
9. Set the dispatch queue.
10. Activate the device.
11. Dispatch the fixed neutral report with a `mach_absolute_time()` timestamp.
12. Check for `kIOReturnSuccess` before returning ready.

Keep the device, queue, copied blocks, callback sender, and cancellation signal
inside one owner. Blocks must not outlive captured Rust state. The device must
not be released before the cancellation callback runs.

### 8.2 Report dispatch

For every valid `input_report`:

1. Verify exactly 20 bytes, header `00 14`, and zero reserved bytes.
2. Copy the array into helper-owned storage.
3. Call `handle_report_with_time_stamp` with a fresh `mach_absolute_time()`.
4. Emit `applied` only for `kIOReturnSuccess`.
5. Otherwise emit `fatal` with class `dispatch_failed` and exit nonzero after
   best-effort cancellation.

### 8.3 Shutdown and EOF

For explicit shutdown:

1. Dispatch neutral even if the last report was already neutral.
2. Require success.
3. Cancel the device.
4. Wait at most the configured helper-internal cancellation bound.
5. Release IOKit and block ownership.
6. Emit `applied` for the shutdown sequence.
7. Flush stdout and exit zero.

For EOF or a broken stdout pipe, perform the same neutral/cancel/release sequence
best-effort, but no acknowledgement can be delivered. Never remain alive after
the parent pipe closes.

In dry-run mode, model the same states (`AwaitCreate`, `Ready`, `ShuttingDown`,
`Finished`) and replies. Do not create a simplified second protocol path.

## 9. Parent worker implementation

`VirtualHidOutput` implements `bridge_output::GamepadOutput`. Blocking pipe I/O
and acknowledgement waits live on a dedicated worker thread; ordinary
`send_state` calls must not block the bridge supervisor.

### 9.1 Startup

`VirtualHidOutput::open` must:

1. Validate config.
2. Require an absolute helper path for packaged use. CLI development paths may
   be canonicalized before configuration is built.
3. Check that the helper is a regular executable file.
4. Spawn the exact path with stdin/stdout piped and stderr inherited.
5. Add `--dry-run` only when `config.dry_run` is true.
6. Start a stdout reader thread before sending create so unsolicited fatal or
   callback traffic cannot fill a pipe.
7. Send create.
8. Wait for ready within `startup_timeout` while accepting fatal responses.
9. Reject a wrong protocol version.
10. Start the report worker and return the output only after ready.

Do not invoke a shell and do not search `PATH`.

### 9.2 Mailbox

Use a bounded `Mutex<VecDeque<WorkItem>>` plus `Condvar`, or an equivalently
bounded structure that can implement priority neutralization. Required
semantics:

- If an unsent ordinary state is already at the back, replace it with the new
  state and increment `reports_coalesced`.
- If no replaceable state exists and the queue is full, record a fatal local
  `QueueOverflow`; subsequent `service` and sends report output loss.
- A neutral work item removes all queued ordinary states before it is added.
- Shutdown removes ordinary states and runs after its own neutral dispatch.
- Control messages cannot be starved by continuous controller input.

Each work item that requires synchronous safety has a one-shot result sender.
Do not reuse the supervisor command channel for helper acknowledgements.

### 9.3 `GamepadOutput` behavior

`send_state`:

- validates `GamepadState`;
- encodes exactly 20 bytes;
- checks the worker failure latch;
- enqueues/coalesces without waiting for helper acknowledgement; and
- returns an error immediately if the mailbox is lost.

Override `send_neutral`:

- enqueue a priority neutral;
- wait for its exact applied sequence;
- bound the wait with `acknowledgement_timeout`; and
- mark the output lost on timeout.

`service`:

- drains callback events and worker status;
- updates diagnostics;
- returns a classified error if the worker/helper has failed; and
- never waits indefinitely.

`take_feedback` returns `None` in milestone one. Raw set reports update virtual
HID diagnostics but do not become `OutputFeedback::Rumble`.

`Drop`:

- request shutdown unless the helper is already known dead;
- wait within `shutdown_timeout` for neutral, cancellation, acknowledgement,
  and process exit;
- kill only the exact child process if the deadline expires;
- join worker/reader threads; and
- never panic.

The parent must not acknowledge a runtime stop/sleep/update command until this
drop path has completed.

## 10. Bridge output and diagnostics

Extend `OutputDiagnostics` in `crates/bridge-output/src/lib.rs` with counters:

```rust
pub virtual_reports_dispatched: u64,
pub virtual_reports_coalesced: u64,
pub virtual_helper_restarts: u64,
pub virtual_protocol_failures: u64,
pub virtual_set_reports_received: u64,
pub virtual_get_reports_received: u64,
pub virtual_fatal_errors: u64,
```

Keep it `Copy` and numeric. Do not put unbounded strings in this structure.

Add a separate configuration-error variant so runtime code can stop permanent
respawn loops without parsing display strings:

```rust
pub enum OutputError {
    // existing variants
    Transport(String),
    Configuration(String),
}
```

Map helper protocol, entitlement, unsupported-platform, and deterministic
creation failures to `Configuration`. Map a child crash, broken pipe, and
isolated dispatch loss to `Transport`.

Update every exhaustive `OutputError` match and its tests.

## 11. Runtime integration

### 11.1 Public API

In `crates/bridge-runtime/src/api.rs`, add:

```rust
pub enum OutputSelection {
    Serial,
    VirtualHid(VirtualHidConfig),
    Dump(DumpFormat),
    File(PathBuf),
    Mock,
}

pub enum OutputBackend {
    SerialBridge,
    VirtualHid,
    Dump,
    File,
    Mock,
}
```

Add helper metadata to `OutputStatus` without exposing the helper path:

```rust
pub struct VirtualHidStatus {
    pub protocol_version: u16,
    pub vendor_id: u16,
    pub product_id: u16,
    pub bundle_identifier: Option<String>,
    pub signing_identifier: Option<String>,
    pub entitlement_present: Option<bool>,
    pub dry_run: bool,
}

pub struct OutputStatus {
    // existing fields
    pub virtual_hid: Option<VirtualHidStatus>,
}
```

For a ready virtual output:

- backend is `VirtualHid`;
- endpoint is `Some("macOS virtual gamepad")`;
- stable ID is `None`;
- ready is `true` only after helper ready;
- firmware is `None`; and
- virtual helper metadata is present.

### 11.2 Explicit capabilities

Stop using `OutputSession.device.is_none()` to infer whether an output is live
or eligible for automatic controller shutdown.

Rename `device` to `serial_device` and add explicit capabilities:

```rust
#[derive(Debug, Clone, Copy)]
pub(crate) struct OutputCapabilities {
    pub(crate) live: bool,
    pub(crate) controller_shutdown: bool,
    pub(crate) firmware: bool,
}
```

Values:

| Backend | live | controller shutdown | firmware |
| --- | --- | --- | --- |
| Serial | yes | yes | yes |
| Virtual HID | yes | yes | no |
| Dump/File/Mock | no | no | no |

In `active_session.rs`, replace the `output.device.is_none()` automatic-shutdown
guard with `!output.capabilities.controller_shutdown`. Continue to neutralize
the selected output before calling `worker.power_off()`.

Only call firmware receipt/version logic when `capabilities.firmware` and
`serial_device.is_some()` are both true.

### 11.3 Discovery and retry policy

`discover_output` must handle three categories:

- Serial: existing serial discovery and handshake.
- Virtual HID: construct `VirtualHidOutput::open` once per scheduled attempt.
- Passive output: existing dump/file/mock construction.

Add a `Discovery::Blocked(String)` or equivalent explicit state for permanent
configuration failures. Do not reuse `Discovery::Error` if its current loop
retries every 500 ms.

Required policy:

- Missing helper, unsupported platform, invalid config, protocol mismatch,
  entitlement rejection, and deterministic creation failure become blocked.
- A blocked output is not retried until explicit Start, backend selection
  change, or process restart.
- Transient child/process loss may retry with exponential backoff: 1, 2, 4, 8,
  16, then 30 seconds maximum.
- A successful ready resets the backoff.
- No retry selects another backend.

Add `ActiveExit::OutputBlocked(String)` so a permanent failure discovered by
`service()` can latch the same blocked state.

### 11.4 Sleep monitor

The current runtime creates the macOS power monitor only when the initial output
is serial. That is insufficient for runtime switching.

On macOS:

- attempt to create the power monitor independently of initial backend;
- retain a monitor-startup error separately from general runtime state;
- allow passive dump/file/mock output to operate if the monitor is unavailable;
- reject Start or a switch into Serial/VirtualHid while the required monitor is
  unavailable; and
- use the existing 25-second sleep teardown acknowledgement bound.

Both Serial and VirtualHid must close before `WillSleep` is acknowledged and
must be rediscovered after the existing wake-settle delay.

### 11.5 Runtime backend switching

Add:

```rust
RuntimeCommand::SetOutput(OutputSelection, CommandAck)
```

Expose a nonblocking begin/poll API modeled on `PendingUpdateResume`, because
the menu event loop must not block for teardown:

```rust
pub struct PendingOutputChange { /* private */ }

pub enum OutputChangePoll {
    Pending,
    TimedOut,
    Complete(Result<(), RuntimeError>),
}

impl BridgeHandle {
    pub fn begin_set_output(
        &self,
        selection: OutputSelection,
    ) -> Result<PendingOutputChange, RuntimeError>;
}
```

Command rules:

1. Reject the command while updater suspension is active.
2. If the requested selection equals the current selection, acknowledge
   success without cycling hardware.
3. In an active session, return an `ActiveExit::OutputChange` carrying the new
   selection and acknowledgement.
4. Run normal engine shutdown so the old output receives neutral.
5. Drop the old output. For virtual HID, this waits for helper cancellation and
   process exit within its bounds.
6. Only after drop succeeds, replace `config.output` and publish configured/not
   ready status for the new backend.
7. Acknowledge the selection change.
8. Let the next supervisor iteration create/discover the new output.

If old-output cleanup fails, do not apply or persist the new selection. The old
session may already have been released; keep its selection configured and let
normal discovery recover it.

For an idle retained output, store a pending change in `Supervisor`, return to
the top-level `run` loop, neutralize/drop `retained_output`, then update config
and acknowledge. Do not acknowledge directly inside `apply_idle_command` while
`retained_output` still exists outside that function.

Backend switching may restart controller discovery. Preserving the active HID
input worker is an optimization, not a requirement for this milestone.

## 12. CLI integration

Use the Clap value `virtual-hid` in all three tools.

### `sc-bridge`

Add:

```text
--output virtual-hid
--virtual-hid-helper PATH
--virtual-hid-vendor-id VID
--virtual-hid-product-id PID
```

Rules:

- Virtual HID requires an explicit helper path.
- The helper flag is invalid unless virtual HID is selected.
- VID and PID overrides are optional but must be supplied together; they are
  invalid unless virtual HID is selected.
- Live automatic controller shutdown is permitted for Serial and VirtualHid.
- Replay virtual HID is permitted and requires the helper path.
- Preserve existing mode-specific defaults: live defaults to serial and replay
  defaults to dump.

### `sc-replay`

Add the same output, helper-path, and paired identity options. Validate all
arguments before opening the recording or spawning the helper.

### `gamepad-simulator`

Add the same output, helper-path, and paired identity options. This is the
primary manual VM test driver, so its help text must show a complete example.

All tools must call `send_neutral` on normal completion. Interactive and delayed
loops must continue calling `service()` at least every 25 ms.

## 13. Menu app and settings

### 13.1 Settings migration

Increment `SETTINGS_VERSION` from 3 to 4.

Add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub(super) enum OutputPreference {
    #[default]
    XiaoUsbBridge,
    VirtualHid,
}
```

Add `#[serde(default)] output: OutputPreference` to `AppSettings`. Accept
versions 1, 2, 3, and 4 when loading, then write version 4. Every older settings
file migrates to `XiaoUsbBridge`.

Tests must cover:

- missing settings file;
- every older accepted version;
- version 3 without an output field becoming XIAO;
- version 4 round trip for both choices;
- invalid enum value falling back through the existing invalid-settings path.

### 13.2 Bundled helper resolution

Add a function that resolves exactly:

```text
<outer app>/Contents/Helpers/
  Steam Controller Bridge Virtual HID Helper.app/
    Contents/MacOS/sc-virtual-hid-helper
```

Start from `std::env::current_exe()`, whose packaged location is
`Contents/MacOS/sc-bridge-menu`. Do not canonicalize into a writable search
directory and do not search `PATH`.

Build `RuntimeConfig.output` from settings:

- XIAO -> `OutputSelection::Serial`;
- VirtualHid -> `OutputSelection::VirtualHid(VirtualHidConfig::new(exact_path))`.

### 13.3 Output submenu

Add an `Output` submenu with checked items:

- `XIAO USB Bridge`
- `Virtual Gamepad — Experimental`

Use stable menu IDs such as `output-xiao` and `output-virtual-hid`.

Add `pending_output_change: Option<...>` to `MenuApp`. On selection:

1. Ignore an already selected item.
2. Refuse another selection while one is pending.
3. Call `begin_set_output`.
4. Disable both items while pending.
5. Poll from `about_to_wait` with the same discipline used by updater recovery.
6. On success, update `AppSettings`, save atomically, update checkmarks, and
   enable the items.
7. On failure, retain the old setting/checkmark, enable the items, and expose
   the runtime error.
8. A timeout is visible but not terminal; retain and continue polling the
   request because the acknowledgement may arrive later.

Firmware status and UF2 actions must be absent when `status.output.firmware` is
`None`. Do not manufacture an empty firmware record for virtual HID.

Update menu-model tests for:

- virtual output label and readiness;
- no firmware row for virtual output;
- output selection checkmarks;
- pending switch disabled state;
- failed switch rollback; and
- XIAO remaining the default.

## 14. Packaging and signing

### 14.1 Helper bundle files

`VirtualHidHelper.Info.plist` must contain at least:

- `CFBundleExecutable = sc-virtual-hid-helper`;
- a dedicated helper bundle identifier, initially
  `com.lynxware.steam-controller-bridge.virtual-hid-helper`;
- `CFBundlePackageType = APPL`;
- version placeholders stamped from the workspace version;
- `LSMinimumSystemVersion = 13.0`; and
- no `LSUIElement` requirement unless packaging tests show it is needed.

`VirtualHidHelper.entitlements` contains only:

```xml
<key>com.apple.developer.hid.virtual.device</key>
<true/>
```

Do not copy that key into the outer app entitlement set.

### 14.2 Builder behavior

Extend `tools/build-macos-app.py` in this order:

1. Build both `sc-bridge-menu` and `sc-virtual-hid-helper` in release mode.
2. Assemble the outer app.
3. Assemble and stamp the nested helper app.
4. Optionally copy a provisioning profile to the helper as
   `Contents/embedded.provisionprofile`.
5. Sign the helper app first with its entitlement plist.
6. Sign the outer app without `--deep`.
7. Verify the complete result with `codesign --verify --deep --strict`.
8. Inspect entitlements and fail if the helper lacks the virtual-HID key or the
   outer executable contains it.

Support environment/configuration inputs with documented names:

```text
SC_BRIDGE_CODESIGN_IDENTITY
SC_BRIDGE_VIRTUAL_HID_PROVISIONING_PROFILE
SC_BRIDGE_VIRTUAL_HID_HELPER_IDENTIFIER
```

Defaults:

- signing identity is `-` for ad-hoc signing;
- no provisioning profile;
- helper identifier is the value above.

An ad-hoc signature containing a restricted entitlement is not authorization.
On normal-security macOS the helper may be rejected before `main` or virtual
device creation may fail. Treat that as expected until an approved provisioning profile is
available.

Do not use `codesign --deep` to sign. `--deep --strict` remains a verification
step only.

Extend the builder self-test so it proves nested placement, version stamping,
safe removal bounds, and construction of inside-out signing commands without
requiring an Apple identity.

## 15. Documentation deliverables

Create `docs/VIRTUAL_HID.md` containing:

- user-facing experimental status;
- XIAO default and no-fallback behavior;
- restricted-entitlement limitation;
- normal-security expectations for ad-hoc builds;
- exact CLI examples;
- sleep/quit safety behavior;
- input-only/rumble limitation; and
- a link to the VM runbook and feasibility matrix.

Create `docs/VIRTUAL_HID_IPC.md` from section 7 and the checked-in fixtures.

Create `docs/VIRTUAL_HID_FEASIBILITY.md` with this matrix:

| Layer | Status | Evidence | Notes |
| --- | --- | --- | --- |
| Rust compile | pending | command/log | all supported targets |
| Dry-run IPC | pending | test name | no entitlement |
| Packaged helper | pending | bundle/signature inspection | no live claim |
| VM device creation | blocked | `hidutil` capture | needs lowered-security VM |
| HID reports | blocked | monitor capture | after enumeration |
| System Settings | blocked | screenshot/notes | after reports |
| Browser Gamepad API | blocked | offline tester capture | after enumeration |
| SDL | blocked | local client output | after enumeration |
| Physical controller E2E | blocked | test notes | after simulator |
| Sleep/wake | blocked | test notes | physical/VM proof |
| Rumble | blocked | raw output-report capture | no implementation yet |
| Normal-security signed build | blocked | approved provisioning profile | paid account/Apple approval |

Use `blocked` when a prerequisite is missing. Do not mark downstream clients as
failed merely because enumeration has not yet passed.

Update the existing architecture, testing, technical guide, and README to link
these documents. Keep all macOS 13 badges and requirements unchanged.

## 16. Automated test plan

### 16.1 Contract tests

Add tests for:

- descriptor golden bytes;
- descriptor length and pinned bytes;
- exact neutral bytes;
- each supported XInput button mapping;
- every hat direction as XInput D-pad bits and centered state;
- axis values `-1.0`, `0.0`, `1.0`;
- trigger values `0.0`, `0.5`, `1.0` with pinned rounding;
- invalid non-finite/out-of-range states;
- report length, header, and reserved-byte rejection;
- every JSON golden fixture;
- unknown fields;
- wrong protocol;
- zero, duplicate, skipped, and decreasing sequences;
- oversized line and report rejection; and
- bounded fatal messages.

### 16.2 Worker tests

Use a fake/dry-run helper process to prove:

- ready follows create;
- startup times out;
- fatal before ready is classified;
- input report gets the matching applied sequence;
- unsolicited get/set events do not break acknowledgement handling;
- ordinary pending states coalesce;
- neutral removes queued ordinary states;
- unserviceable overflow latches output loss;
- acknowledgement timeout latches output loss;
- protocol mismatch is permanent;
- helper exit is distinct from dispatch failure;
- explicit shutdown neutralizes and reaps the child; and
- dropping/killing the parent side does not leave the child running.

### 16.3 Runtime tests

Add tests for:

- configured and ready virtual status;
- firmware remains `None`;
- explicit output capabilities;
- automatic shutdown allowed for Serial and VirtualHid only;
- Serial -> VirtualHid switch ordering;
- VirtualHid -> Serial switch ordering;
- old output neutral before drop;
- acknowledgement after old output release;
- switch rejected during updater suspension;
- switch while system-sleep suspended updates configuration but creates no
  hardware until wake;
- stop, quit, sleep, and updater teardown ordering;
- wake/resume rediscovery;
- permanent failure latch and explicit retry;
- transient failure backoff; and
- no fallback to another backend.

Prefer fake `GamepadOutput` and fake helper executables. Unit tests must not
depend on the restricted entitlement.

### 16.4 CLI and menu tests

Test every new output name and argument relationship. Invalid combinations must
fail before files are created or helpers are spawned.

Test settings v1-v4 migration, both output choices, menu checkmarks, pending
switch state, persistence after success only, and firmware-row absence.

### 16.5 Packaging gates

On macOS CI or a local Mac, prove:

- helper release build;
- exact nested path;
- both deployment targets are 13.0;
- helper bundle/version metadata;
- entitlement present on helper;
- entitlement absent from outer executable;
- helper signed before outer app;
- outer signing command does not contain `--deep`; and
- final `codesign --verify --deep --strict` succeeds.

## 17. Required validation commands

Run from the repository root after implementation:

```bash
python3 tools/build-macos-app.py --self-test
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo deny check
make -C firmware/xiao-nrf52840 test
```

Also retain the repository's Linux portability proof when the target is
installed:

```bash
cargo clippy --workspace --all-targets --all-features \
  --target x86_64-unknown-linux-gnu -- -D warnings
```

On macOS:

```bash
./tools/build-macos-app.py
codesign --verify --deep --strict "dist/Steam Controller Bridge.app"
```

Run the dry-run integration directly, substituting the actual Cargo test name:

```bash
cargo test -p macos-virtual-hid --test dry_run_ipc
```

Do not run live helper creation on the development host without the approved
entitlement. A failed ad-hoc live launch is diagnostic evidence, not a failed
automated gate.

## 18. Disposable VM live-proof runbook

This is a manual experiment, not part of CI.

Use VirtualBuddy on the current Apple-silicon host because it supports macOS
recovery boot, shared folders, and cheap duplication.

### 18.1 Create the disposable guest

1. Create a clean stable macOS VM.
2. Install no personal accounts.
3. Copy the packaged test artifact into the guest.
4. Shut down the VM.
5. Duplicate it as a disposable lowered-security clone.
6. Never execute the security commands below on the host.

### 18.2 Lower security in the clone only

In the clone's Recovery environment:

```bash
csrutil disable
```

Boot the clone normally and run:

```bash
sudo nvram boot-args="amfi_get_out_of_my_way=0x1"
sudo reboot
```

After reboot, capture:

```bash
csrutil status
nvram boot-args
codesign -dvvv --entitlements :- \
  "Steam Controller Bridge.app/Contents/Helpers/Steam Controller Bridge Virtual HID Helper.app"
```

Disconnect guest networking after artifact transfer. Do not enter Apple,
Steam, browser-sync, cloud-gaming, password-manager, or other personal
credentials.

### 18.3 Prove layers in order

First use `gamepad-simulator`; no Steam Controller or USB passthrough is needed.

1. Open System Settings > Game Controllers.
2. Launch the simulator with the exact packaged helper path and no identity
   override, thereby testing the fixed `045e:028e` contract.
3. Capture `hidutil list` and `GCController.controllers`.
4. Capture deterministic neutral, buttons, hat, axes, and triggers in a HID
   monitoring tool.
5. Confirm neutral at startup, explicit stop, input timeout, and shutdown.
6. Kill the parent and confirm the virtual service disappears.
7. Only if GameController or System Settings recognizes the device, record
   browser Gamepad API `id`, `mapping`, axes, buttons, and disconnect.
8. Record a local SDL test client's enumeration and mapping after that same
   prerequisite passes.
9. Only after simulator success, test physical Steam Controller end to end.

Use the exact commands in `docs/VIRTUAL_HID_FEASIBILITY.md`. If an identity
experiment is needed, supply both VID and PID flags and change no other
contract variable.

Update `docs/VIRTUAL_HID_FEASIBILITY.md` after each layer. Preserve command
output and concise observations; do not infer downstream compatibility.

Delete the lowered-security clone after testing. Retain only the clean baseline
for repeatability.

## 19. Implementation phases and stop gates

### Phase 1: contract only

- [x] Add the package and platform-neutral contract.
- [x] Add descriptor/report golden tests.
- [x] Add IPC types, fixtures, and strict decoding tests.
- [ ] Keep the workspace green on macOS and Linux. macOS passes; the new crate
  passes Linux cross-Clippy, while the full workspace is environment-blocked as
  recorded above.

Stop gate: do not write IOKit ownership code until descriptor and IPC tests pass.

### Phase 2: dry-run helper and parent worker

- [x] Implement helper state machine in dry-run mode.
- [x] Implement bounded parent worker and sequencing.
- [x] Prove neutral/shutdown and child cleanup.
- [x] Prove permanent/transient error classification.

Stop gate: do not integrate runtime/menu until dry-run process tests pass.

### Phase 3: live IOKit provider

- [x] Implement the narrow macOS FFI owner.
- [x] Add callbacks, activation, report dispatch, and cancellation.
- [x] Compile and self-test without attempting unauthorized live creation.
- [x] Review every unsafe block and callback lifetime.

Stop gate: no normal-host live-success claim is allowed.

### Phase 4: runtime and tools

- [x] Add output selection, capabilities, status, and diagnostics.
- [x] Add safe runtime switching.
- [x] Add retry/block policy.
- [x] Add CLI, replay, and simulator support.
- [x] Run runtime and CLI tests.

Stop gate: switching tests must prove neutral-before-release and ack-after-drop.

### Phase 5: menu and packaging

- [x] Add settings migration and Output submenu.
- [x] Add exact bundled-helper resolution.
- [x] Assemble and sign nested helper inside-out.
- [x] Add packaging entitlement assertions.
- [x] Update documentation.

Stop gate: outer app must not contain the virtual-device entitlement.

### Phase 6: automated completion

- [ ] Run every command in section 17. All native commands pass; full-workspace
  Linux cross-Clippy is environment-blocked as recorded above.
- [x] Fix all warnings and failures in runnable gates.
- [x] Record which proof is automated versus blocked/manual.
- [x] Perform a final changed-code review for reuse, clarity, and efficiency.

Stop gate: do not describe the feature as live-tested.

### Phase 7: disposable VM proof

- [x] Create and isolate the disposable lowered-security clone.
- [x] Prove HID enumeration through `hidutil`.
- [x] Prove changing reports through `sc-probe`.
- [x] Prove the fixed `045e:028e` contract reaches GameController and the
  offline Gamepad API tester.
- [ ] Prove neutralization for the fixed contract.
- [ ] Prove cleanup.
- [ ] Test browser, SDL, and physical input in prerequisite order.
- [ ] Update the feasibility matrix.
- [ ] Delete the lowered-security clone.

Stop gate: lowered-security success does not prove normal-security distribution.

## 20. Promotion criteria

Keep `Virtual Gamepad — Experimental` until all of these are true:

1. Disposable VM enumeration and reports are proven.
2. Browser and SDL clients expose usable controls.
3. Physical Steam Controller input works end to end.
4. Stop, quit, helper crash, sleep, and wake behavior are proven.
5. Apple confirms the chosen public API is acceptable for the entitlement.
6. A paid Apple Developer account has the managed entitlement for the helper
   identifier.
7. The provisioning profile contains the entitlement.
8. A normally secured Mac runs the signed/notarized artifact successfully.
9. Target games and cloud clients pass compatibility testing.
10. Rumble is either implemented from captured evidence or explicitly remains
    unsupported.

If `IOHIDUserDevice` fails the Apple-approval gate but the rest of the system is
sound, do not rewrite the application. Introduce a CoreHID Swift helper only as
a new provider behind the same versioned IPC contract, after explicit approval
to add Swift.

## 21. Final handoff checklist

An implementation handoff is incomplete unless it reports all of the following:

- files and public interfaces added;
- backend-switch safety behavior;
- helper entitlement placement;
- exact automated commands and outcomes;
- macOS bundle/signature verification outcome;
- whether live virtual-device creation was attempted;
- whether a lowered-security VM was used;
- enumeration/report/client results if available;
- remaining manual or Apple-account gates; and
- confirmation that XIAO firmware was unchanged.
