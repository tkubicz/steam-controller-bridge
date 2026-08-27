# Crate groups

Package names stay independent from their filesystem group.

| Directory | Responsibility |
| --- | --- |
| `bridge/` | Gamepad state, bridge protocol and output, core translation, and live orchestration. |
| `controller/` | Steam Controller protocol, device access, discovery, and mapping. |
| `host/` | Operating-system facades and native host integration. |
| `app/` | Reusable product features, updater logic, recording, and presentation support. |

Platform-facing crates belong to the responsibility they serve. They are not
grouped into operating-system-specific directories.
