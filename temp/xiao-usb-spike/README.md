# XIAO Linux direct-USB spike

This disposable program is the Phase 0 gate for the cross-platform USB transport plan. It uses the currently flashed XIAO firmware, claims only the descriptor-discovered CDC function, and reuses `bridge-output::SerialConnection<T>` for the bridge protocol. It is outside the production workspace and must not be merged as a product crate.

Do not start the production transport PRs until the required observations below pass.

## Prepare Ubuntu

Install build and test tools:

```bash
sudo apt install build-essential pkg-config libudev-dev usbutils evtest strace acl
```

Detach the forwarded XIAO. Remove the earlier diagnostic `cdc_acm` dynamic ID by reloading the module, then forward the XIAO again:

```bash
sudo modprobe -r cdc_acm
sudo modprobe cdc_acm
```

Install the temporary exact-match raw-USB access rule:

```bash
sudo install -m 0644 60-steam-controller-bridge-spike.rules \
  /etc/udev/rules.d/60-steam-controller-bridge-spike.rules
sudo udevadm control --reload-rules
```

Detach and reattach the XIAO after installing the rule. Confirm that the CDC interfaces have no driver, the Xbox interface uses `xpad`, and no tty exists:

```bash
usb-devices | sed -n '/Vendor=045e ProdID=028e/,/^$/p'
ls -l /dev/ttyACM* 2>/dev/null || true
```

Use the bus and device numbers from `lsusb -d 045e:028e` to verify the active-session ACL, replacing `BBB` and `DDD` with zero-padded values:

```bash
udevadm info --query=property --name=/dev/bus/usb/BBB/DDD | grep uaccess
getfacl /dev/bus/usb/BBB/DDD
test -r /dev/bus/usb/BBB/DDD && test -w /dev/bus/usb/BBB/DDD
```

Build the spike as your normal desktop user:

```bash
cargo test
cargo build --release
```

Never run the spike with `sudo`. Normal-user access through the exact ACL rule is part of the gate.

## 1. Direct USB and automated input

Start `evtest` for the Xbox controller in another terminal, then run:

```bash
./target/release/xiao-usb-spike smoke
```

The command must report all of the following:

- the masked stable serial and descriptor-selected interface and endpoint numbers;
- continued `xpad` ownership of the Xbox interface;
- a successful `USBDEVFS_DROP_PRIVILEGES` mask;
- an Xbox-interface claim rejected specifically with `EACCES`;
- DTR low followed by DTR high;
- a completed protocol-v1 Hello;
- a reported firmware revision for target `seeed-xiao-nrf52840`;
- neutral output and DTR clear at shutdown.

Retain the final protocol metrics and inbound sequence-gap count. Successful runs require zero framing failures, zero checksum failures, zero dropped states, and zero inbound sequence gaps.

Verify buttons, hats, triggers, and both sticks in `evtest`.

Trace one run and retain the output. The source does not expose configuration-setting, reset, or driver-detach operations; the trace must agree:

```bash
strace -f -e trace=ioctl -o /tmp/xiao-usb-spike-ioctl.log \
  ./target/release/xiao-usb-spike smoke
grep -E 'USBDEVFS_(SETCONFIGURATION|RESET|DISCONNECT)' \
  /tmp/xiao-usb-spike-ioctl.log
```

The final `grep` must produce no output.

Repeat DTR intervals of 1, 5, 10, and 25 ms. Record the lowest interval that passes ten consecutive runs. Production must use a measured value with a safety margin.

```bash
for interval in 1 5 10 25; do
  for run in $(seq 1 10); do
    ./target/release/xiao-usb-spike smoke --dtr-low-ms "$interval" || break
  done
done
```

`--duration-secs` controls how long `smoke` and `replay` wait for the firmware report. It controls total run time for the long-running commands.

## 2. Recorded Puck replay

Use a JSONL recording containing `mapped_gamepad_state` events from the real Puck capture:

```bash
./target/release/xiao-usb-spike replay --recording /path/to/puck-recording.jsonl
```

The command must send at least one mapped state, finish neutral, and show the recorded controls in `evtest`.

## 3. Refresh, crash, and stale-session recovery

First confirm that an unchanged active state is refreshed without watchdog neutralization for at least 30 seconds:

```bash
./target/release/xiao-usb-spike hold --duration-secs 30
```

For crash recovery, run a longer hold and send `SIGKILL` from another terminal using the printed PID:

```bash
./target/release/xiao-usb-spike hold
kill -9 PID
./target/release/xiao-usb-spike smoke
```

Repeat ten times. `evtest` must show watchdog neutralization after each kill, and the immediately restarted smoke test must complete a fresh Hello.

Leave an incomplete frame in the firmware decoder and kill the process before its timer expires:

```bash
./target/release/xiao-usb-spike poison
kill -9 PID
./target/release/xiao-usb-spike smoke
```

The next Hello must complete without stale framing or response state. Ctrl-C and SIGTERM are intentionally clean paths: long-running commands send neutral where applicable and clear DTR. Use only `kill -9` for the crash cases.

## 4. Replug without restarting the host process

Run:

```bash
./target/release/xiao-usb-spike reconnect --duration-secs 300
```

While it stays open, detach the XIAO from USB-over-IP and attach it again. The command must rediscover the same stable serial, force a fresh DTR edge, complete Hello and firmware reporting again, and resume the active test state. Repeat several times, then press Ctrl-C for a clean stop.

## 5. Force feedback

Run:

```bash
./target/release/xiao-usb-spike feedback --duration-secs 60
```

Send one strong-only and one weak-only rumble effect through the `xpad` event device. The command succeeds only after it receives both an isolated nonzero low-frequency response and an isolated nonzero high-frequency response. Zero-valued negotiation or stop responses do not pass this gate.

## 6. Idle CPU and USB power

Run an idle session:

```bash
/usr/bin/time -v ./target/release/xiao-usb-spike idle --duration-secs 60
```

During the run, locate the exact sysfs device by its manufacturer, product, and serial attributes. Record `power/control` and `power/runtime_status` during the run and after it exits. Also retain user CPU time, system CPU time, CPU percentage, and maximum RSS from `/usr/bin/time`.

This is an observation gate. Bare-metal suspend and resume remain separate product acceptance work.

## 7. Steam coexistence

Start Steam with its normal controller configuration and repeat `smoke`, `replay`, `feedback`, one `hold` crash/restart, and the `reconnect` replug test. Throughout the run:

- `xpad` must stay attached;
- only one Xbox controller may appear;
- no stale input may remain;
- every direct-USB command must still pass.

## 8. Existing tty preference and deduplication

Install the repository's existing tty access rule, register the diagnostic `cdc_acm` ID, and reconnect the XIAO:

```bash
sudo install -m 0644 ../../packaging/linux/60-steam-controller-bridge.rules \
  /etc/udev/rules.d/60-steam-controller-bridge.rules
sudo udevadm control --reload-rules
printf '045e 028e 02\n' | sudo tee /sys/bus/usb/drivers/cdc_acm/new_id
```

Confirm `/dev/ttyACM*` exists, then run as the normal user:

```bash
./target/release/xiao-usb-spike dedupe
```

It must match raw USB and tty by the full stable serial, report one logical bridge, prefer the tty endpoint, and complete Hello and firmware reporting through the tty.

Detach the XIAO, reload `cdc_acm` to remove the dynamic ID, and reattach it before the final direct-USB and reboot checks:

```bash
sudo modprobe -r cdc_acm
sudo modprobe cdc_acm
```

Repeat `smoke` after a VM reboot with no `new_id` registration or binding helper installed.

## Cleanup

```bash
sudo rm /etc/udev/rules.d/60-steam-controller-bridge-spike.rules
sudo udevadm control --reload-rules
```

Detach and reattach the XIAO after removing the spike rule. The existing repository device-access rule may remain installed.
