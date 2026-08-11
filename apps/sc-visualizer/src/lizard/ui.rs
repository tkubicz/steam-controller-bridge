//! Dedicated full-window lizard mouse lab workflow.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "bounded capture coordinates and animation clocks are rendered as egui f32 values"
)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use controller_art::PadSide;
use desktop_bindings::BindingProfile;
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use ui_theme::{ACCENT, DANGER, DETAIL, MUTED_TEXT, SUCCESS, SUNKEN, TEXT};

use crate::device::HidBrokerClient;

use super::capture::{CapturePreflight, GuiCapture, GuiCaptureCommand, GuiCaptureEvent};
use super::protocol::{guided_trials, GuidedTrial, TrialVisual};
use super::results::{
    available_profiles, write_reports, ArtifactPaths, LabResults, ResultWorker, StageTrajectory,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Landing,
    Capturing,
    Processing,
    Results,
}

#[derive(Debug, Clone, Copy)]
enum TrialPhase {
    Waiting,
    Countdown(Instant),
    Measuring(Instant),
    Review,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LabAction {
    Stay,
    Exit,
}

pub(crate) struct LabUi {
    hid_broker: HidBrokerClient,
    screen: Screen,
    profiles: Vec<BindingProfile>,
    selected_profile: usize,
    profile_warning: Option<String>,
    capture: Option<GuiCapture>,
    preflight: Option<CapturePreflight>,
    preflight_wait: Option<(Instant, u64)>,
    paths: Option<ArtifactPaths>,
    trials: Vec<GuidedTrial>,
    trial_index: usize,
    attempt: u32,
    phase: TrialPhase,
    preview: Vec<(i64, i64)>,
    result_worker: Option<ResultWorker>,
    results: Option<LabResults>,
    selected_trajectory: usize,
    status: String,
    error: Option<String>,
    pending_exit: bool,
}

impl LabUi {
    pub(crate) fn new(hid_broker: HidBrokerClient) -> Self {
        let (profiles, profile_warning) = available_profiles();
        let selected_profile = profiles
            .iter()
            .position(|profile| profile.name == desktop_bindings::DEFAULT_PROFILE_NAME)
            .unwrap_or(0);
        Self {
            hid_broker,
            screen: Screen::Landing,
            profiles,
            selected_profile,
            profile_warning,
            capture: None,
            preflight: None,
            preflight_wait: None,
            paths: None,
            trials: guided_trials(),
            trial_index: 0,
            attempt: 1,
            phase: TrialPhase::Waiting,
            preview: Vec::new(),
            result_worker: None,
            results: None,
            selected_trajectory: 0,
            status: "Choose a guided capture or open an existing recording.".to_owned(),
            error: None,
            pending_exit: false,
        }
    }

    pub(crate) fn ui(&mut self, ui: &mut egui::Ui) -> LabAction {
        self.poll_workers();
        // Landing and Results are purely input-driven; only live capture and
        // background processing need timed repaints to observe worker events.
        if matches!(self.screen, Screen::Capturing | Screen::Processing) {
            ui.ctx().request_repaint_after(Duration::from_millis(25));
        }
        let mut action = LabAction::Stay;
        egui::Panel::top("lizard_lab_header").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Lizard Mouse Lab");
                ui.add_space(12.0);
                if ui.button("← Controller dashboard").clicked() {
                    action = self.request_exit();
                }
            });
            ui.label(egui::RichText::new(&self.status).color(MUTED_TEXT));
        });
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| match self.screen {
                    Screen::Landing => self.landing(ui),
                    Screen::Capturing => self.capturing(ui),
                    Screen::Processing => self.processing(ui),
                    Screen::Results => self.results(ui),
                });
        });
        if self.pending_exit && self.capture.is_none() && self.result_worker.is_none() {
            action = LabAction::Exit;
        }
        action
    }

    fn landing(&mut self, ui: &mut egui::Ui) {
        ui.heading("Measure the original controller mouse behavior");
        ui.label(
            "The wizard records raw HID, lizard output, and passive macOS pointer events without disabling lizard mode.",
        );
        ui.add_space(12.0);
        self.profile_picker(ui);
        if let Some(warning) = &self.profile_warning {
            ui.colored_label(DANGER, warning);
        }
        ui.add_space(12.0);
        Self::permission_ui(ui);
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let capture = ui.add_enabled(
                cfg!(target_os = "macos"),
                egui::Button::new("New guided test…"),
            );
            if capture.clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("JSONL capture", &["jsonl"])
                    .set_file_name("lizard.jsonl")
                    .save_file()
                {
                    self.start_capture(ensure_jsonl_extension(path));
                }
            }
            if ui.button("Open existing capture…").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("JSONL capture", &["jsonl"])
                    .pick_file()
                {
                    self.open_existing(path);
                }
            }
        });
        if !cfg!(target_os = "macos") {
            ui.colored_label(
                MUTED_TEXT,
                "New capture is macOS-only. Existing recordings remain portable.",
            );
        }
        if let Some(error) = &self.error {
            ui.add_space(12.0);
            ui.colored_label(DANGER, error);
        }
        ui.add_space(18.0);
        ui.separator();
        ui.label(format!(
            "The thorough protocol contains {} individually illustrated trials and takes about three minutes.",
            self.trials.len()
        ));
    }

    fn profile_picker(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Compare against profile:");
            let current = self
                .profiles
                .get(self.selected_profile)
                .map_or("Default", |profile| profile.name.as_str());
            egui::ComboBox::from_id_salt("lizard_profile")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (index, profile) in self.profiles.iter().enumerate() {
                        ui.selectable_value(&mut self.selected_profile, index, &profile.name);
                    }
                });
        });
    }

    #[cfg(target_os = "macos")]
    fn permission_ui(ui: &mut egui::Ui) {
        use desktop_bindings::{input_monitoring_access, PermissionState};
        match input_monitoring_access() {
            PermissionState::Granted => {
                ui.colored_label(SUCCESS, "● Input Monitoring granted");
            }
            PermissionState::Undecided => {
                ui.colored_label(DANGER, "● Input Monitoring has not been requested");
                if ui.button("Request Input Monitoring").clicked() {
                    let _ = desktop_bindings::request_input_monitoring_access();
                }
            }
            PermissionState::Denied => {
                ui.colored_label(DANGER, "● Input Monitoring denied");
                ui.label(
                    "Enable Steam Controller Visualizer in System Settings → Privacy & Security → Input Monitoring, then relaunch it.",
                );
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn permission_ui(ui: &mut egui::Ui) {
        ui.colored_label(
            MUTED_TEXT,
            "Passive pointer capture is unavailable on this platform.",
        );
    }

    fn start_capture(&mut self, path: PathBuf) {
        self.paths = Some(ArtifactPaths::from_capture(path.clone()));
        self.capture = Some(GuiCapture::start(None, path, self.hid_broker.clone()));
        self.preflight = None;
        self.preflight_wait = None;
        self.trial_index = 0;
        self.attempt = 1;
        self.phase = TrialPhase::Waiting;
        self.preview.clear();
        self.error = None;
        self.screen = Screen::Capturing;
        "Preflight: detecting the controller before checking lizard mouse output…"
            .clone_into(&mut self.status);
    }

    fn open_existing(&mut self, path: PathBuf) {
        let paths = ArtifactPaths::from_capture(path);
        self.paths = Some(paths.clone());
        self.result_worker = Some(ResultWorker::start(
            paths,
            self.selected_profile().clone(),
            false,
        ));
        self.error = None;
        self.screen = Screen::Processing;
        "Loading and comparing the existing capture…".clone_into(&mut self.status);
    }

    fn capturing(&mut self, ui: &mut egui::Ui) {
        let Some(preflight) = &self.preflight else {
            ui.spinner();
            if let Some((started, timeout_secs)) = self.preflight_wait {
                let remaining = Duration::from_secs(timeout_secs)
                    .saturating_sub(started.elapsed())
                    .as_secs()
                    .saturating_add(1)
                    .min(timeout_secs);
                ui.heading("Move a controller pad now");
                ui.label(
                    egui::RichText::new(
                        "Touch and move either the left or right pad. This triggers the 0x40 lizard mouse reports required by the test.",
                    )
                    .strong()
                    .color(TEXT),
                );
                ui.label(format!(
                    "Waiting up to {timeout_secs} seconds - approximately {remaining} seconds remaining."
                ));
            } else {
                ui.label("Auto-detecting the active controller and preparing preflight…");
            }
            if ui.button("Cancel capture").clicked() {
                self.cancel_capture();
            }
            return;
        };
        self.preflight_ui(ui, preflight);
        if let Some(reason) = &preflight.invalid_reason {
            ui.colored_label(DANGER, reason);
            return;
        }
        ui.separator();
        let trial = self.trials[self.trial_index].clone();
        ui.heading(format!(
            "Trial {} of {} - {}",
            self.trial_index + 1,
            self.trials.len(),
            trial.title
        ));
        ui.label(&trial.instruction);
        ui.label(
            egui::RichText::new(
                "Use Enter/Space so another mouse does not contaminate the capture.",
            )
            .color(MUTED_TEXT),
        );
        let observed = matches!(self.phase, TrialPhase::Review).then_some(self.preview.as_slice());
        paint_trial(ui, &trial, observed);
        self.trial_controls(ui, &trial);
        if ui.button("Cancel and preserve partial capture").clicked() {
            self.cancel_capture();
        }
    }

    fn preflight_ui(&self, ui: &mut egui::Ui, preflight: &CapturePreflight) {
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "Controller {}: {} ({})",
                preflight.controller_index, preflight.controller, preflight.transport
            ));
            status_chip(ui, preflight.state_reports > 0, "state reports");
            status_chip(ui, preflight.lizard_reports > 0, "0x40 reports");
            status_chip(ui, preflight.event_tap_ready, "event tap");
            ui.label(format!("{} display(s)", preflight.displays.len()));
        });
        for display in &preflight.displays {
            ui.label(egui::RichText::new(display).color(MUTED_TEXT));
        }
        if let Some(paths) = &self.paths {
            ui.label(format!("Capture: {}", paths.capture.display()));
            ui.label(format!("Analysis: {}", paths.analysis.display()));
            ui.label(format!("Comparison: {}", paths.comparison.display()));
        }
    }

    fn trial_controls(&mut self, ui: &mut egui::Ui, trial: &GuidedTrial) {
        let start_key = ui.input(|input| {
            input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Space)
        });
        match self.phase {
            TrialPhase::Waiting => {
                if ui.button("Start trial (Enter)").clicked() || start_key {
                    self.phase = TrialPhase::Countdown(Instant::now());
                }
            }
            TrialPhase::Countdown(started) => {
                let elapsed = started.elapsed();
                if elapsed >= Duration::from_secs(1) {
                    if self.send_capture(GuiCaptureCommand::Start {
                        trial_id: trial.id.clone(),
                        attempt: self.attempt,
                    }) {
                        self.preview.clear();
                        self.phase = TrialPhase::Measuring(Instant::now());
                    }
                } else {
                    ui.heading(format!("Starting in {:.1}…", 1.0 - elapsed.as_secs_f32()));
                }
            }
            TrialPhase::Measuring(started) => {
                let remaining = trial.duration.saturating_sub(started.elapsed());
                ui.heading(format!("Recording {:.1} s", remaining.as_secs_f32()));
                if remaining.is_zero()
                    && self.send_capture(GuiCaptureCommand::End {
                        trial_id: trial.id.clone(),
                        attempt: self.attempt,
                    })
                {
                    self.phase = TrialPhase::Review;
                }
            }
            TrialPhase::Review => {
                ui.horizontal(|ui| {
                    if ui.button("Accept (Enter)").clicked() || start_key {
                        self.accept_trial(trial);
                    }
                    let retry_key = ui.input(|input| input.key_pressed(egui::Key::R));
                    if ui.button("Retry (R)").clicked() || retry_key {
                        self.retry_trial(trial);
                    }
                });
                ui.label(format!(
                    "Attempt {} captured {} reference motion points.",
                    self.attempt,
                    self.preview.len().saturating_sub(1)
                ));
            }
        }
    }

    fn accept_trial(&mut self, trial: &GuidedTrial) {
        if !self.send_capture(GuiCaptureCommand::Accept {
            trial_id: trial.id.clone(),
            attempt: self.attempt,
        }) {
            return;
        }
        if self.trial_index + 1 == self.trials.len() {
            let _ = self.send_capture(GuiCaptureCommand::Finish);
            self.screen = Screen::Processing;
            "Finalizing capture and flushing every accepted event…".clone_into(&mut self.status);
        } else {
            self.trial_index += 1;
            self.attempt = 1;
            self.preview.clear();
            self.phase = TrialPhase::Waiting;
        }
    }

    fn retry_trial(&mut self, trial: &GuidedTrial) {
        if self.send_capture(GuiCaptureCommand::Discard {
            trial_id: trial.id.clone(),
            attempt: self.attempt,
        }) {
            self.attempt = self.attempt.saturating_add(1);
            self.preview.clear();
            self.phase = TrialPhase::Waiting;
        }
    }

    fn cancel_capture(&mut self) {
        // `GuiCapture::cancel` also raises the stop flag: a worker still
        // blocked in controller discovery has no command loop yet, and a
        // command alone would leave this Processing screen waiting forever.
        if let Some(capture) = &self.capture {
            capture.cancel();
        }
        self.screen = Screen::Processing;
        "Canceling safely and finalizing the partial capture…".clone_into(&mut self.status);
    }

    fn processing(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(80.0);
            ui.spinner();
            ui.heading("Processing capture");
            ui.label(&self.status);
            if let Some(error) = &self.error {
                ui.colored_label(DANGER, error);
            }
        });
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the results screen keeps one linear presentation order for all descriptive metrics"
    )]
    fn results(&mut self, ui: &mut egui::Ui) {
        let Some(results) = &mut self.results else {
            return;
        };
        match results.analysis.capture_validity.metadata_valid {
            Some(true) => {
                ui.colored_label(SUCCESS, "● Valid capture");
            }
            Some(false) => {
                ui.colored_label(DANGER, "● Invalid or incomplete capture");
            }
            None => {
                ui.colored_label(
                    MUTED_TEXT,
                    "● Legacy capture - validity metadata unavailable",
                );
            }
        }
        if let Some(reason) = &results.analysis.capture_validity.invalid_reason {
            ui.colored_label(DANGER, reason);
        }
        if let Some(error) = &results.report_write_error {
            ui.colored_label(
                DANGER,
                format!("Capture preserved, but automatic report export failed: {error}"),
            );
        }
        if let Some(error) = &self.error {
            ui.colored_label(DANGER, error);
        }
        ui.label(format!(
            "{} accepted attempt(s), {} discarded retry attempt(s)",
            results.accepted_attempts, results.discarded_attempts
        ));
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            metric(
                ui,
                "Stationary leakage",
                results.analysis.stationary_leakage_pixels,
                "px",
            );
            metric(
                ui,
                "Click displacement",
                results.analysis.click_displacement_pixels,
                "px",
            );
            if let Some(summary) = &results.comparison.guided_summary {
                metric_f64(ui, "Guided RMS", summary.rms_path_error_pixels, "px");
                metric_f64(
                    ui,
                    "Path ratio",
                    summary.bridge_to_reference_ratio.unwrap_or(0.0),
                    "×",
                );
            } else {
                metric_f64(ui, "RMS", results.comparison.rms_path_error_pixels, "px");
            }
            metric_f64(
                ui,
                "Endpoint error",
                results.comparison.endpoint_error.distance_pixels,
                "px",
            );
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "State {:.1} Hz · lizard {:.1} Hz",
                results.analysis.cadence.state_hz, results.analysis.cadence.lizard_hz
            ));
            ui.label(format!(
                "Unmatched pointer events: {} · edge clipping: {}",
                results.analysis.raw_to_screen.unmatched_host_events,
                results.analysis.raw_to_screen.cursor_edge_clipping_events
            ));
            ui.label(format!(
                "Response latency median: {}",
                results
                    .analysis
                    .response_latency_us
                    .median
                    .map_or_else(|| "-".to_owned(), |value| format!("{value} µs"))
            ));
            ui.label(format!(
                "Bridge/reference latency error: {}",
                results
                    .comparison
                    .latency_error_us
                    .map_or_else(|| "-".to_owned(), |value| format!("{value:+} µs"))
            ));
            ui.label(format!(
                "Angular error: {}",
                results
                    .comparison
                    .angular_error_degrees
                    .map_or_else(|| "-".to_owned(), |value| format!("{value:.2}°"))
            ));
            if let Some(ratio) = results.analysis.raw_to_screen.host_to_raw_ratio {
                ui.label(format!("Host/raw motion ratio: {ratio:.3}×"));
            }
        });
        ui.label(format!(
            "Stationary leakage reference/bridge: {} / {} px · click leakage reference/bridge: {} / {} px",
            results.comparison.stationary_leakage.reference_pixels,
            results.comparison.stationary_leakage.bridge_pixels,
            results.comparison.click_leakage.reference_pixels,
            results.comparison.click_leakage.bridge_pixels
        ));
        if !results.comparison.speed_bin_response.is_empty() {
            ui.collapsing("Speed-bin response", |ui| {
                egui::Grid::new("lizard_speed_bins")
                    .striped(true)
                    .show(ui, |ui| {
                        ui.strong("Bin");
                        ui.strong("Samples");
                        ui.strong("Reference");
                        ui.strong("Bridge");
                        ui.end_row();
                        for bin in &results.comparison.speed_bin_response {
                            ui.label(bin.label);
                            ui.label(bin.samples.to_string());
                            ui.label(format!("{} px", bin.reference_pixels));
                            ui.label(format!("{} px", bin.bridge_pixels));
                            ui.end_row();
                        }
                    });
            });
        }
        ui.separator();
        if !results.trajectories.is_empty() {
            self.selected_trajectory = self
                .selected_trajectory
                .min(results.trajectories.len().saturating_sub(1));
            egui::ComboBox::from_id_salt("lizard_trajectory")
                .selected_text(&results.trajectories[self.selected_trajectory].name)
                .show_ui(ui, |ui| {
                    for (index, trajectory) in results.trajectories.iter().enumerate() {
                        ui.selectable_value(&mut self.selected_trajectory, index, &trajectory.name);
                    }
                });
            paint_trajectory(ui, &results.trajectories[self.selected_trajectory]);
        }
        ui.separator();
        ui.heading("Per-trial comparison");
        egui::Grid::new("lizard_stage_results")
            .striped(true)
            .show(ui, |ui| {
                ui.strong("Trial");
                ui.strong("Reference");
                ui.strong("Bridge");
                ui.strong("Ratio");
                ui.strong("RMS");
                ui.end_row();
                for stage in &results.comparison.guided_stages {
                    ui.label(&stage.name);
                    ui.label(format!("{} px", stage.reference_path_pixels));
                    ui.label(format!("{} px", stage.bridge_path_pixels));
                    ui.label(
                        stage
                            .bridge_to_reference_ratio
                            .map_or_else(|| "-".to_owned(), |ratio| format!("{ratio:.3}×")),
                    );
                    ui.label(format!("{:.2} px", stage.rms_path_error_pixels));
                    ui.end_row();
                }
            });
        ui.add_space(12.0);
        if results.reports_written {
            ui.colored_label(
                SUCCESS,
                format!(
                    "Reports saved to '{}' and '{}'.",
                    results.paths.analysis.display(),
                    results.paths.comparison.display()
                ),
            );
        } else if ui.button("Write reports beside capture").clicked() {
            match write_reports(results) {
                Ok(()) => {
                    results.reports_written = true;
                    results.report_write_error = None;
                    self.error = None;
                    "Reports written successfully".clone_into(&mut self.status);
                }
                Err(error) => self.error = Some(error),
            }
        }
        if ui.button("Start another test").clicked() {
            self.screen = Screen::Landing;
            self.results = None;
            self.error = None;
            "Choose a guided capture or open an existing recording.".clone_into(&mut self.status);
        }
    }

    fn poll_workers(&mut self) {
        let mut capture_events = Vec::new();
        if let Some(capture) = &mut self.capture {
            while let Some(event) = capture.try_event() {
                capture_events.push(event);
            }
        }
        for event in capture_events {
            match event {
                GuiCaptureEvent::PreflightStarted { timeout_secs } => {
                    self.preflight_wait = Some((Instant::now(), timeout_secs));
                    "Preflight: touch and move either controller pad now."
                        .clone_into(&mut self.status);
                }
                GuiCaptureEvent::Preflight(preflight) => {
                    self.preflight_wait = None;
                    if let Some(reason) = &preflight.invalid_reason {
                        self.error = Some(reason.clone());
                    } else {
                        "Preflight complete. Follow each illustrated trial."
                            .clone_into(&mut self.status);
                    }
                    self.preflight = Some(preflight);
                }
                GuiCaptureEvent::TrialPreview { trial_id, points } => {
                    if self
                        .trials
                        .get(self.trial_index)
                        .is_some_and(|trial| trial.id == trial_id)
                    {
                        self.preview = points;
                    }
                }
                GuiCaptureEvent::Finished(result) => {
                    let completed = result.is_ok();
                    if let Err(error) = result {
                        self.error = Some(error);
                    }
                    if let Some(mut capture) = self.capture.take() {
                        let _ = capture.join();
                    }
                    if self.pending_exit {
                        // The user is leaving the lab; the finalized capture
                        // file is preserved, but do not hold the exit on an
                        // analysis pass or write reports beside it.
                        self.paths = None;
                    } else if let Some(paths) = self.paths.clone() {
                        // A canceled or invalidated capture is still analyzed
                        // for the Results screen, but reports are only written
                        // automatically for a completed one; the explicit
                        // "Write reports beside capture" button remains.
                        self.result_worker = Some(ResultWorker::start(
                            paths,
                            self.selected_profile().clone(),
                            completed,
                        ));
                        self.screen = Screen::Processing;
                        "Analyzing capture and comparing the production mouse engine…"
                            .clone_into(&mut self.status);
                    }
                }
            }
        }
        if let Some(result) = self
            .result_worker
            .as_mut()
            .and_then(ResultWorker::try_result)
        {
            self.result_worker = None;
            match result {
                Ok(results) => {
                    self.results = Some(results);
                    self.screen = Screen::Results;
                    "Analysis complete. Results are descriptive, not a pass/fail verdict."
                        .clone_into(&mut self.status);
                }
                Err(error) => {
                    self.error = Some(error);
                    self.screen = Screen::Landing;
                    "The capture was preserved, but reports could not be produced."
                        .clone_into(&mut self.status);
                }
            }
        }
    }

    fn request_exit(&mut self) -> LabAction {
        if self.capture.is_some() {
            self.pending_exit = true;
            self.cancel_capture();
            LabAction::Stay
        } else if self.result_worker.is_some() {
            self.pending_exit = true;
            LabAction::Stay
        } else {
            LabAction::Exit
        }
    }

    fn selected_profile(&self) -> &BindingProfile {
        &self.profiles[self.selected_profile.min(self.profiles.len() - 1)]
    }

    fn send_capture(&mut self, command: GuiCaptureCommand) -> bool {
        match self.capture.as_ref().map(|capture| capture.send(command)) {
            Some(Ok(())) => true,
            Some(Err(error)) => {
                self.error = Some(error);
                false
            }
            None => false,
        }
    }
}

fn ensure_jsonl_extension(mut path: PathBuf) -> PathBuf {
    if path.extension().is_none() {
        path.set_extension("jsonl");
    }
    path
}

fn status_chip(ui: &mut egui::Ui, ready: bool, label: &str) {
    ui.colored_label(
        if ready { SUCCESS } else { DANGER },
        format!("{} {label}", if ready { "✓" } else { "×" }),
    );
}

fn metric(ui: &mut egui::Ui, label: &str, value: u64, unit: &str) {
    ui.group(|ui| {
        ui.label(egui::RichText::new(label).color(MUTED_TEXT));
        ui.heading(format!("{value} {unit}"));
    });
}

fn metric_f64(ui: &mut egui::Ui, label: &str, value: f64, unit: &str) {
    ui.group(|ui| {
        ui.label(egui::RichText::new(label).color(MUTED_TEXT));
        ui.heading(format!("{value:.3} {unit}"));
    });
}

fn paint_trial(ui: &mut egui::Ui, trial: &GuidedTrial, observed: Option<&[(i64, i64)]>) {
    let size = Vec2::splat(320.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 12.0, SUNKEN);
    let pad = Rect::from_center_size(rect.center(), Vec2::splat(238.0));
    controller_art::draw_trackpad_surface(&painter, pad, PadSide::Right);
    painter.text(
        rect.center_top() + Vec2::new(0.0, 8.0),
        egui::Align2::CENTER_TOP,
        "FRONT OF CONTROLLER ↑",
        egui::FontId::proportional(11.0),
        MUTED_TEXT,
    );
    painter.text(
        rect.center_bottom() - Vec2::new(0.0, 8.0),
        egui::Align2::CENTER_BOTTOM,
        "RIGHT PAD",
        egui::FontId::proportional(11.0),
        MUTED_TEXT,
    );
    match trial.visual {
        TrialVisual::Hold(point) | TrialVisual::Click(point) | TrialVisual::Precision(point) => {
            let position = pad_position(pad, point.normalized());
            let radius = if matches!(trial.visual, TrialVisual::Precision(_)) {
                20.0
            } else {
                11.0
            };
            painter.circle_stroke(position, radius, Stroke::new(3.0, ACCENT));
            painter.circle_filled(position, 6.0, ACCENT);
        }
        TrialVisual::Swipe(direction, _) | TrialVisual::ClickDrag(direction) => {
            let (start, end) = direction.endpoints();
            let start = pad_position(pad, start);
            let end = pad_position(pad, end);
            painter.arrow(start, end - start, Stroke::new(4.0, ACCENT));
            let time = ui.input(|input| input.time) as f32;
            let progress = (time.fract() * 1.2).min(1.0);
            painter.circle_filled(start.lerp(end, progress), 7.0, TEXT);
        }
    }
    if let Some(points) = observed {
        paint_pad_polyline(&painter, pad, points, SUCCESS);
    }
}

fn paint_trajectory(ui: &mut egui::Ui, trajectory: &StageTrajectory) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(600.0, 320.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 8.0, SUNKEN);
    let plot = rect.shrink(22.0);
    painter.line_segment(
        [
            Pos2::new(plot.left(), plot.center().y),
            Pos2::new(plot.right(), plot.center().y),
        ],
        Stroke::new(1.0, DETAIL),
    );
    painter.line_segment(
        [
            Pos2::new(plot.center().x, plot.top()),
            Pos2::new(plot.center().x, plot.bottom()),
        ],
        Stroke::new(1.0, DETAIL),
    );
    paint_pair_polylines(&painter, plot, &trajectory.reference, &trajectory.bridge);
    painter.text(
        rect.left_top() + Vec2::new(10.0, 8.0),
        egui::Align2::LEFT_TOP,
        "Reference",
        egui::FontId::proportional(13.0),
        ACCENT,
    );
    painter.text(
        rect.left_top() + Vec2::new(90.0, 8.0),
        egui::Align2::LEFT_TOP,
        "Bridge",
        egui::FontId::proportional(13.0),
        SUCCESS,
    );
}

fn pad_position(surface: Rect, point: (f32, f32)) -> Pos2 {
    controller_art::trackpad_surface_point(
        surface,
        PadSide::Right,
        [point.0.mul_add(2.0, -1.0), 1.0 - point.1 * 2.0],
    )
}

fn paint_pad_polyline(
    painter: &egui::Painter,
    surface: Rect,
    points: &[(i64, i64)],
    color: Color32,
) {
    if points.len() > 1 {
        let mapped = map_points(surface.shrink(18.0), points, bounds(points, &[]))
            .into_iter()
            .map(|point| {
                let locus = [
                    (point.x - surface.center().x) / (surface.width() * 0.5),
                    (surface.center().y - point.y) / (surface.height() * 0.5),
                ];
                controller_art::trackpad_surface_point(surface, PadSide::Right, locus)
            })
            .collect();
        painter.add(egui::Shape::line(mapped, Stroke::new(2.0, color)));
    }
}

fn paint_pair_polylines(
    painter: &egui::Painter,
    rect: Rect,
    first: &[(i64, i64)],
    second: &[(i64, i64)],
) {
    let bounds = bounds(first, second);
    if first.len() > 1 {
        painter.add(egui::Shape::line(
            map_points(rect, first, bounds),
            Stroke::new(2.0, ACCENT),
        ));
    }
    if second.len() > 1 {
        painter.add(egui::Shape::line(
            map_points(rect, second, bounds),
            Stroke::new(2.0, SUCCESS),
        ));
    }
}

fn bounds(first: &[(i64, i64)], second: &[(i64, i64)]) -> (i64, i64, i64, i64) {
    let mut min_x = 0;
    let mut max_x = 0;
    let mut min_y = 0;
    let mut max_y = 0;
    for &(x, y) in first.iter().chain(second) {
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    (min_x, max_x, min_y, max_y)
}

fn map_points(rect: Rect, points: &[(i64, i64)], bounds: (i64, i64, i64, i64)) -> Vec<Pos2> {
    let (min_x, max_x, min_y, max_y) = bounds;
    let width = (max_x - min_x).max(1) as f32;
    let height = (max_y - min_y).max(1) as f32;
    let scale = (rect.width() / width).min(rect.height() / height) * 0.9;
    let center_x = (min_x + max_x) as f32 / 2.0;
    let center_y = (min_y + max_y) as f32 / 2.0;
    points
        .iter()
        .map(|&(x, y)| {
            Pos2::new(
                rect.center().x + (x as f32 - center_x) * scale,
                rect.center().y + (y as f32 - center_y) * scale,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_is_only_added_when_missing() {
        assert_eq!(
            ensure_jsonl_extension(PathBuf::from("capture")),
            PathBuf::from("capture.jsonl")
        );
        assert_eq!(
            ensure_jsonl_extension(PathBuf::from("capture.data")),
            PathBuf::from("capture.data")
        );
    }
}
