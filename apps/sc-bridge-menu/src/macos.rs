use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bridge_runtime::{BridgeHandle, BridgeRuntime, BridgeStatus, RuntimeConfig};
use tiny_skia::{
    FillRule, LineCap, LineJoin, Paint, Path as SkiaPath, PathBuilder, Pixmap, Stroke, Transform,
};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::WindowId;

use crate::model::{MenuModel, TrayState};

const POLL_INTERVAL: Duration = Duration::from_millis(250);
const LOG_LIMIT_BYTES: u64 = 2 * 1024 * 1024;
const START_ID: &str = "start";
const STOP_ID: &str = "stop";
const COPY_ERROR_ID: &str = "copy-error";
const COPY_ID: &str = "copy-diagnostics";
const SETTINGS_ID: &str = "input-monitoring";
const LOGS_ID: &str = "open-logs";
const QUIT_ID: &str = "quit";

pub fn run() -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|error| error.to_string())?;
    let mut app = MenuApp::new()?;
    event_loop
        .run_app(&mut app)
        .map_err(|error| error.to_string())
}

struct MenuItems {
    bridge: MenuItem,
    status: MenuItem,
    input: MenuItem,
    controller: MenuItem,
    xiao: MenuItem,
    battery: MenuItem,
    haptics: MenuItem,
    problem: MenuItem,
    start: MenuItem,
    stop: MenuItem,
    copy_error: MenuItem,
}

struct MenuApp {
    runtime: BridgeHandle,
    tray: Option<TrayIcon>,
    items: Option<MenuItems>,
    last_revision: u64,
    last_model: Option<MenuModel>,
    next_poll: Instant,
    logger: StatusLogger,
    shutting_down: bool,
}

impl MenuApp {
    fn new() -> Result<Self, String> {
        Ok(Self {
            runtime: BridgeRuntime::spawn(RuntimeConfig::default()),
            tray: None,
            items: None,
            last_revision: u64::MAX,
            last_model: None,
            next_poll: Instant::now(),
            logger: StatusLogger::new()?,
            shutting_down: false,
        })
    }

    fn create_tray(&mut self) -> Result<(), String> {
        let bridge = MenuItem::new("Bridge: Starting", false, None);
        let status = MenuItem::new("Status: Looking for hardware", false, None);
        let input = MenuItem::new("Input: Discovering", false, None);
        let controller = MenuItem::new("Controller: Not connected", false, None);
        let xiao = MenuItem::new("XIAO: Discovering", false, None);
        let battery = MenuItem::new("Battery: Unknown", false, None);
        let haptics = MenuItem::new("Haptics: Idle", false, None);
        let problem = MenuItem::new("Problem: None", false, None);
        let start = MenuItem::with_id(START_ID, "Start Bridge", false, None);
        let stop = MenuItem::with_id(STOP_ID, "Stop Bridge", true, None);
        let copy_error = MenuItem::with_id(COPY_ERROR_ID, "Copy Full Error", false, None);
        let copy = MenuItem::with_id(COPY_ID, "Copy Diagnostics", true, None);
        let settings = MenuItem::with_id(SETTINGS_ID, "Open Input Monitoring Settings", true, None);
        let logs = MenuItem::with_id(LOGS_ID, "Open Log Folder", true, None);
        let quit = MenuItem::with_id(QUIT_ID, "Quit", true, None);
        let separator1 = PredefinedMenuItem::separator();
        let separator2 = PredefinedMenuItem::separator();
        let separator3 = PredefinedMenuItem::separator();
        let separator4 = PredefinedMenuItem::separator();
        let menu = Menu::with_items(&[
            &bridge,
            &status,
            &separator1,
            &controller,
            &input,
            &xiao,
            &battery,
            &haptics,
            &separator2,
            &problem,
            &copy_error,
            &separator3,
            &start,
            &stop,
            &separator4,
            &copy,
            &settings,
            &logs,
            &quit,
        ])
        .map_err(|error| error.to_string())?;
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Steam Controller Bridge")
            .with_icon(template_icon(TrayState::Waiting)?)
            .with_icon_as_template(true)
            .build()
            .map_err(|error| error.to_string())?;
        self.items = Some(MenuItems {
            bridge,
            status,
            input,
            controller,
            xiao,
            battery,
            haptics,
            problem,
            start,
            stop,
            copy_error,
        });
        self.tray = Some(tray);
        self.refresh_status();
        Ok(())
    }

    fn refresh_status(&mut self) {
        let status = self.runtime.status();
        if status.revision == self.last_revision {
            return;
        }
        let model = MenuModel::from_status(&status);
        let icon_changed = self
            .last_model
            .as_ref()
            .is_none_or(|previous| previous.tray_state != model.tray_state);
        if self.last_model.as_ref() != Some(&model) {
            if let Some(items) = &self.items {
                items.bridge.set_text(&model.bridge);
                items.status.set_text(&model.status);
                items.input.set_text(&model.input);
                items.controller.set_text(&model.controller);
                items.xiao.set_text(&model.xiao);
                items.battery.set_text(&model.battery);
                items.haptics.set_text(&model.haptics);
                items.problem.set_text(&model.problem);
                items.start.set_enabled(model.start_enabled);
                items.stop.set_enabled(model.stop_enabled);
                items.copy_error.set_enabled(model.has_error);
            }
            if let Some(tray) = &self.tray {
                if icon_changed {
                    match template_icon(model.tray_state) {
                        Ok(icon) => {
                            // `set_icon` installs a non-template NSImage on macOS even when
                            // the tray was originally built with `with_icon_as_template`.
                            // Mark every replacement as a template so AppKit recolors it.
                            if let Err(error) = tray.set_icon_with_as_template(Some(icon), true) {
                                eprintln!("cannot update menu-bar icon: {error}");
                            }
                        }
                        Err(error) => eprintln!("cannot render menu-bar icon: {error}"),
                    }
                }
                let _ = tray.set_tooltip(Some(model.tray_state.tooltip()));
            }
            self.last_model = Some(model);
        }
        if let Err(error) = self.logger.write_status(&status) {
            eprintln!("cannot write menu-app diagnostics: {error}");
        }
        self.last_revision = status.revision;
    }

    fn handle_menu_event(&mut self, id: &str, event_loop: &ActiveEventLoop) {
        match id {
            START_ID => {
                if let Err(error) = self.runtime.request_start() {
                    eprintln!("cannot start bridge: {error}");
                }
            }
            STOP_ID => {
                if let Err(error) = self.runtime.request_stop() {
                    eprintln!("cannot stop bridge: {error}");
                }
            }
            COPY_ERROR_ID => {
                if let Some(error) = self.runtime.status().last_error {
                    if let Err(copy_error) = copy_text(&error) {
                        eprintln!("cannot copy full error: {copy_error}");
                    }
                }
            }
            COPY_ID => {
                if let Err(error) = copy_diagnostics(&self.runtime.status()) {
                    eprintln!("cannot copy diagnostics: {error}");
                }
            }
            SETTINGS_ID => {
                if let Err(error) = open_path(
                    "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent",
                ) {
                    eprintln!("cannot open Input Monitoring settings: {error}");
                }
            }
            LOGS_ID => {
                if let Err(error) = open_path(&self.logger.directory.to_string_lossy()) {
                    eprintln!("cannot open log folder: {error}");
                }
            }
            QUIT_ID => {
                self.shutdown();
                event_loop.exit();
            }
            _ => {}
        }
    }

    fn shutdown(&mut self) {
        if self.shutting_down {
            return;
        }
        self.shutting_down = true;
        if let Err(error) = self.runtime.shutdown() {
            eprintln!("bridge shutdown failed: {error}");
        }
    }
}

impl ApplicationHandler for MenuApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.tray.is_none() {
            if let Err(error) = self.create_tray() {
                eprintln!("cannot create menu-bar icon: {error}");
                self.shutdown();
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        _event: WindowEvent,
    ) {
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            self.handle_menu_event(event.id.as_ref(), event_loop);
        }
        if Instant::now() >= self.next_poll {
            self.refresh_status();
            self.next_poll = Instant::now() + POLL_INTERVAL;
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_poll));
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.shutdown();
    }
}

const ICON_LOGICAL_WIDTH: u32 = 24;
const ICON_LOGICAL_HEIGHT: u32 = 18;
const ICON_RENDER_SCALE: u32 = 4;
const ICON_RENDER_SCALE_F32: f32 = 4.0;
const ICON_WIDTH: u32 = ICON_LOGICAL_WIDTH * ICON_RENDER_SCALE;
const ICON_HEIGHT: u32 = ICON_LOGICAL_HEIGHT * ICON_RENDER_SCALE;

fn template_icon(state: TrayState) -> Result<Icon, String> {
    Icon::from_rgba(template_icon_rgba(state), ICON_WIDTH, ICON_HEIGHT)
        .map_err(|error| error.to_string())
}

fn template_icon_rgba(state: TrayState) -> Vec<u8> {
    let mut pixmap =
        Pixmap::new(ICON_WIDTH, ICON_HEIGHT).expect("the fixed menu icon dimensions are valid");
    let mut paint = Paint::default();
    paint.set_color_rgba8(0, 0, 0, 255);
    paint.anti_alias = true;
    let transform = Transform::from_scale(ICON_RENDER_SCALE_F32, ICON_RENDER_SCALE_F32);

    stroke_icon_path(&mut pixmap, &controller_outline(), &paint, 1.4, transform);
    stroke_icon_path(&mut pixmap, &d_pad(), &paint, 1.3, transform);
    fill_icon_circle(&mut pixmap, &paint, 12.2, 7.1, 0.72, transform);
    fill_icon_circle(&mut pixmap, &paint, 14.2, 8.7, 0.72, transform);

    match state {
        TrayState::Off => {
            stroke_icon_path(&mut pixmap, &off_badge(), &paint, 1.5, transform);
        }
        TrayState::Waiting => {
            for x in [19.3, 21.2, 23.1] {
                fill_icon_circle(&mut pixmap, &paint, x, 9.0, 0.53, transform);
            }
        }
        TrayState::Ready => {
            stroke_icon_path(&mut pixmap, &ready_badge(), &paint, 1.55, transform);
        }
        TrayState::Error => {
            stroke_icon_path(&mut pixmap, &error_badge(), &paint, 1.55, transform);
            fill_icon_circle(&mut pixmap, &paint, 21.2, 12.8, 0.68, transform);
        }
    }

    pixmap.take()
}

fn controller_outline() -> SkiaPath {
    let mut path = PathBuilder::new();
    path.move_to(5.5, 2.4);
    path.cubic_to(3.7, 2.4, 2.3, 3.6, 1.9, 5.3);
    path.line_to(0.65, 11.6);
    path.cubic_to(0.2, 13.8, 1.3, 15.9, 3.05, 16.45);
    path.cubic_to(4.5, 16.9, 5.5, 15.8, 6.25, 14.35);
    path.line_to(7.25, 12.5);
    path.cubic_to(7.55, 11.9, 7.95, 11.7, 8.55, 11.7);
    path.line_to(9.35, 11.7);
    path.cubic_to(9.95, 11.7, 10.35, 11.9, 10.65, 12.5);
    path.line_to(11.65, 14.35);
    path.cubic_to(12.4, 15.8, 13.4, 16.9, 14.85, 16.45);
    path.cubic_to(16.6, 15.9, 17.7, 13.8, 17.25, 11.6);
    path.line_to(16.0, 5.3);
    path.cubic_to(15.6, 3.6, 14.2, 2.4, 12.4, 2.4);
    path.close();
    path.finish()
        .expect("the static controller outline is a valid path")
}

fn d_pad() -> SkiaPath {
    let mut path = PathBuilder::new();
    path.move_to(5.15, 6.4);
    path.line_to(5.15, 9.6);
    path.move_to(3.55, 8.0);
    path.line_to(6.75, 8.0);
    path.finish().expect("the static d-pad is a valid path")
}

fn off_badge() -> SkiaPath {
    let mut path = PathBuilder::new();
    path.move_to(19.3, 7.1);
    path.line_to(22.9, 10.9);
    path.move_to(22.9, 7.1);
    path.line_to(19.3, 10.9);
    path.finish().expect("the static off badge is a valid path")
}

fn ready_badge() -> SkiaPath {
    let mut path = PathBuilder::new();
    path.move_to(19.1, 9.1);
    path.line_to(20.7, 10.7);
    path.line_to(23.1, 6.8);
    path.finish()
        .expect("the static ready badge is a valid path")
}

fn error_badge() -> SkiaPath {
    let mut path = PathBuilder::new();
    path.move_to(21.2, 5.5);
    path.line_to(21.2, 10.2);
    path.finish()
        .expect("the static error badge is a valid path")
}

fn stroke_icon_path(
    pixmap: &mut Pixmap,
    path: &SkiaPath,
    paint: &Paint<'_>,
    width: f32,
    transform: Transform,
) {
    let stroke = Stroke {
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    pixmap.stroke_path(path, paint, &stroke, transform, None);
}

fn fill_icon_circle(
    pixmap: &mut Pixmap,
    paint: &Paint<'_>,
    x: f32,
    y: f32,
    radius: f32,
    transform: Transform,
) {
    let path =
        PathBuilder::from_circle(x, y, radius).expect("the static icon circle is a valid path");
    pixmap.fill_path(&path, paint, FillRule::Winding, transform, None);
}

struct StatusLogger {
    directory: PathBuf,
    path: PathBuf,
}

impl StatusLogger {
    fn new() -> Result<Self, String> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or("HOME is not set; cannot locate the user log directory")?;
        let directory = home.join("Library/Logs/Steam Controller Bridge");
        fs::create_dir_all(&directory).map_err(|error| {
            format!(
                "cannot create log directory '{}': {error}",
                directory.display()
            )
        })?;
        let path = directory.join("sc-bridge.log");
        Ok(Self { directory, path })
    }

    fn write_status(&self, status: &BridgeStatus) -> Result<(), String> {
        rotate_log(&self.path)?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|error| error.to_string())?;
        writeln!(
            log,
            "timestamp={timestamp} revision={} state={:?} detail={:?} \
             input_connected={} input_active={} input_transport={:?} \
             input_product={:?} input_serial={:?} controller_connected={} xiao_path={:?} \
             xiao_serial={:?} battery={:?} lizard_suppressed={} \
             lizard_refreshes={} lizard_failures={} lizard_refresh_age_ms={:?} last_error={:?} \
             haptics_state={:?} rumble_commands={} rumble_writes={} rumble_refreshes={} \
             rumble_coalesced={} rumble_failures={} rumble_command_age_ms={:?} \
             input_reports={} dropped_reports={} output_packets={} state_refreshes={}",
            status.revision,
            status.state,
            status.detail,
            status.source.connected,
            status.source.active,
            status.source.transport,
            status
                .source
                .identity
                .as_ref()
                .and_then(|info| info.product.as_deref()),
            status
                .source
                .identity
                .as_ref()
                .and_then(|info| info.serial_number.as_deref()),
            status.controller.connected,
            status.xiao.path,
            status.xiao.usb_serial,
            status.battery_percent,
            status.lizard.suppressed,
            status.lizard.refreshes,
            status.lizard.failures,
            status.lizard.last_refresh_age.map(|age| age.as_millis()),
            status.last_error,
            status.haptics.state,
            status.haptics.commands_received,
            status.haptics.writes,
            status.haptics.refreshes,
            status.haptics.coalesced_commands,
            status.haptics.failures,
            status.haptics.last_command_age.map(|age| age.as_millis()),
            status.bridge_metrics.input_reports,
            status.bridge_metrics.dropped_input_reports,
            status.bridge_metrics.output_packets,
            status.output_diagnostics.state_refreshes
        )
        .map_err(|error| error.to_string())
    }
}

fn rotate_log(path: &Path) -> Result<(), String> {
    let Ok(metadata) = path.metadata() else {
        return Ok(());
    };
    if metadata.len() < LOG_LIMIT_BYTES {
        return Ok(());
    }
    let rotated = path.with_extension("log.1");
    if rotated.exists() {
        fs::remove_file(&rotated).map_err(|error| error.to_string())?;
    }
    fs::rename(path, rotated).map_err(|error| error.to_string())
}

fn diagnostics_text(status: &BridgeStatus) -> String {
    let mut text = String::new();
    let _ = writeln!(text, "Steam Controller Bridge diagnostics");
    let _ = writeln!(text, "state: {:?}", status.state);
    let _ = writeln!(text, "detail: {}", status.detail);
    let _ = writeln!(text, "input_source: {:?}", status.source);
    let _ = writeln!(text, "controller: {:?}", status.controller);
    let _ = writeln!(text, "xiao: {:?}", status.xiao);
    let _ = writeln!(text, "battery_percent: {:?}", status.battery_percent);
    let _ = writeln!(text, "lizard: {:?}", status.lizard);
    let _ = writeln!(text, "haptics: {:?}", status.haptics);
    let _ = writeln!(text, "bridge_metrics: {:?}", status.bridge_metrics);
    let _ = writeln!(text, "output_diagnostics: {:?}", status.output_diagnostics);
    let _ = writeln!(text, "last_error: {:?}", status.last_error);
    text
}

fn copy_diagnostics(status: &BridgeStatus) -> Result<(), String> {
    copy_text(&diagnostics_text(status))
}

fn copy_text(value: &str) -> Result<(), String> {
    let mut process = Command::new("/usr/bin/pbcopy")
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    process
        .stdin
        .take()
        .ok_or("pbcopy stdin is unavailable")?
        .write_all(value.as_bytes())
        .map_err(|error| error.to_string())?;
    let exit = process.wait().map_err(|error| error.to_string())?;
    if exit.success() {
        Ok(())
    } else {
        Err(format!("pbcopy exited with {exit}"))
    }
}

fn open_path(path: &str) -> Result<(), String> {
    let status = Command::new("/usr/bin/open")
        .arg(path)
        .status()
        .map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("open exited with {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_include_hardware_and_safety_state() {
        let text = diagnostics_text(&BridgeStatus::default());
        assert!(text.contains("input_source:"));
        assert!(text.contains("xiao:"));
        assert!(text.contains("lizard:"));
        assert!(text.contains("haptics:"));
        assert!(text.contains("output_diagnostics:"));
    }

    #[test]
    fn template_icons_are_valid_and_distinct_for_every_state() {
        let states = [
            TrayState::Off,
            TrayState::Waiting,
            TrayState::Ready,
            TrayState::Error,
        ];
        let images: Vec<_> = states
            .iter()
            .map(|state| template_icon_rgba(*state))
            .collect();
        for (state, pixels) in states.iter().zip(&images) {
            assert!(template_icon(*state).is_ok());
            assert_eq!(
                pixels.len(),
                usize::try_from(ICON_WIDTH * ICON_HEIGHT * 4).unwrap()
            );
            assert!(pixels.chunks_exact(4).any(|pixel| pixel[3] != 0));
            assert!(
                pixels
                    .chunks_exact(4)
                    .any(|pixel| pixel[3] > 0 && pixel[3] < 255),
                "{state:?} should retain anti-aliased edges"
            );
            let occupied_rows: Vec<_> = pixels
                .chunks_exact(usize::try_from(ICON_WIDTH * 4).unwrap())
                .enumerate()
                .filter_map(|(row, pixels)| {
                    pixels
                        .chunks_exact(4)
                        .any(|pixel| pixel[3] > 8)
                        .then_some(row)
                })
                .collect();
            assert!(
                occupied_rows.last().unwrap() - occupied_rows.first().unwrap()
                    >= usize::try_from(14 * ICON_RENDER_SCALE).unwrap(),
                "{state:?} should fill the native menu-bar height"
            );
        }
        for left in 0..images.len() {
            for right in left + 1..images.len() {
                assert_ne!(images[left], images[right]);
            }
        }
    }
}
