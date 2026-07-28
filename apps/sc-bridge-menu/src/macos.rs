use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bridge_runtime::{BridgeHandle, BridgeRuntime, BridgeStatus, RuntimeConfig};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::WindowId;

use crate::model::MenuModel;

const POLL_INTERVAL: Duration = Duration::from_millis(250);
const LOG_LIMIT_BYTES: u64 = 2 * 1024 * 1024;
const START_ID: &str = "start";
const STOP_ID: &str = "stop";
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
    puck: MenuItem,
    controller: MenuItem,
    xiao: MenuItem,
    battery: MenuItem,
    error: MenuItem,
    start: MenuItem,
    stop: MenuItem,
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
        let puck = MenuItem::new("Puck: Discovering", false, None);
        let controller = MenuItem::new("Controller: Not connected", false, None);
        let xiao = MenuItem::new("XIAO: Discovering", false, None);
        let battery = MenuItem::new("Battery: Unknown", false, None);
        let error = MenuItem::new("Last error: None", false, None);
        let start = MenuItem::with_id(START_ID, "Start Bridge", false, None);
        let stop = MenuItem::with_id(STOP_ID, "Stop Bridge", true, None);
        let copy = MenuItem::with_id(COPY_ID, "Copy Diagnostics", true, None);
        let settings = MenuItem::with_id(SETTINGS_ID, "Open Input Monitoring Settings", true, None);
        let logs = MenuItem::with_id(LOGS_ID, "Open Log Folder", true, None);
        let quit = MenuItem::with_id(QUIT_ID, "Quit", true, None);
        let separator1 = PredefinedMenuItem::separator();
        let separator2 = PredefinedMenuItem::separator();
        let separator3 = PredefinedMenuItem::separator();
        let menu = Menu::with_items(&[
            &bridge,
            &puck,
            &controller,
            &xiao,
            &battery,
            &error,
            &separator1,
            &start,
            &stop,
            &separator2,
            &copy,
            &settings,
            &logs,
            &separator3,
            &quit,
        ])
        .map_err(|error| error.to_string())?;
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Steam Controller Bridge")
            .with_icon(template_icon()?)
            .with_icon_as_template(true)
            .build()
            .map_err(|error| error.to_string())?;
        self.items = Some(MenuItems {
            bridge,
            puck,
            controller,
            xiao,
            battery,
            error,
            start,
            stop,
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
        if self.last_model.as_ref() != Some(&model) {
            if let Some(items) = &self.items {
                items.bridge.set_text(&model.bridge);
                items.puck.set_text(&model.puck);
                items.controller.set_text(&model.controller);
                items.xiao.set_text(&model.xiao);
                items.battery.set_text(&model.battery);
                items.error.set_text(&model.error);
                items.start.set_enabled(model.start_enabled);
                items.stop.set_enabled(model.stop_enabled);
            }
            if let Some(tray) = &self.tray {
                let _ = tray.set_tooltip(Some(format!("{:?}", status.state)));
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

fn template_icon() -> Result<Icon, String> {
    const WIDTH: u32 = 18;
    const HEIGHT: u32 = 18;
    let mut rgba = vec![0_u8; usize::try_from(WIDTH * HEIGHT * 4).unwrap_or(0)];
    let pixels = [
        (5, 4),
        (6, 3),
        (7, 3),
        (8, 3),
        (9, 3),
        (10, 3),
        (11, 3),
        (12, 4),
        (4, 5),
        (13, 5),
        (3, 6),
        (14, 6),
        (3, 7),
        (14, 7),
        (2, 8),
        (15, 8),
        (2, 9),
        (15, 9),
        (2, 10),
        (15, 10),
        (3, 11),
        (14, 11),
        (4, 12),
        (13, 12),
        (5, 11),
        (6, 10),
        (11, 10),
        (12, 11),
        (6, 7),
        (6, 8),
        (5, 8),
        (7, 8),
        (11, 7),
        (12, 8),
    ];
    for (x, y) in pixels {
        let offset = usize::try_from((y * WIDTH + x) * 4).unwrap_or(0);
        rgba[offset..offset + 4].copy_from_slice(&[0, 0, 0, 255]);
    }
    Icon::from_rgba(rgba, WIDTH, HEIGHT).map_err(|error| error.to_string())
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
             puck_connected={} controller_connected={} xiao_path={:?} \
             xiao_serial={:?} battery={:?} lizard_suppressed={} \
             lizard_refreshes={} lizard_failures={} lizard_refresh_age_ms={:?} last_error={:?} \
             input_reports={} dropped_reports={} output_packets={} state_refreshes={}",
            status.revision,
            status.state,
            status.detail,
            status.puck.connected,
            status.controller.connected,
            status.xiao.path,
            status.xiao.usb_serial,
            status.battery_percent,
            status.lizard.suppressed,
            status.lizard.refreshes,
            status.lizard.failures,
            status.lizard.last_refresh_age.map(|age| age.as_millis()),
            status.last_error,
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
    let _ = writeln!(text, "puck: {:?}", status.puck);
    let _ = writeln!(text, "controller: {:?}", status.controller);
    let _ = writeln!(text, "xiao: {:?}", status.xiao);
    let _ = writeln!(text, "battery_percent: {:?}", status.battery_percent);
    let _ = writeln!(text, "lizard: {:?}", status.lizard);
    let _ = writeln!(text, "bridge_metrics: {:?}", status.bridge_metrics);
    let _ = writeln!(text, "output_diagnostics: {:?}", status.output_diagnostics);
    let _ = writeln!(text, "last_error: {:?}", status.last_error);
    text
}

fn copy_diagnostics(status: &BridgeStatus) -> Result<(), String> {
    let mut process = Command::new("/usr/bin/pbcopy")
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    process
        .stdin
        .take()
        .ok_or("pbcopy stdin is unavailable")?
        .write_all(diagnostics_text(status).as_bytes())
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
        assert!(text.contains("puck:"));
        assert!(text.contains("xiao:"));
        assert!(text.contains("lizard:"));
        assert!(text.contains("output_diagnostics:"));
    }

    #[test]
    fn template_icon_has_expected_dimensions_and_nontransparent_pixels() {
        assert!(template_icon().is_ok());
    }
}
