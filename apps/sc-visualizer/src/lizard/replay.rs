#![allow(clippy::cast_precision_loss)] // Dump output reports fractional replay microseconds.

use std::thread;
use std::time::Duration;

use desktop_bindings::{DesktopInputSink, KeyboardKey, Modifier, MouseButton};

use crate::lizard::compare::{bridge_motion, load_profile};
use crate::lizard::trace::{Motion, Trace};
use crate::lizard::{ReplayOutput, ReplaySource};

pub(crate) fn run(
    trace: &Trace,
    source: ReplaySource,
    output: ReplayOutput,
    speed: f64,
) -> Result<(), String> {
    if !speed.is_finite() || speed <= 0.0 {
        return Err(format!(
            "replay speed must be finite and positive, got {speed}"
        ));
    }
    let motion = match source {
        ReplaySource::Reference => trace.reference_motion(),
        ReplaySource::Bridge => bridge_motion(trace, load_profile(None, None)?)?,
    };
    if motion.is_empty() {
        return Err("selected replay source contains no pointer motion".to_owned());
    }
    match output {
        ReplayOutput::Dump => {
            dump(&motion, source, speed);
            Ok(())
        }
        ReplayOutput::Desktop => inject(&motion, speed),
    }
}

fn dump(motion: &[Motion], source: ReplaySource, speed: f64) {
    let label = match source {
        ReplaySource::Reference => "reference",
        ReplaySource::Bridge => "bridge",
    };
    for item in motion {
        println!(
            "{:>12} us source={label} dx={:>4} dy={:>4} replay_at={:.3} us",
            item.timestamp_us,
            item.x,
            item.y,
            item.timestamp_us as f64 / speed
        );
    }
}

fn inject(motion: &[Motion], speed: f64) -> Result<(), String> {
    let mut sink = pointer_only_desktop_sink()?;
    let mut previous = motion[0].timestamp_us;
    for item in motion {
        let delay_us = item.timestamp_us.saturating_sub(previous);
        let delay = Duration::from_secs_f64(Duration::from_micros(delay_us).as_secs_f64() / speed);
        if !delay.is_zero() {
            thread::sleep(delay);
        }
        sink.mouse_move(item.x, item.y)?;
        previous = item.timestamp_us;
    }
    Ok(())
}

fn pointer_only_desktop_sink() -> Result<Box<dyn DesktopInputSink>, String> {
    let mut factory = desktop_input::current_factory()
        .map_err(|_| "desktop replay is implemented only on macOS; use --output dump".to_owned())?;
    let session = factory.detect_session()?;
    factory
        .create(&session)
        .map(PointerOnlySink)
        .map(|sink| Box::new(sink) as Box<dyn DesktopInputSink>)
}

struct PointerOnlySink(Box<dyn DesktopInputSink>);

impl DesktopInputSink for PointerOnlySink {
    fn key(&mut self, _key: KeyboardKey, _pressed: bool) -> Result<(), String> {
        Ok(())
    }

    fn modifier(&mut self, _modifier: Modifier, _pressed: bool) -> Result<(), String> {
        Ok(())
    }

    fn mouse_button(&mut self, _button: MouseButton, _pressed: bool) -> Result<(), String> {
        Ok(())
    }

    fn mouse_move(&mut self, x: i32, y: i32) -> Result<(), String> {
        self.0.mouse_move(x, y)
    }

    fn scroll(&mut self, _x: i32, _y: i32) -> Result<(), String> {
        Ok(())
    }

    fn flush(&mut self) -> Result<(), String> {
        self.0.flush()
    }
}
