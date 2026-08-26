#![cfg_attr(not(any(test, target_os = "linux")), allow(dead_code, unused_imports))]

use std::error::Error;
use std::io;
use std::path::{Path, PathBuf};

use nusb::descriptors::{ConfigurationDescriptor, TransferType};
use nusb::transfer::Direction;

type SpikeResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

const USB_CLASS_COMMUNICATIONS: u8 = 0x02;
const USB_CLASS_CDC_DATA: u8 = 0x0a;
const USB_CLASS_VENDOR: u8 = 0xff;
const CDC_SUBCLASS_ACM: u8 = 0x02;
const XINPUT_SUBCLASS: u8 = 0x5d;
const XINPUT_PROTOCOL: u8 = 0x01;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Topology {
    control_interface: u8,
    data_interface: u8,
    xinput_interface: u8,
    bulk_in: u8,
    bulk_out: u8,
}

#[derive(Debug, Default)]
struct FeedbackObservation {
    responses: u64,
    low_only: bool,
    high_only: bool,
}

impl FeedbackObservation {
    fn observe(&mut self, low_frequency: u16, high_frequency: u16) {
        self.responses += 1;
        self.low_only |= low_frequency > 0 && high_frequency == 0;
        self.high_only |= high_frequency > 0 && low_frequency == 0;
    }
}

fn masked_serial(serial: Option<&str>) -> String {
    let Some(serial) = serial.filter(|value| !value.is_empty()) else {
        return "<none>".to_owned();
    };
    if serial.chars().count() <= 4 {
        return "****".to_owned();
    }
    let suffix: String = serial
        .chars()
        .skip(serial.chars().count().saturating_sub(4))
        .collect();
    format!("****{suffix}")
}

fn usb_interface_sysfs_path(
    device_path: &Path,
    configuration: &str,
    interface: u8,
) -> SpikeResult<PathBuf> {
    let device_name = device_path
        .file_name()
        .ok_or_else(|| spike_error("USB sysfs device path has no file name"))?
        .to_string_lossy();
    Ok(device_path.join(format!("{device_name}:{configuration}.{interface}")))
}

fn discover_topology(configuration: ConfigurationDescriptor<'_>) -> SpikeResult<Topology> {
    let interfaces = configuration
        .interface_alt_settings()
        .filter(|descriptor| descriptor.alternate_setting() == 0)
        .collect::<Vec<_>>();

    let controls = interfaces
        .iter()
        .filter(|descriptor| {
            descriptor.class() == USB_CLASS_COMMUNICATIONS
                && descriptor.subclass() == CDC_SUBCLASS_ACM
                && descriptor.protocol() == 0
        })
        .collect::<Vec<_>>();
    let control = only(controls, "CDC ACM control interface")?;
    let control_number = control.interface_number();

    let union = only(
        control
            .descriptors()
            .filter(|descriptor| {
                descriptor.descriptor_type() == 0x24
                    && descriptor.len() >= 3
                    && descriptor[2] == 0x06
            })
            .collect::<Vec<_>>(),
        "CDC union descriptor",
    )?;
    if union.len() != 5 || union[3] != control_number {
        return Err(spike_error(
            "CDC union descriptor is malformed or its master does not match the control interface",
        ));
    }
    let data_number = union[4];

    let header = only(
        control
            .descriptors()
            .filter(|descriptor| {
                descriptor.descriptor_type() == 0x24
                    && descriptor.len() >= 3
                    && descriptor[2] == 0x00
            })
            .collect::<Vec<_>>(),
        "CDC header descriptor",
    )?;
    if header.len() != 5 || u16::from_le_bytes([header[3], header[4]]) < 0x0110 {
        return Err(spike_error(
            "CDC header descriptor is malformed or predates CDC 1.10",
        ));
    }
    let call_management = only(
        control
            .descriptors()
            .filter(|descriptor| {
                descriptor.descriptor_type() == 0x24
                    && descriptor.len() >= 3
                    && descriptor[2] == 0x01
            })
            .collect::<Vec<_>>(),
        "CDC call-management descriptor",
    )?;
    if call_management.len() != 5 || call_management[4] != data_number {
        return Err(spike_error(
            "CDC call-management descriptor does not reference the data interface",
        ));
    }
    let acm = only(
        control
            .descriptors()
            .filter(|descriptor| {
                descriptor.descriptor_type() == 0x24
                    && descriptor.len() >= 3
                    && descriptor[2] == 0x02
            })
            .collect::<Vec<_>>(),
        "CDC ACM descriptor",
    )?;
    if acm.len() != 4 || acm[3] & 0x02 == 0 {
        return Err(spike_error(
            "CDC ACM descriptor does not support line coding and control-line state",
        ));
    }
    let control_notifications = control
        .endpoints()
        .filter(|endpoint| {
            endpoint.transfer_type() == TransferType::Interrupt
                && endpoint.direction() == Direction::In
        })
        .count();
    if control.num_endpoints() != 1 || control_notifications != 1 {
        return Err(spike_error(
            "CDC control interface must contain exactly one interrupt IN endpoint",
        ));
    }

    let data_candidates = interfaces
        .iter()
        .filter(|descriptor| {
            descriptor.interface_number() == data_number
                && descriptor.class() == USB_CLASS_CDC_DATA
                && descriptor.subclass() == 0
                && descriptor.protocol() == 0
        })
        .collect::<Vec<_>>();
    let data = only(
        data_candidates,
        "CDC data interface referenced by the union",
    )?;
    let bulk_endpoints = data
        .endpoints()
        .filter(|endpoint| endpoint.transfer_type() == TransferType::Bulk)
        .collect::<Vec<_>>();
    if bulk_endpoints.len() != 2 || data.num_endpoints() != 2 {
        return Err(spike_error(
            "CDC data interface must contain exactly two bulk endpoints",
        ));
    }
    let bulk_in = only(
        bulk_endpoints
            .iter()
            .filter(|endpoint| endpoint.direction() == Direction::In)
            .collect::<Vec<_>>(),
        "CDC bulk IN endpoint",
    )?
    .address();
    let bulk_out = only(
        bulk_endpoints
            .iter()
            .filter(|endpoint| endpoint.direction() == Direction::Out)
            .collect::<Vec<_>>(),
        "CDC bulk OUT endpoint",
    )?
    .address();

    let xinput_candidates = interfaces
        .iter()
        .filter(|descriptor| {
            descriptor.class() == USB_CLASS_VENDOR
                && descriptor.subclass() == XINPUT_SUBCLASS
                && descriptor.protocol() == XINPUT_PROTOCOL
        })
        .collect::<Vec<_>>();
    let xinput = only(xinput_candidates, "Xbox interface")?;
    let xinput_endpoints = xinput
        .endpoints()
        .filter(|endpoint| endpoint.transfer_type() == TransferType::Interrupt)
        .collect::<Vec<_>>();
    if xinput.num_endpoints() != 2 || xinput_endpoints.len() != 2 {
        return Err(spike_error(
            "Xbox interface must contain exactly two interrupt endpoints",
        ));
    }
    only(
        xinput_endpoints
            .iter()
            .filter(|endpoint| endpoint.direction() == Direction::In)
            .collect::<Vec<_>>(),
        "Xbox interrupt IN endpoint",
    )?;
    only(
        xinput_endpoints
            .iter()
            .filter(|endpoint| endpoint.direction() == Direction::Out)
            .collect::<Vec<_>>(),
        "Xbox interrupt OUT endpoint",
    )?;

    Ok(Topology {
        control_interface: control_number,
        data_interface: data_number,
        xinput_interface: xinput.interface_number(),
        bulk_in,
        bulk_out,
    })
}

fn only<T>(mut values: Vec<T>, name: &str) -> SpikeResult<T> {
    if values.len() != 1 {
        return Err(spike_error(format!(
            "expected exactly one {name}, found {}",
            values.len()
        )));
    }
    Ok(values.remove(0))
}

fn spike_error(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    io::Error::other(message.into()).into()
}

#[cfg(target_os = "linux")]
mod linux {
    use std::fs::{self, File};
    use std::io::{self, BufReader};
    use std::os::fd::OwnedFd;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    use bridge_output::{
        available_serial_devices, ByteTransport, FirmwareInfo, FirmwareTarget, FirmwareTargetId,
        FirmwareVersion, GamepadOutput, OutputError, OutputFeedback, SerialConfig,
        SerialConnection, SerialOutput, SerialStatus, BRIDGE_DEVICE_USB_PRODUCT,
    };
    use bridge_protocol::StreamDecoder;
    use gamepad_simulator::automated_sequence;
    use gamepad_state::{Button, GamepadState};
    use linux_raw_sys::{errno::EACCES, ioctl::USBDEVFS_DROP_PRIVILEGES};
    use nusb::transfer::{Buffer, Bulk, ControlOut, ControlType, In, Out, Recipient};
    use nusb::{DeviceInfo, Endpoint, Interface, MaybeFuture};
    use recording::{ReplayOptions, ReplaySession};
    use rustix::fs::{Mode, OFlags};

    use super::{
        discover_topology, masked_serial, spike_error, usb_interface_sysfs_path,
        FeedbackObservation, SpikeResult, Topology,
    };

    const VENDOR_ID: u16 = 0x045e;
    const PRODUCT_ID: u16 = 0x028e;
    const MANUFACTURER: &str = "Lynxware";
    const FIRMWARE_TARGET_ID: &str = "seeed-xiao-nrf52840";
    const CONTROL_TIMEOUT: Duration = Duration::from_millis(250);
    const BULK_OUT_TIMEOUT: Duration = Duration::from_millis(50);
    const ACTIVE_SERVICE_INTERVAL: Duration = Duration::from_millis(5);
    const IDLE_SERVICE_INTERVAL: Duration = Duration::from_millis(20);
    static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Command {
        Smoke,
        Hold,
        Idle,
        Feedback,
        Poison,
        Replay,
        Reconnect,
        Dedupe,
    }

    #[derive(Debug)]
    struct Options {
        command: Command,
        dtr_low: Duration,
        duration: Duration,
        drop_privileges: bool,
        recording: Option<PathBuf>,
    }

    pub fn main() -> SpikeResult<()> {
        STOP_REQUESTED.store(false, Ordering::Release);
        ctrlc::set_handler(|| STOP_REQUESTED.store(true, Ordering::Release))?;
        let options = parse_options()?;
        match options.command {
            Command::Smoke => smoke(options),
            Command::Hold => hold(options),
            Command::Idle => idle(options),
            Command::Feedback => feedback(options),
            Command::Poison => poison(options),
            Command::Replay => replay(options),
            Command::Reconnect => reconnect(options),
            Command::Dedupe => dedupe(),
        }
    }

    fn parse_options() -> SpikeResult<Options> {
        let mut arguments = std::env::args().skip(1);
        let command = match arguments.next().as_deref() {
            None | Some("smoke") => Command::Smoke,
            Some("hold") => Command::Hold,
            Some("idle") => Command::Idle,
            Some("feedback") => Command::Feedback,
            Some("poison") => Command::Poison,
            Some("replay") => Command::Replay,
            Some("reconnect") => Command::Reconnect,
            Some("dedupe") => Command::Dedupe,
            Some(other) => return Err(spike_error(format!("unknown command '{other}'"))),
        };
        let mut dtr_low = Duration::from_millis(25);
        let mut duration = match command {
            Command::Smoke => Duration::from_secs(3),
            Command::Hold | Command::Poison | Command::Reconnect => Duration::from_secs(300),
            Command::Idle | Command::Feedback => Duration::from_secs(30),
            Command::Replay => Duration::from_secs(3),
            Command::Dedupe => Duration::from_secs(1),
        };
        let mut drop_privileges = true;
        let mut recording = None;
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--dtr-low-ms" => {
                    dtr_low = Duration::from_millis(parse_u64(arguments.next(), "--dtr-low-ms")?);
                }
                "--duration-secs" => {
                    duration = Duration::from_secs(parse_u64(arguments.next(), "--duration-secs")?);
                }
                "--no-drop-privileges" => drop_privileges = false,
                "--recording" => {
                    recording = Some(PathBuf::from(
                        arguments
                            .next()
                            .ok_or_else(|| spike_error("--recording requires a path"))?,
                    ));
                }
                other => return Err(spike_error(format!("unknown option '{other}'"))),
            }
        }
        if dtr_low.is_zero() {
            return Err(spike_error("--dtr-low-ms must be greater than zero"));
        }
        if duration.is_zero() {
            return Err(spike_error("--duration-secs must be greater than zero"));
        }
        if command == Command::Replay && recording.is_none() {
            return Err(spike_error("replay requires --recording PATH"));
        }
        if command != Command::Replay && recording.is_some() {
            return Err(spike_error("--recording is only valid with replay"));
        }
        Ok(Options {
            command,
            dtr_low,
            duration,
            drop_privileges,
            recording,
        })
    }

    fn parse_u64(value: Option<String>, option: &str) -> SpikeResult<u64> {
        value
            .ok_or_else(|| spike_error(format!("{option} requires a value")))?
            .parse()
            .map_err(|_| spike_error(format!("{option} requires a non-negative integer")))
    }

    fn smoke(options: Options) -> SpikeResult<()> {
        let (mut connection, clock) = connect(&options)?;
        let firmware = wait_for_official_firmware(&mut connection, clock, options.duration)?;
        println!("firmware: {firmware:?}");
        let mut interrupted = false;
        for state in automated_sequence(8) {
            connection.queue_state(state)?;
            let report = service_for(
                &mut connection,
                clock,
                Duration::from_millis(50),
                ACTIVE_SERVICE_INTERVAL,
                true,
            )?;
            if report.interrupted {
                interrupted = true;
                break;
            }
        }
        close(connection, clock)?;
        if interrupted {
            return Err(spike_error("smoke test interrupted after clean shutdown"));
        }
        println!("direct-USB automated smoke test passed");
        Ok(())
    }

    fn hold(options: Options) -> SpikeResult<()> {
        let (mut connection, clock) = connect(&options)?;
        let mut state = GamepadState {
            left_x: 1.0,
            right_trigger: 1.0,
            ..GamepadState::neutral()
        };
        state.buttons.set(Button::South, true);
        connection.queue_state(state)?;
        println!(
            "holding a non-neutral state for {} seconds; use kill -9 {} for the crash test",
            options.duration.as_secs(),
            std::process::id()
        );
        let report = service_for(
            &mut connection,
            clock,
            options.duration,
            ACTIVE_SERVICE_INTERVAL,
            true,
        )?;
        close(connection, clock)?;
        if report.interrupted {
            println!("hold interrupted after neutral state and DTR clear");
        }
        Ok(())
    }

    fn idle(options: Options) -> SpikeResult<()> {
        let (mut connection, clock) = connect(&options)?;
        println!(
            "servicing an idle connection for {} seconds",
            options.duration.as_secs()
        );
        let report = service_for(
            &mut connection,
            clock,
            options.duration,
            IDLE_SERVICE_INTERVAL,
            true,
        )?;
        close(connection, clock)?;
        if report.interrupted {
            println!("idle test interrupted after neutral state and DTR clear");
        }
        Ok(())
    }

    fn feedback(options: Options) -> SpikeResult<()> {
        let (mut connection, clock) = connect(&options)?;
        connection.queue_state(GamepadState::neutral())?;
        println!(
            "waiting {} seconds for Xbox force feedback; trigger distinct low/high effects now",
            options.duration.as_secs()
        );
        let report = service_for(
            &mut connection,
            clock,
            options.duration,
            ACTIVE_SERVICE_INTERVAL,
            true,
        )?;
        close(connection, clock)?;
        if !report.feedback.low_only || !report.feedback.high_only {
            return Err(spike_error(format!(
                "expected isolated nonzero low- and high-frequency rumble responses; observed {:?}",
                report.feedback
            )));
        }
        println!(
            "validated {} rumble response(s) with isolated low and high channels",
            report.feedback.responses
        );
        Ok(())
    }

    fn poison(options: Options) -> SpikeResult<()> {
        let (connection, _) = connect(&options)?;
        let mut transport = connection.into_inner();
        transport.write_all(&[b'S', b'C', 1, 3, 18, 0, 0x34, 0x12, 0xaa, 0xbb, 0xcc])?;
        println!(
            "left an incomplete frame in the firmware decoder; use kill -9 {} within {} seconds",
            std::process::id(),
            options.duration.as_secs()
        );
        let deadline = Instant::now() + options.duration;
        while Instant::now() < deadline && !STOP_REQUESTED.load(Ordering::Acquire) {
            thread::sleep(ACTIVE_SERVICE_INTERVAL);
        }
        transport.clear_dtr()?;
        if STOP_REQUESTED.load(Ordering::Acquire) {
            println!("poison test interrupted; cleared DTR normally");
            return Ok(());
        }
        Err(spike_error(
            "poison mode exited normally; repeat it and use kill -9",
        ))
    }

    fn replay(options: Options) -> SpikeResult<()> {
        let path = options.recording.as_ref().expect("validated by parser");
        let session = ReplaySession::read(BufReader::new(File::open(path).map_err(|error| {
            spike_error(format!("cannot open '{}': {error}", path.display()))
        })?))?;
        let (mut connection, clock) = connect(&options)?;
        wait_for_official_firmware(&mut connection, clock, options.duration)?;
        let replay_result = {
            let mut output = ConnectionOutput {
                connection: &mut connection,
                clock,
            };
            session.play_once(&mut output, ReplayOptions::default())
        };
        close(connection, clock)?;
        let stats = replay_result?;
        if stats.states_sent == 0 {
            return Err(spike_error(format!(
                "'{}' contains no mapped gamepad states",
                path.display()
            )));
        }
        println!(
            "replay passed: processed {} events, sent {} states, ignored {} events",
            stats.events_processed, stats.states_sent, stats.events_ignored
        );
        Ok(())
    }

    fn reconnect(options: Options) -> SpikeResult<()> {
        let info = find_device(None)?;
        let stable_serial = info
            .serial_number()
            .ok_or_else(|| spike_error("official XIAO bridge has no stable serial"))?
            .to_owned();
        println!(
            "watching {} for detach/replug during the next {} seconds; press Ctrl-C for a clean stop",
            masked_serial(Some(&stable_serial)),
            options.duration.as_secs()
        );
        let deadline = Instant::now() + options.duration;
        let mut disconnected_at = None;
        while Instant::now() < deadline && !STOP_REQUESTED.load(Ordering::Acquire) {
            let (mut connection, clock) = match connect_expected(&options, Some(&stable_serial)) {
                Ok(connected) => connected,
                Err(error) => {
                    if disconnected_at.is_none() {
                        disconnected_at = Some(Instant::now());
                        println!("waiting for bridge: {error}");
                    }
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }
            };
            wait_for_official_firmware(&mut connection, clock, Duration::from_secs(3))?;
            if let Some(started) = disconnected_at.take() {
                println!("reconnected after {} ms", started.elapsed().as_millis());
            } else {
                println!("initial connection ready");
            }
            let mut state = GamepadState {
                left_x: 1.0,
                ..GamepadState::neutral()
            };
            state.buttons.set(Button::South, true);
            connection.queue_state(state)?;
            loop {
                if Instant::now() >= deadline || STOP_REQUESTED.load(Ordering::Acquire) {
                    close(connection, clock)?;
                    println!("reconnect test stopped cleanly");
                    return Ok(());
                }
                if let Err(error) = connection.poll(clock.elapsed()) {
                    println!("bridge disconnected: {error}");
                    println!("metrics before disconnect: {:?}", connection.metrics());
                    let transport = connection.into_inner();
                    println!(
                        "USB inbound sequence gaps before disconnect: {}",
                        transport.inbound_sequence_gaps
                    );
                    drop(transport);
                    disconnected_at = Some(Instant::now());
                    break;
                }
                thread::sleep(ACTIVE_SERVICE_INTERVAL);
            }
        }
        println!("reconnect test ended while the bridge was detached");
        Ok(())
    }

    fn dedupe() -> SpikeResult<()> {
        let raw = find_device(None)?;
        let serial = raw
            .serial_number()
            .ok_or_else(|| spike_error("official XIAO bridge has no stable serial"))?;
        let tty_matches = available_serial_devices()?
            .into_iter()
            .filter(|device| {
                device.is_bridge_device()
                    && device.vendor_id == Some(VENDOR_ID)
                    && device.product_id == Some(PRODUCT_ID)
                    && device.manufacturer.as_deref() == Some(MANUFACTURER)
                    && device.serial_number.as_deref() == Some(serial)
            })
            .collect::<Vec<_>>();
        let tty = super::only(
            tty_matches,
            "tty endpoint matching the raw-USB stable serial",
        )?;
        println!(
            "deduplicated raw USB and {} as one bridge {}; selected tty",
            tty.path,
            masked_serial(Some(serial))
        );
        let mut output = SerialOutput::open(&tty.path, 115_200, SerialConfig::default())?;
        let firmware = output.wait_for_firmware_info(Duration::from_secs(3))?;
        validate_official_firmware(firmware)?;
        println!("preferred tty endpoint completed Hello and firmware reporting");
        Ok(())
    }

    struct ConnectionOutput<'a> {
        connection: &'a mut SerialConnection<UsbTransport>,
        clock: Instant,
    }

    impl GamepadOutput for ConnectionOutput<'_> {
        fn send_state(&mut self, state: &GamepadState) -> Result<(), OutputError> {
            if STOP_REQUESTED.load(Ordering::Acquire) {
                return Err(OutputError::Transport("replay interrupted".to_owned()));
            }
            self.connection
                .queue_state(*state)
                .and_then(|()| self.connection.poll(self.clock.elapsed()))
                .map_err(|error| OutputError::Transport(error.to_string()))
        }

        fn service(&mut self) -> Result<(), OutputError> {
            if STOP_REQUESTED.load(Ordering::Acquire) {
                return Err(OutputError::Transport("replay interrupted".to_owned()));
            }
            self.connection
                .poll(self.clock.elapsed())
                .map_err(|error| OutputError::Transport(error.to_string()))
        }
    }

    fn connect(options: &Options) -> SpikeResult<(SerialConnection<UsbTransport>, Instant)> {
        connect_expected(options, None)
    }

    fn connect_expected(
        options: &Options,
        expected_serial: Option<&str>,
    ) -> SpikeResult<(SerialConnection<UsbTransport>, Instant)> {
        let transport = UsbTransport::open(options, expected_serial)?;
        let clock = Instant::now();
        let mut connection =
            SerialConnection::new(transport, SerialConfig::default(), Duration::ZERO)?;
        while connection.status() == SerialStatus::Handshaking {
            connection.poll(clock.elapsed())?;
            if connection.status() == SerialStatus::Handshaking {
                thread::sleep(ACTIVE_SERVICE_INTERVAL);
            }
        }
        if connection.status() != SerialStatus::Ready {
            return Err(spike_error(format!(
                "protocol did not become ready: {:?}",
                connection.status()
            )));
        }
        println!("protocol-v1 Hello completed");
        Ok((connection, clock))
    }

    fn wait_for_official_firmware(
        connection: &mut SerialConnection<UsbTransport>,
        clock: Instant,
        timeout: Duration,
    ) -> SpikeResult<FirmwareInfo> {
        let deadline = Instant::now() + timeout;
        loop {
            connection.poll(clock.elapsed())?;
            let info = connection.firmware_info();
            if info.version != FirmwareVersion::Pending {
                validate_official_firmware(info)?;
                return Ok(info);
            }
            if STOP_REQUESTED.load(Ordering::Acquire) {
                return Err(spike_error("firmware query interrupted"));
            }
            if Instant::now() >= deadline {
                return Err(spike_error(format!(
                    "firmware report remained pending for {} seconds",
                    timeout.as_secs()
                )));
            }
            thread::sleep(ACTIVE_SERVICE_INTERVAL);
        }
    }

    fn validate_official_firmware(info: FirmwareInfo) -> SpikeResult<()> {
        let expected_target = FirmwareTargetId::new(FIRMWARE_TARGET_ID)?;
        if !matches!(info.version, FirmwareVersion::Reported(_)) {
            return Err(spike_error(format!(
                "official firmware did not report a valid revision: {info:?}"
            )));
        }
        if info.target != FirmwareTarget::Reported(expected_target) {
            return Err(spike_error(format!(
                "firmware target is not {FIRMWARE_TARGET_ID}: {info:?}"
            )));
        }
        Ok(())
    }

    #[derive(Debug, Default)]
    struct ServiceReport {
        feedback: FeedbackObservation,
        interrupted: bool,
    }

    fn service_for(
        connection: &mut SerialConnection<UsbTransport>,
        clock: Instant,
        duration: Duration,
        interval: Duration,
        print_feedback: bool,
    ) -> SpikeResult<ServiceReport> {
        let deadline = Instant::now() + duration;
        let mut report = ServiceReport::default();
        while Instant::now() < deadline && !STOP_REQUESTED.load(Ordering::Acquire) {
            connection.poll(clock.elapsed())?;
            if let Some(value) = connection.take_feedback() {
                match value {
                    OutputFeedback::Rumble {
                        low_frequency,
                        high_frequency,
                    } => {
                        report.feedback.observe(low_frequency, high_frequency);
                        if print_feedback {
                            println!("rumble low={low_frequency} high={high_frequency}");
                        }
                    }
                }
            }
            thread::sleep(interval);
        }
        report.interrupted = STOP_REQUESTED.load(Ordering::Acquire);
        Ok(report)
    }

    fn close(mut connection: SerialConnection<UsbTransport>, clock: Instant) -> SpikeResult<()> {
        if connection.status() == SerialStatus::Ready {
            connection.send_neutral_now()?;
            connection.poll(clock.elapsed())?;
        }
        let metrics = connection.metrics();
        let mut transport = connection.into_inner();
        transport.clear_dtr()?;
        println!("metrics: {metrics:?}");
        println!(
            "USB inbound sequence gaps: {}",
            transport.inbound_sequence_gaps
        );
        Ok(())
    }

    struct UsbTransport {
        control: Interface,
        _data: Interface,
        bulk_in: Endpoint<Bulk, In>,
        bulk_out: Endpoint<Bulk, Out>,
        dtr_asserted: bool,
        observer: StreamDecoder,
        last_inbound_sequence: Option<u16>,
        inbound_sequence_gaps: u64,
    }

    impl UsbTransport {
        fn open(options: &Options, expected_serial: Option<&str>) -> SpikeResult<Self> {
            let info = find_device(expected_serial)?;
            let preflight = preflight_topology(&info)?;
            verify_xpad_driver(&info, preflight.xinput_interface)?;
            let node = PathBuf::from(format!(
                "/dev/bus/usb/{:03}/{:03}",
                info.busnum(),
                info.device_address()
            ));
            let fd = rustix::fs::open(&node, OFlags::RDWR | OFlags::CLOEXEC, Mode::empty())
                .map_err(|error| {
                    spike_error(format!("cannot open '{}': {error}", node.display()))
                })?;
            let privileges_dropped = if options.drop_privileges {
                drop_privileges(&fd, interface_mask(preflight)?)?;
                true
            } else {
                false
            };
            let device = nusb::Device::from_fd(fd).wait()?;
            let configuration_value = device.active_configuration()?.configuration_value();
            let topology = discover_topology(device.active_configuration()?)?;
            if topology.control_interface != preflight.control_interface
                || topology.data_interface != preflight.data_interface
                || topology.xinput_interface != preflight.xinput_interface
            {
                return Err(spike_error(format!(
                    "sysfs and active descriptor topology disagree: {preflight:?} vs {topology:?}"
                )));
            }
            println!(
                "device serial={} configuration={} control={} data={} xinput={} bulk_out=0x{:02x} bulk_in=0x{:02x}",
                masked_serial(info.serial_number()),
                configuration_value,
                topology.control_interface,
                topology.data_interface,
                topology.xinput_interface,
                topology.bulk_out,
                topology.bulk_in,
            );

            let control = device.claim_interface(topology.control_interface).wait()?;
            let data = device.claim_interface(topology.data_interface).wait()?;
            if privileges_dropped {
                match device.claim_interface(topology.xinput_interface).wait() {
                    Ok(unexpected) => {
                        drop(unexpected);
                        return Err(spike_error(
                            "privilege mask unexpectedly allowed claiming the Xbox interface",
                        ));
                    }
                    Err(error) if error.os_error() == Some(EACCES) => {
                        println!("Xbox-interface negative claim rejected with EACCES")
                    }
                    Err(error) => {
                        return Err(spike_error(format!(
                            "Xbox-interface negative claim failed with {error}, not EACCES"
                        )));
                    }
                }
            }

            let bulk_in = data.endpoint::<Bulk, In>(topology.bulk_in)?;
            let bulk_out = data.endpoint::<Bulk, Out>(topology.bulk_out)?;
            let read_size = bulk_in.max_packet_size().max(64);
            let mut transport = Self {
                control,
                _data: data,
                bulk_in,
                bulk_out,
                dtr_asserted: false,
                observer: StreamDecoder::new(),
                last_inbound_sequence: None,
                inbound_sequence_gaps: 0,
            };
            set_line_coding(&transport.control, topology.control_interface)?;
            transport.set_dtr(false)?;
            thread::sleep(options.dtr_low);
            transport.set_dtr(true)?;
            println!(
                "forced DTR low for {} ms, then high",
                options.dtr_low.as_millis()
            );
            transport.bulk_in.submit(Buffer::new(read_size));
            Ok(transport)
        }

        fn set_dtr(&mut self, asserted: bool) -> SpikeResult<()> {
            set_dtr(&self.control, self.control.interface_number(), asserted)?;
            self.dtr_asserted = asserted;
            Ok(())
        }

        fn clear_dtr(&mut self) -> SpikeResult<()> {
            if self.dtr_asserted {
                self.set_dtr(false)?;
                println!("cleared DTR");
            }
            Ok(())
        }
    }

    impl ByteTransport for UsbTransport {
        fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
            let completion = self
                .bulk_out
                .transfer_blocking(Buffer::from(bytes), BULK_OUT_TIMEOUT);
            completion.status.map_err(io::Error::from)?;
            if completion.actual_len != bytes.len() {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    format!(
                        "bulk OUT transferred {} of {} bytes",
                        completion.actual_len,
                        bytes.len()
                    ),
                ));
            }
            Ok(())
        }

        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            let Some(mut completion) = self.bulk_in.wait_next_complete(Duration::ZERO) else {
                return Err(io::Error::from(io::ErrorKind::WouldBlock));
            };
            completion.status.map_err(io::Error::from)?;
            if completion.actual_len > bytes.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "bulk IN completion exceeded the protocol read buffer",
                ));
            }
            bytes[..completion.actual_len]
                .copy_from_slice(&completion.buffer[..completion.actual_len]);
            let count = completion.actual_len;
            for frame in self.observer.push(&bytes[..count]).into_iter().flatten() {
                if let Some(previous) = self.last_inbound_sequence {
                    self.inbound_sequence_gaps +=
                        u64::from(frame.sequence.wrapping_sub(previous).wrapping_sub(1));
                }
                self.last_inbound_sequence = Some(frame.sequence);
            }
            completion.buffer.clear();
            self.bulk_in.submit(completion.buffer);
            Ok(count)
        }
    }

    impl Drop for UsbTransport {
        fn drop(&mut self) {
            let _ = self.clear_dtr();
        }
    }

    fn find_device(expected_serial: Option<&str>) -> SpikeResult<DeviceInfo> {
        let matches = nusb::list_devices()
            .wait()?
            .filter(|device| {
                device.vendor_id() == VENDOR_ID
                    && device.product_id() == PRODUCT_ID
                    && device.manufacturer_string() == Some(MANUFACTURER)
                    && device.product_string() == Some(BRIDGE_DEVICE_USB_PRODUCT)
                    && device
                        .serial_number()
                        .is_some_and(|serial| !serial.is_empty())
                    && expected_serial
                        .is_none_or(|expected| device.serial_number() == Some(expected))
            })
            .collect::<Vec<_>>();
        super::only(matches, "official XIAO bridge USB device")
    }

    fn preflight_topology(info: &DeviceInfo) -> SpikeResult<Topology> {
        let control = super::only(
            info.interfaces()
                .filter(|interface| {
                    interface.class() == super::USB_CLASS_COMMUNICATIONS
                        && interface.subclass() == super::CDC_SUBCLASS_ACM
                        && interface.protocol() == 0
                })
                .collect::<Vec<_>>(),
            "preflight CDC control interface",
        )?
        .interface_number();
        let data = super::only(
            info.interfaces()
                .filter(|interface| {
                    interface.class() == super::USB_CLASS_CDC_DATA
                        && interface.subclass() == 0
                        && interface.protocol() == 0
                })
                .collect::<Vec<_>>(),
            "preflight CDC data interface",
        )?
        .interface_number();
        let xinput = super::only(
            info.interfaces()
                .filter(|interface| {
                    interface.class() == super::USB_CLASS_VENDOR
                        && interface.subclass() == super::XINPUT_SUBCLASS
                        && interface.protocol() == super::XINPUT_PROTOCOL
                })
                .collect::<Vec<_>>(),
            "preflight Xbox interface",
        )?
        .interface_number();
        Ok(Topology {
            control_interface: control,
            data_interface: data,
            xinput_interface: xinput,
            bulk_in: 0,
            bulk_out: 0,
        })
    }

    fn interface_mask(topology: Topology) -> SpikeResult<u32> {
        let mut mask = 0_u32;
        for interface in [topology.control_interface, topology.data_interface] {
            mask |= 1_u32
                .checked_shl(u32::from(interface))
                .ok_or_else(|| spike_error("USBDEVFS_DROP_PRIVILEGES supports interfaces 0-31"))?;
        }
        Ok(mask)
    }

    fn drop_privileges(fd: &OwnedFd, mask: u32) -> SpikeResult<()> {
        // SAFETY: USBDEVFS_DROP_PRIVILEGES takes a pointer to a u32 interface mask.
        let request =
            unsafe { rustix::ioctl::Setter::<{ USBDEVFS_DROP_PRIVILEGES as _ }, u32>::new(mask) };
        // SAFETY: fd is an open usbfs device and request carries the kernel's u32 ABI.
        unsafe { rustix::ioctl::ioctl(fd, request) }?;
        println!("applied USBDEVFS_DROP_PRIVILEGES mask=0x{mask:08x}");
        Ok(())
    }

    fn verify_xpad_driver(info: &DeviceInfo, interface: u8) -> SpikeResult<()> {
        let configuration_path = info.sysfs_path().join("bConfigurationValue");
        let configuration = fs::read_to_string(&configuration_path).map_err(|error| {
            spike_error(format!(
                "cannot read active USB configuration from '{}': {error}",
                configuration_path.display()
            ))
        })?;
        let configuration = configuration.trim();
        let interface_path = usb_interface_sysfs_path(info.sysfs_path(), configuration, interface)?;
        let driver_path = interface_path.join("driver");
        let driver = fs::read_link(&driver_path).map_err(|error| {
            spike_error(format!(
                "cannot read the Xbox interface driver from '{}': {error}",
                driver_path.display()
            ))
        })?;
        if driver.file_name().and_then(|name| name.to_str()) != Some("xpad") {
            return Err(spike_error(format!(
                "Xbox interface is not owned by xpad: {}",
                driver.display()
            )));
        }
        println!("verified xpad owns interface {interface}");
        Ok(())
    }

    fn set_line_coding(control: &Interface, interface: u8) -> SpikeResult<()> {
        let mut line_coding = Vec::with_capacity(7);
        line_coding.extend_from_slice(&115_200_u32.to_le_bytes());
        line_coding.extend_from_slice(&[0, 0, 8]);
        control
            .control_out(
                ControlOut {
                    control_type: ControlType::Class,
                    recipient: Recipient::Interface,
                    request: 0x20,
                    value: 0,
                    index: u16::from(interface),
                    data: &line_coding,
                },
                CONTROL_TIMEOUT,
            )
            .wait()?;
        Ok(())
    }

    fn set_dtr(control: &Interface, interface: u8, asserted: bool) -> SpikeResult<()> {
        control
            .control_out(
                ControlOut {
                    control_type: ControlType::Class,
                    recipient: Recipient::Interface,
                    request: 0x22,
                    value: u16::from(asserted),
                    index: u16::from(interface),
                    data: &[],
                },
                CONTROL_TIMEOUT,
            )
            .wait()?;
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn main() {
    if let Err(error) = linux::main() {
        eprintln!("xiao-usb-spike: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("xiao-usb-spike must run on Linux");
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topology_is_descriptor_driven() {
        let bytes = configuration_descriptor(4, 7, 9);
        let descriptor = ConfigurationDescriptor::new(&bytes).expect("valid configuration");
        assert_eq!(
            discover_topology(descriptor).unwrap(),
            Topology {
                control_interface: 4,
                data_interface: 7,
                xinput_interface: 9,
                bulk_in: 0x82,
                bulk_out: 0x01,
            }
        );
    }

    #[test]
    fn topology_rejects_a_union_pointing_at_a_non_data_interface() {
        let mut bytes = configuration_descriptor(4, 7, 9);
        let union_slave = bytes
            .windows(3)
            .position(|window| window == [5, 0x24, 0x06])
            .expect("union descriptor")
            + 4;
        bytes[union_slave] = 6;
        let descriptor = ConfigurationDescriptor::new(&bytes).unwrap();
        assert!(discover_topology(descriptor).is_err());
    }

    #[test]
    fn topology_rejects_descriptors_that_do_not_support_the_required_operations() {
        for (name, index, value) in [
            ("CDC version", 22, 0x00),
            ("ACM capabilities", 31, 0x00),
            ("control endpoint count", 13, 0x02),
            ("Xbox endpoint transfer type", 79, 0x02),
        ] {
            let mut bytes = configuration_descriptor(4, 7, 9);
            bytes[index] = value;
            let descriptor = ConfigurationDescriptor::new(&bytes).unwrap();
            assert!(discover_topology(descriptor).is_err(), "accepted {name}");
        }
    }

    #[test]
    fn rumble_gate_requires_isolated_nonzero_low_and_high_channels() {
        let mut observation = FeedbackObservation::default();
        observation.observe(0, 0);
        observation.observe(10, 20);
        assert!(!observation.low_only);
        assert!(!observation.high_only);

        observation.observe(10, 0);
        assert!(observation.low_only);
        assert!(!observation.high_only);

        observation.observe(0, 20);
        assert!(observation.high_only);
        assert_eq!(observation.responses, 4);
    }

    #[test]
    fn diagnostic_serials_never_reveal_short_values() {
        assert_eq!(masked_serial(None), "<none>");
        assert_eq!(masked_serial(Some("abcd")), "****");
        assert_eq!(masked_serial(Some("5E6EF905E5468F85")), "****8F85");
    }

    #[test]
    fn interface_sysfs_path_is_a_child_of_the_canonical_device_path() {
        let device = Path::new("/sys/devices/platform/vhci_hcd.0/usb3/3-1");
        assert_eq!(
            usb_interface_sysfs_path(device, "1", 2).unwrap(),
            device.join("3-1:1.2")
        );
    }

    fn configuration_descriptor(control: u8, data: u8, xinput: u8) -> Vec<u8> {
        vec![
            9, 2, 90, 0, 3, 1, 0, 0x80, 50, 9, 4, control, 0, 1, 2, 2, 0, 4, 5, 0x24, 0, 0x10,
            0x01, 5, 0x24, 1, 0, data, 4, 0x24, 2, 2, 5, 0x24, 6, control, data, 7, 5, 0x81, 3, 8,
            0, 16, 9, 4, data, 0, 2, 0x0a, 0, 0, 0, 7, 5, 0x01, 2, 64, 0, 0, 7, 5, 0x82, 2, 64, 0,
            0, 9, 4, xinput, 0, 2, 0xff, 0x5d, 1, 5, 7, 5, 0x02, 3, 32, 0, 8, 7, 5, 0x83, 3, 32, 0,
            4,
        ]
    }
}
