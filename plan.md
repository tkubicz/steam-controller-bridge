# Steam Controller macOS Bridge — Pre-Hardware Development Plan

## Objective

Build the host-side architecture, protocol model, tooling, tests, and firmware-facing transport required for a Steam Controller translation bridge.

The immediate goal is to complete as much of the system as possible before the Seeed Studio XIAO nRF52840 arrives.

The intended end-to-end architecture is:

```text
Steam Controller
        │
        │ proprietary HID reports
        ▼
macOS host application
        │
        ├── device discovery
        ├── Steam Controller protocol decoding
        ├── normalization
        ├── mapping and filtering
        └── generic gamepad state
        │
        │ framed binary protocol over USB CDC
        ▼
XIAO nRF52840
        │
        │ physical USB HID gamepad
        ▼
macOS
        │
        ├── browsers
        ├── Xbox Cloud Gaming
        ├── GeForce NOW
        └── Boosteroid
```

The nRF52840 firmware is not part of the primary implementation yet, but its protocol and interfaces must be designed now.

---

# 1. Primary goals

Implement:

1. A platform-neutral gamepad state model.
2. A stable, versioned host-to-device binary protocol.
3. Steam Controller HID enumeration and report capture.
4. Steam Controller report decoding for basic controls.
5. Normalization and mapping into a conventional gamepad layout.
6. Record and replay tooling.
7. A controller-state visualizer.
8. Mock, dump, file, and serial output backends.
9. A keyboard-driven gamepad simulator.
10. Comprehensive unit and integration tests.
11. Documentation for the Steam Controller protocol and future firmware.
12. CI for formatting, linting, tests, and builds.

Do not implement the final nRF52840 firmware until the hardware arrives.

---

# 2. Non-goals

Do not implement yet:

- A production macOS GUI application
- SwiftUI
- A background launch agent
- Automatic startup
- App notarization
- Apple restricted virtual-HID entitlements
- DriverKit
- Kernel extensions
- Bluetooth communication with the XIAO
- nRF52840 firmware flashing
- OpenPuck integration
- Advanced gyro aiming
- Complex action layers
- Per-game profiles
- Cloud synchronization
- Steam Overlay integration
- Haptic feedback
- Final installer or packaging

Basic architecture should allow these to be added later.

---

# 3. Project structure

Create a Rust workspace.

Suggested structure:

```text
steam-controller-bridge/
├── Cargo.toml
├── README.md
├── POC_PLAN.md
├── LICENSE
├── rustfmt.toml
├── deny.toml
├── .github/
│   └── workflows/
│       └── ci.yml
├── docs/
│   ├── ARCHITECTURE.md
│   ├── GAMEPAD_PROTOCOL.md
│   ├── STEAM_CONTROLLER_PROTOCOL.md
│   ├── RECORDING_FORMAT.md
│   ├── TESTING.md
│   └── FIRMWARE_PLAN.md
├── crates/
│   ├── gamepad-state/
│   ├── bridge-protocol/
│   ├── steam-controller-protocol/
│   ├── steam-controller-device/
│   ├── controller-mapper/
│   ├── recording/
│   ├── bridge-output/
│   └── bridge-core/
└── apps/
    ├── sc-probe/
    ├── sc-record/
    ├── sc-replay/
    ├── sc-visualizer/
    ├── gamepad-simulator/
    └── sc-bridge/
```

Keep crates small and dependency-light.

Avoid creating many abstractions before they are used, but preserve separation between:

- physical input;
- protocol decoding;
- normalized state;
- mappings;
- output transport.

---

# 4. Core architecture

Use a pipeline architecture:

```text
Input source
    │
    ▼
Raw packet
    │
    ▼
Protocol decoder
    │
    ▼
Source controller state
    │
    ▼
Normalization
    │
    ▼
Mapping and filters
    │
    ▼
Generic GamepadState
    │
    ▼
Output backend
```

Suggested traits:

```rust
pub trait InputSource {
    type RawInput;

    fn poll(&mut self) -> Result<Option<Self::RawInput>, InputError>;
}

pub trait Decoder<Raw, State> {
    fn decode(&mut self, raw: &Raw) -> Result<State, DecodeError>;
}

pub trait Mapper<Input, Output> {
    fn map(&mut self, input: &Input) -> Output;
}

pub trait GamepadOutput {
    fn send_state(&mut self, state: &GamepadState) -> Result<(), OutputError>;
}
```

Do not force all implementations into a synchronous polling model if macOS HID callbacks are more natural.

It is acceptable for the macOS HID layer to use callbacks internally and expose decoded events through a channel.

---

# 5. Generic gamepad state

Create a platform-neutral representation.

Recommended initial model:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GamepadButtons(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum HatState {
    North = 0,
    NorthEast = 1,
    East = 2,
    SouthEast = 3,
    South = 4,
    SouthWest = 5,
    West = 6,
    NorthWest = 7,
    #[default]
    Centered = 8,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GamepadState {
    pub buttons: GamepadButtons,
    pub hat: HatState,

    pub left_x: f32,
    pub left_y: f32,

    pub right_x: f32,
    pub right_y: f32,

    pub left_trigger: f32,
    pub right_trigger: f32,
}
```

Conventions:

- Stick axes: `-1.0..=1.0`
- Triggers: `0.0..=1.0`
- Neutral sticks: `0.0`
- Released triggers: `0.0`
- Released buttons: zero
- Neutral D-pad: `Centered`

Provide:

- clamping;
- finite-value validation;
- neutral-state construction;
- button helper methods;
- stable button indices;
- conversion to firmware packet representation.

Define standard button names:

```text
South
East
West
North
LeftShoulder
RightShoulder
LeftStick
RightStick
Back
Start
Guide
LeftGrip
RightGrip
Extra1
Extra2
Extra3
```

Document their bit positions.

---

# 6. Host-to-device protocol

Design a fixed-size, versioned binary protocol for communication with the XIAO.

The protocol must support:

- reliable framing over USB CDC;
- version negotiation or rejection;
- sequence numbers;
- corruption detection;
- neutral-state messages;
- future extension;
- deterministic serialization;
- no Rust layout assumptions across devices.

Do not serialize Rust structs by directly copying memory.

Use explicit byte encoding.

## Suggested frame

```text
Offset  Size  Field
0       2     Magic
2       1     Protocol version
3       1     Message type
4       2     Payload length
6       2     Sequence number
8       N     Payload
8+N     2/4   Checksum
```

Possible magic:

```text
0x53 0x43
```

Meaning:

```text
"SC"
```

Suggested message types:

```rust
#[repr(u8)]
pub enum MessageType {
    Hello = 1,
    HelloResponse = 2,
    GamepadState = 3,
    Neutral = 4,
    Ping = 5,
    Pong = 6,
    DeviceInfo = 7,
    Error = 255,
}
```

## Gamepad payload

Use explicit integer fields:

```text
buttons       u32
hat           u8
flags         u8
left_x        i16
left_y        i16
right_x       i16
right_y       i16
left_trigger  u16
right_trigger u16
```

Suggested conversions:

- `-1.0` stick → `-32767`
- `0.0` stick → `0`
- `1.0` stick → `32767`
- `0.0` trigger → `0`
- `1.0` trigger → `65535`

Reserve `i16::MIN` unless explicitly needed.

## Framing requirements

The decoder must:

- recover after malformed bytes;
- search for the next magic sequence;
- reject unsupported versions;
- reject impossible payload sizes;
- verify checksum before decoding;
- ignore unknown message types safely;
- handle partial reads;
- handle multiple frames in one read;
- impose a maximum payload size.

Use CRC-16 or CRC-32.

Document the selected algorithm precisely.

---

# 7. Bridge protocol crate

Create `bridge-protocol`.

Responsibilities:

- packet types;
- serialization;
- deserialization;
- stream framing;
- checksums;
- protocol version constants;
- compatibility checks;
- test vectors.

Required tests:

- round-trip every message type;
- partial frame parsing;
- multiple frames in one buffer;
- corrupted checksum;
- invalid magic;
- unsupported version;
- oversized payload;
- random garbage before a valid frame;
- truncated frame;
- recovery after a malformed frame;
- all minimum and maximum axis values.

Generate static protocol test vectors that can later be copied into firmware tests.

---

# 8. Steam Controller device discovery

Create `steam-controller-device`.

Use macOS HID APIs or a suitable cross-platform HID library.

Prefer a design that allows using native IOKit later if a high-level HID library is insufficient.

The device layer should:

- enumerate HID devices;
- print vendor ID;
- print product ID;
- print usage page;
- print usage;
- print transport;
- print product and manufacturer strings;
- detect all HID collections belonging to the controller;
- open the appropriate collection;
- receive input reports;
- send feature reports;
- handle disconnect;
- handle reconnect.

Create the diagnostic application:

```text
sc-probe
```

Suggested commands:

```bash
sc-probe list
sc-probe inspect
sc-probe monitor
sc-probe monitor --raw
sc-probe capture --output reports.jsonl
```

The application must not assume device identifiers before enumeration confirms them.

Log enough metadata to distinguish:

- USB;
- official wireless receiver;
- Bluetooth.

---

# 9. Raw report model

Represent every received HID report with metadata:

```rust
pub struct RawHidReport {
    pub timestamp: std::time::Duration,
    pub report_id: u8,
    pub data: Vec<u8>,
}
```

For the hot path, a fixed-capacity buffer may later replace `Vec<u8>`.

Initially prioritize correctness and diagnostics.

Record:

- monotonic timestamp;
- report ID;
- report length;
- raw bytes;
- source device identifier;
- transport;
- dropped-report counters where detectable.

---

# 10. Steam Controller protocol decoder

Create `steam-controller-protocol`.

Separate:

- raw report parsing;
- decoded controller state;
- controller initialization;
- feature report commands;
- lizard-mode handling;
- heartbeat or keepalive behavior.

Suggested decoded source model:

```rust
pub struct SteamControllerState {
    pub buttons: SteamButtons,

    pub left_stick_x: i16,
    pub left_stick_y: i16,

    pub left_pad_x: i16,
    pub left_pad_y: i16,
    pub left_pad_touched: bool,
    pub left_pad_pressed: bool,

    pub right_pad_x: i16,
    pub right_pad_y: i16,
    pub right_pad_touched: bool,
    pub right_pad_pressed: bool,

    pub left_trigger: u16,
    pub right_trigger: u16,

    pub gyro: Option<GyroState>,
    pub acceleration: Option<AccelerationState>,
}
```

Begin with basic controls:

- A/B/X/Y
- D-pad
- menu buttons
- shoulders
- triggers
- left stick
- trackpad touch and click
- grip buttons

Gyro and haptics may be decoded later, but preserve raw fields where practical.

## Decoder requirements

- No panics on malformed input.
- Validate report size.
- Validate report ID.
- Preserve unknown bytes for diagnostics.
- Support multiple known firmware or report versions when discovered.
- Emit structured decode errors.
- Add tests using captured and synthetic packets.

---

# 11. Controller initialization

Research and implement the minimal initialization sequence needed to prevent keyboard/mouse compatibility behavior.

Likely responsibilities:

- disable lizard mode;
- send required feature report;
- maintain any required heartbeat;
- restore safe device behavior on exit where appropriate.

Do not send undocumented feature reports without:

- recording the source;
- documenting the report bytes;
- isolating them behind clearly named methods;
- providing a dry-run or logging mode.

Suggested API:

```rust
pub trait SteamControllerControl {
    fn disable_lizard_mode(&mut self) -> Result<(), DeviceError>;
    fn keep_alive(&mut self) -> Result<(), DeviceError>;
    fn restore_default_mode(&mut self) -> Result<(), DeviceError>;
}
```

---

# 12. Mapping layer

Create `controller-mapper`.

Map `SteamControllerState` into `GamepadState`.

Initial default mapping:

| Steam Controller input | Generic gamepad output |
| ---------------------- | ---------------------- |
| Left stick             | Left stick             |
| Right trackpad         | Right stick            |
| A/B/X/Y                | South/East/West/North  |
| D-pad input            | Hat switch             |
| Left trigger           | Left trigger           |
| Right trigger          | Right trigger          |
| Left bumper            | Left shoulder          |
| Right bumper           | Right shoulder         |
| Menu button            | Start                  |
| View button            | Back                   |
| Steam button           | Guide                  |
| Left stick click       | Left stick             |
| Right pad click        | Right stick            |
| Rear grips             | Extra buttons          |

Start with an absolute right-trackpad mapping:

```text
trackpad center       → right stick center
trackpad left edge    → -1.0 X
trackpad right edge   → +1.0 X
trackpad top edge     → +1.0 Y
trackpad bottom edge  → -1.0 Y
finger released       → right stick center
```

Do not implement trackball inertia initially.

---

# 13. Input filtering

Implement reusable filters:

- axis dead zone;
- radial dead zone;
- trigger dead zone;
- axis inversion;
- sensitivity curve;
- optional low-pass smoothing;
- saturation;
- output clamping.

Suggested API:

```rust
pub trait StateFilter {
    fn apply(&mut self, state: &mut GamepadState, delta_time: f32);
}
```

Avoid dynamic dispatch in the hot path unless useful.

## Required tests

- center remains neutral;
- values inside dead zone become zero;
- values outside dead zone are rescaled smoothly;
- full-range input remains full range;
- no NaN output;
- no values outside the documented range;
- smoothing converges;
- disconnect resets smoothing state.

---

# 14. Recording format

Create `recording`.

Support recording:

1. Raw HID reports
2. Decoded Steam Controller states
3. Final generic gamepad states

Prefer a newline-delimited format for initial debugging.

Suggested JSONL envelope:

```json
{
  "version": 1,
  "timestamp_us": 123456,
  "kind": "raw_hid",
  "payload": {
    "report_id": 66,
    "bytes": "base64..."
  }
}
```

Other event kinds:

```text
device_connected
device_disconnected
raw_hid
decoded_steam_state
mapped_gamepad_state
warning
error
marker
```

Use a monotonic timestamp relative to recording start.

Document:

- format version;
- timestamp semantics;
- binary encoding;
- forward compatibility;
- unknown event handling.

---

# 15. Recorder application

Create:

```text
sc-record
```

Suggested usage:

```bash
sc-record raw --output capture.jsonl
sc-record decoded --output decoded.jsonl
sc-record full --output session.jsonl
```

Features:

- duration limit;
- stop on Ctrl+C;
- optional device metadata header;
- optional human-readable raw byte output;
- optional markers entered from the keyboard;
- flush data periodically;
- cleanly close files on exit.

Example marker workflow:

```text
Press Enter and type:
"a_press"
"left_trigger_full"
"right_pad_top_left"
```

This will help correlate physical actions with reports.

---

# 16. Replay system

Create:

```text
sc-replay
```

The replay system must support:

- real-time playback;
- accelerated playback;
- slowed playback;
- step-by-step mode;
- looping;
- seeking by timestamp where practical;
- replay into mapper;
- replay into output backend;
- deterministic test mode.

Suggested commands:

```bash
sc-replay session.jsonl
sc-replay session.jsonl --speed 0.5
sc-replay session.jsonl --speed 2.0
sc-replay session.jsonl --loop
sc-replay session.jsonl --output dump
sc-replay session.jsonl --output serial
```

Replay timing should use recorded relative timestamps.

For tests, provide a mode that ignores real timing and processes all events immediately.

---

# 17. Output backends

Create `bridge-output`.

Implement the following backends before hardware arrives.

## 17.1 Mock output

Stores sent states in memory.

Use for tests.

```rust
pub struct MockOutput {
    pub states: Vec<GamepadState>,
}
```

## 17.2 Dump output

Prints every changed state.

Output formats:

- compact text;
- pretty text;
- JSON;
- raw bridge-protocol bytes.

## 17.3 File output

Writes framed bridge packets to a file.

This allows firmware protocol inspection without hardware.

## 17.4 Serial output

Implement serial transport now, even without the board.

Requirements:

- configurable serial port;
- configurable baud rate;
- automatic reconnect;
- clear connection status;
- bounded outgoing queue;
- neutral packet on disconnect where possible;
- protocol hello handshake;
- sequence numbers;
- ping/pong health check;
- optional packet logging.

Use a mock serial transport in tests.

Do not tightly couple bridge framing to a specific serial crate.

Suggested abstraction:

```rust
pub trait ByteTransport {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), TransportError>;
    fn read(&mut self, buffer: &mut [u8]) -> Result<usize, TransportError>;
}
```

## 17.5 Future virtual HID output

Keep a placeholder for the restricted macOS virtual-HID backend.

Do not make it a default feature.

Do not require restricted entitlements to build or test the workspace.

---

# 18. Gamepad simulator

Create:

```text
gamepad-simulator
```

This must generate `GamepadState` without a Steam Controller.

Modes:

```bash
gamepad-simulator keyboard
gamepad-simulator automated
gamepad-simulator replay path/to/file
```

Suggested keyboard mapping:

```text
W/A/S/D       Left stick
Arrow keys    Right stick
Q/E           Left/right trigger
I/J/K/L       D-pad
Space         South button
1–9           Other buttons
R             Reset to neutral
Escape        Exit
```

Automated mode must:

1. Press every button.
2. Rotate both sticks.
3. Sweep both triggers.
4. Exercise all D-pad directions.
5. Return to neutral.
6. Repeat.

The simulator must support all output backends.

This will become the first host-side application used with the XIAO firmware.

---

# 19. Visualizer

Create `sc-visualizer`.

Use `egui` unless there is a strong reason to use another GUI library.

Display:

- device connection status;
- report frequency;
- raw report size and ID;
- decoded buttons;
- left stick;
- both trackpads;
- triggers;
- D-pad;
- grip buttons;
- gyro values when available;
- outgoing generic gamepad state;
- serial connection status;
- sequence number;
- packets sent;
- checksum or framing failures.

Useful controls:

- start/stop recording;
- insert recording marker;
- select mapping profile;
- adjust dead zones;
- invert axes;
- enable smoothing;
- select output backend;
- reset to neutral;
- show raw report bytes.

Do not make the visualizer a required dependency of the bridge core.

The command-line tools and libraries must work without a GUI.

---

# 20. Main bridge application

Create:

```text
sc-bridge
```

Responsibilities:

- select Steam Controller;
- initialize it;
- receive HID reports;
- decode state;
- normalize state;
- apply mapping;
- apply filters;
- send only changed states where appropriate;
- maintain serial connection;
- send neutral state on disconnect;
- recover after controller reconnect;
- recover after XIAO reconnect;
- expose useful metrics and logs.

Suggested usage:

```bash
sc-bridge \
  --controller auto \
  --output serial \
  --port /dev/cu.usbmodemXXXX \
  --profile default
```

Other modes:

```bash
sc-bridge --output dump
sc-bridge --record session.jsonl
sc-bridge --input replay --file session.jsonl
```

---

# 21. Concurrency model

Use a simple and observable architecture.

Suggested tasks:

```text
HID input task
    │
    ▼
bounded channel
    │
    ▼
decode/map task
    │
    ▼
latest-state channel
    │
    ▼
output task
```

Requirements:

- bounded channels;
- no unbounded packet accumulation;
- latest-state behavior for controller updates;
- preserve lifecycle events;
- disconnect immediately emits neutral state;
- clear shutdown coordination;
- Ctrl+C handling;
- no detached tasks left running.

Avoid introducing a large async runtime unless it simplifies HID and serial integration significantly.

A threaded implementation is acceptable.

---

# 22. Logging and diagnostics

Use structured logging.

Recommended levels:

```text
error
warn
info
debug
trace
```

Log:

- controller discovery;
- device metadata;
- report rate;
- initialization commands;
- dropped packets;
- malformed reports;
- reconnect attempts;
- mapping profile;
- serial connection state;
- protocol version;
- checksum failures;
- output packet rate;
- shutdown reason.

Do not log every input report at `info`.

Raw report logging must be opt-in.

---

# 23. Metrics

Track:

- input reports received;
- input reports per second;
- decode failures;
- state changes;
- output packets sent;
- output packets skipped because unchanged;
- serial reconnects;
- HID reconnects;
- malformed serial frames;
- checksum failures;
- average decode time;
- average mapping time;
- average end-to-end host processing time.

Metrics may initially be printed periodically.

Do not add a heavy telemetry framework.

---

# 24. Safety and failure behavior

The system must always prefer neutral output after uncertainty.

Send a neutral state when:

- the Steam Controller disconnects;
- report decoding repeatedly fails;
- the input stream times out;
- the bridge exits normally;
- the user presses reset;
- the selected profile becomes invalid.

Use a configurable input timeout, for example:

```text
100–250 ms
```

Do not leave triggers, sticks, or buttons active after input stops.

---

# 25. Unit tests

Add tests for:

## Gamepad state

- neutral state;
- button set and clear;
- axis clamping;
- trigger clamping;
- NaN handling;
- hat conversion.

## Protocol

- serialization;
- deserialization;
- checksums;
- malformed data recovery;
- partial reads;
- multiple frames;
- version mismatch;
- maximum payload enforcement.

## Steam Controller decoder

- known button reports;
- stick endpoints;
- trigger endpoints;
- trackpad touch;
- malformed report sizes;
- unknown report IDs;
- captured report regression tests.

## Mapper

- every button mapping;
- trigger normalization;
- stick normalization;
- right-trackpad mapping;
- touch release returning to center;
- axis inversion;
- dead zones;
- smoothing.

## Recorder and replay

- event round trip;
- timestamps remain ordered;
- unknown event handling;
- deterministic replay;
- truncated recording handling.

## Outputs

- only changed states sent where configured;
- neutral state on shutdown;
- serial reconnect;
- handshake success;
- handshake version rejection.

---

# 26. Property and fuzz testing

Use property-based testing where valuable.

Candidates:

- arbitrary byte streams into frame decoder;
- arbitrary HID reports into Steam decoder;
- arbitrary floating-point state into packet conversion;
- serialization followed by deserialization;
- decoder never panics;
- mapped state never contains non-finite values;
- all output values remain in range.

Add fuzz targets if the repository setup remains manageable.

Priority fuzz targets:

1. Bridge frame decoder
2. Steam HID report decoder
3. Recording parser

---

# 27. Performance targets

Host-side targets:

- less than 1 ms average processing per controller report;
- no avoidable allocation in the steady-state decode/map/output path;
- bounded memory usage;
- no artificial input queue;
- negligible CPU use while disconnected;
- automatic reconnect without process restart;
- neutral output within 100 ms of confirmed controller disconnect;
- no report-rate reduction below the source controller rate.

Do not optimize prematurely, but record basic timings.

---

# 28. Documentation

## `README.md`

Include:

- project purpose;
- current status;
- architecture diagram;
- build instructions;
- command examples;
- known limitations;
- Apple virtual-HID entitlement limitation;
- planned XIAO nRF52840 support.

## `docs/ARCHITECTURE.md`

Document:

- crate boundaries;
- pipeline;
- concurrency;
- lifecycle;
- error handling;
- disconnect behavior.

## `docs/GAMEPAD_PROTOCOL.md`

Document:

- message framing;
- endianness;
- checksums;
- message types;
- gamepad payload;
- protocol versions;
- recovery behavior;
- static test vectors.

## `docs/STEAM_CONTROLLER_PROTOCOL.md`

Document:

- discovered devices;
- transport differences;
- HID collections;
- report IDs;
- packet layouts;
- feature reports;
- lizard-mode behavior;
- heartbeat;
- unknown fields;
- information sources;
- captured examples.

Clearly distinguish:

```text
Confirmed
Inferred
Unknown
```

## `docs/RECORDING_FORMAT.md`

Document:

- event schema;
- versioning;
- timestamps;
- binary data encoding;
- compatibility.

## `docs/FIRMWARE_PLAN.md`

Define the future XIAO firmware:

- USB composite device;
- CDC interface;
- HID gamepad interface;
- bridge protocol parser;
- watchdog;
- neutral state after timeout;
- protocol hello;
- LED diagnostics;
- firmware test vectors.

Do not implement the firmware yet.

---

# 29. Continuous integration

Add GitHub Actions jobs for:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace --all-targets
```

Consider separate platform jobs:

- macOS for HID-specific crates;
- Linux for platform-neutral crates.

Use feature flags or conditional compilation so non-macOS CI can test:

- gamepad state;
- bridge protocol;
- mapper;
- recording;
- mocks;
- replay.

Do not require physical hardware in CI.

---

# 30. Suggested implementation order

## Phase 1 — Foundation

1. Create workspace.
2. Add `gamepad-state`.
3. Add `bridge-protocol`.
4. Implement serialization and framing.
5. Add tests and protocol documentation.

Exit criterion:

> Arbitrary `GamepadState` values can be converted into stable, validated protocol frames and decoded again.

## Phase 2 — Simulation and outputs

1. Add output traits.
2. Implement mock output.
3. Implement dump output.
4. Implement file output.
5. Implement gamepad simulator.
6. Add automated test sequence.

Exit criterion:

> A simulated controller can produce valid framed packets without hardware.

## Phase 3 — Recording and replay

1. Define recording format.
2. Implement recorder library.
3. Implement replay library.
4. Add `sc-replay`.
5. Add deterministic replay tests.

Exit criterion:

> A simulated session can be recorded and replayed identically.

## Phase 4 — Steam Controller probing

1. Enumerate HID devices.
2. Add `sc-probe`.
3. Capture raw reports.
4. Record metadata.
5. Test direct USB and receiver where available.

Exit criterion:

> Physical inputs produce timestamped raw report captures.

## Phase 5 — Protocol decoding

1. Implement known report parser.
2. Decode buttons.
3. Decode sticks.
4. Decode triggers.
5. Decode trackpads.
6. Add captured-report tests.
7. Document unknown fields.

Exit criterion:

> Basic Steam Controller inputs produce a stable decoded source state.

## Phase 6 — Mapping

1. Implement default mapping.
2. Add dead zones.
3. Add trigger normalization.
4. Add right-trackpad-to-right-stick conversion.
5. Add axis inversion.
6. Add optional smoothing.
7. Add tests.

Exit criterion:

> Decoded Steam Controller reports produce correct conventional `GamepadState` values.

## Phase 7 — Visualizer

1. Add live source-state visualization.
2. Show mapped state.
3. Add recording controls.
4. Add output backend selection.
5. Display diagnostics.

Exit criterion:

> Every decoded input and outgoing gamepad value can be inspected visually.

## Phase 8 — Serial transport

1. Implement serial backend.
2. Add frame handshake.
3. Add sequence numbers.
4. Add reconnect.
5. Add mock serial tests.
6. Support simulator-to-serial output.

Exit criterion:

> The host is ready to communicate with future XIAO firmware without architectural changes.

## Phase 9 — Integrated bridge

1. Combine input, decoding, mapping, and output.
2. Add shutdown handling.
3. Add neutral-on-disconnect.
4. Add metrics.
5. Add recording and replay modes.

Exit criterion:

> The entire host pipeline works using dump, file, mock, and serial outputs.

---

# 31. Hardware-arrival readiness criteria

Before the XIAO arrives, the project should be able to:

- generate generic gamepad state from keyboard input;
- generate an automated test sequence;
- serialize gamepad state into versioned binary frames;
- write frames to a mock or file transport;
- connect to an arbitrary serial port;
- perform a protocol handshake;
- reconnect automatically;
- record and replay controller sessions;
- capture Steam Controller HID reports;
- decode basic Steam Controller input;
- map decoded state into generic gamepad state;
- visualize both source and output state;
- provide firmware test vectors;
- document the intended USB composite firmware.

When the board arrives, only the following should remain:

1. Set up the nRF52840 firmware project.
2. Configure USB CDC and USB HID.
3. Parse the existing bridge protocol.
4. Convert packets into HID reports.
5. Add timeout-to-neutral behavior.
6. Test with macOS and browsers.

---

# 32. Quality requirements

The coding agent must:

- avoid placeholder implementations that silently succeed;
- avoid panics on external input;
- avoid undocumented magic values;
- document all packet layouts;
- keep macOS-specific code isolated;
- preserve platform-neutral tests;
- use bounded queues;
- make disconnect behavior explicit;
- use compile-time checks for packet sizes where practical;
- keep unsafe code minimal and documented;
- run formatting, linting, and tests after each major phase;
- update documentation as protocol discoveries are made.

When the implementation encounters uncertain Steam Controller fields, preserve the raw bytes and mark the field as unknown rather than guessing.

---

# 33. Initial coding task

Start with the following task:

> Create the Rust workspace and implement `gamepad-state`, `bridge-protocol`, `bridge-output`, and `gamepad-simulator`. Define a fixed-size generic gamepad state, a versioned framed binary transport protocol with checksum and sequence numbers, mock/dump/file outputs, and automated and keyboard-driven simulation modes. Add exhaustive unit tests, static protocol test vectors, and initial protocol documentation. Do not implement hardware access or firmware yet.

After completing that task, display:

- created files;
- architecture decisions;
- protocol layout;
- test results;
- remaining work for the next phase.

This can also be narrowed into a first implementation prompt limited to the workspace, packet protocol, and simulator.
