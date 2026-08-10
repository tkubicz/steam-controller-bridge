# Third-party notices

Steam Controller Bridge is distributed under the [MIT License](LICENSE). It links
third-party crates whose own terms continue to apply. Most are MIT or Apache-2.0
and need no separate notice; the dependencies below carry obligations worth
recording explicitly.

The complete license set is enforced in CI by [`cargo-deny`](deny.toml). To
regenerate the underlying data:

```bash
cargo deny check licenses
cargo metadata --format-version 1 --filter-platform aarch64-apple-darwin
```

## Embedded fonts — `sc-visualizer` and bindings editor

`epaint_default_fonts`, reached through `eframe` → `egui` → `epaint`, embeds font
binaries directly into the `sc-visualizer` and `sc-bridge-menu` executables. The crate as a whole is
`(MIT OR Apache-2.0) AND OFL-1.1 AND Ubuntu-font-1.0`.

| Font | License |
| --- | --- |
| Ubuntu-Light | Ubuntu Font Licence 1.0 |
| Hack-Regular | Hack Open Font License / Bitstream Vera |
| NotoEmoji-Regular | SIL Open Font License 1.1 |
| emoji-icon-font | MIT |

Full texts ship inside the crate as `fonts/UFL.txt`, `fonts/Hack-Regular.txt`,
`fonts/OFL.txt`, and `fonts/emoji-icon-font-mit-license.txt`. Source:
<https://github.com/emilk/egui/tree/main/crates/epaint_default_fonts>

Neither the SIL OFL nor the Ubuntu Font Licence restricts distributing a program
that embeds the fonts. Both require that their notices travel with the binary,
which is the purpose of this file. The menu binary uses these fonts only in its
`bindings-editor` window; `sc-bridge` embeds no fonts.

## `enigo`

`enigo` provides the replaceable macOS keyboard/mouse injection adapter used by
opt-in desktop bindings. It is used unmodified under the MIT license. Source:
<https://github.com/enigo-rs/enigo>

## `serialport` — Mozilla Public License 2.0

`serialport` provides the host side of the USB CDC link and is linked into every
binary in this workspace.

- Source: <https://github.com/serialport/serialport-rs>
- License: MPL-2.0 — <https://www.mozilla.org/MPL/2.0/>

MPL-2.0 is file-scoped copyleft. Combining it with MIT-licensed code in a larger
work is permitted and does not change this project's license. The obligation
attaches only to MPL-covered files: anyone who modifies them and distributes the
result must make those modified files available under MPL-2.0. This project uses
`serialport` unmodified, as a published crates.io dependency.

## OpenPuck

The XIAO firmware's nonblocking TinyUSB and watchdog structure was informed by
OpenPuck, but **no OpenPuck source or USB descriptors are included**. OpenPuck is
AGPL-3.0; this project is MIT and deliberately shares no code with it. See
[`firmware/xiao-nrf52840/README.md`](firmware/xiao-nrf52840/README.md).

## USB identity

The firmware enumerates with the Xbox 360 compatibility VID/PID `045e:028e`,
which belongs to Microsoft. This is a deliberate compatibility choice — Apple's
built-in driver will not publish a generic HID gamepad to GameController — and is
not a claim of ownership or affiliation. A distributable product needs an owned
or licensed USB identity. See the README's compatibility note.

`Steam Controller`, `Steam`, and `Valve` are trademarks of Valve Corporation.
`Xbox` is a trademark of Microsoft Corporation. This project is not affiliated
with, endorsed by, or sponsored by either company.
