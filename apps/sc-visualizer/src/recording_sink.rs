//! Bounded background recording so JSON serialization and file I/O never run
//! on the UI thread.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use recording::{RecordingEvent, RecordingWriter};

/// More than two seconds of worst-case three-events-per-report traffic at the
/// nominal 250 Hz input rate. Saturation stops recording explicitly rather
/// than silently creating an incomplete capture or blocking rendering.
const QUEUE_CAPACITY: usize = 2_048;
const FLUSH_INTERVAL: Duration = Duration::from_millis(100);

pub(crate) struct RecordingSession {
    sender: Option<SyncSender<RecordingEvent>>,
    result: Receiver<Result<(), String>>,
    worker: Option<JoinHandle<()>>,
}

impl RecordingSession {
    /// Creates the file before launching the worker, so path errors are
    /// reported synchronously by the Start button.
    pub(crate) fn start(path: impl AsRef<Path>) -> Result<Self, String> {
        let file = File::create(path).map_err(|error| error.to_string())?;
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let (result_sender, result) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("sc-visualizer-recording".to_owned())
            .spawn(move || {
                let mut writer = RecordingWriter::new(BufWriter::with_capacity(64 * 1024, file));
                let mut last_flush = Instant::now();
                let outcome = loop {
                    let wait = FLUSH_INTERVAL.saturating_sub(last_flush.elapsed());
                    match receiver.recv_timeout(wait) {
                        Ok(event) => {
                            if let Err(error) = writer.write_event_buffered(&event) {
                                break Err(error.to_string());
                            }
                            // A continuously busy channel never times out, so
                            // check the deadline after writes as well.
                            if last_flush.elapsed() >= FLUSH_INTERVAL {
                                if let Err(error) = writer.flush() {
                                    break Err(error.to_string());
                                }
                                last_flush = Instant::now();
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            if let Err(error) = writer.flush() {
                                break Err(error.to_string());
                            }
                            last_flush = Instant::now();
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            break writer.flush().map_err(|error| error.to_string());
                        }
                    }
                };
                let _ = result_sender.send(outcome);
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            sender: Some(sender),
            result,
            worker: Some(worker),
        })
    }

    pub(crate) fn record(&self, event: RecordingEvent) -> Result<(), String> {
        let sender = self
            .sender
            .as_ref()
            .ok_or_else(|| "recording is stopping".to_owned())?;
        match sender.try_send(event) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(format!(
                "recording queue reached {QUEUE_CAPACITY} events; capture is stopping instead of dropping events silently"
            )),
            Err(TrySendError::Disconnected(_)) => Err("recording worker stopped".to_owned()),
        }
    }

    #[must_use]
    pub(crate) fn is_accepting(&self) -> bool {
        self.sender.is_some()
    }

    /// Stops accepting events without waiting for file I/O. The UI polls the
    /// result while the worker drains every event it already accepted.
    pub(crate) fn request_finish(&mut self) {
        self.sender.take();
    }

    /// Returns a completed worker result without blocking.
    pub(crate) fn poll_result(&self) -> Option<Result<(), String>> {
        match self.result.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                Some(Err("recording worker stopped without a result".to_owned()))
            }
        }
    }

    /// Closes the queue and waits until every accepted event is durable.
    #[cfg(test)]
    pub(crate) fn finish(mut self) -> Result<(), String> {
        self.sender.take();
        if self
            .worker
            .take()
            .is_some_and(|worker| worker.join().is_err())
        {
            return Err("recording worker panicked".to_owned());
        }
        self.result
            .recv()
            .unwrap_or_else(|_| Err("recording worker stopped without a result".to_owned()))
    }
}

impl Drop for RecordingSession {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RecordingSession;
    use recording::{read_events, RecordingEvent, KIND_MARKER};
    use serde_json::json;
    use std::fs::File;
    use std::io::BufReader;

    #[test]
    fn background_sink_preserves_an_nominal_second_of_three_event_reports() {
        let path = std::env::temp_dir().join(format!(
            "sc-visualizer-recording-{}-{}.jsonl",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let session = RecordingSession::start(&path).expect("start recording");
        for index in 0..750_u64 {
            session
                .record(RecordingEvent::new(
                    index,
                    KIND_MARKER,
                    json!({"index": index}),
                ))
                .expect("the nominal event burst fits the bounded queue");
        }
        session.finish().expect("finish recording");
        let events = read_events(BufReader::new(File::open(&path).expect("open capture")))
            .expect("read capture");
        assert_eq!(events.len(), 750);
        std::fs::remove_file(path).expect("remove capture");
    }

    #[test]
    fn requested_finish_rejects_new_events_and_drains_accepted_ones() {
        let path = std::env::temp_dir().join(format!(
            "sc-visualizer-recording-stop-{}.jsonl",
            std::process::id()
        ));
        let mut session = RecordingSession::start(&path).expect("start recording");
        session
            .record(RecordingEvent::new(0, KIND_MARKER, json!({"index": 0})))
            .expect("accept first event");
        session.request_finish();
        assert!(!session.is_accepting());
        assert!(session
            .record(RecordingEvent::new(1, KIND_MARKER, json!({"index": 1})))
            .is_err());
        session.finish().expect("finish recording");
        let events = read_events(BufReader::new(File::open(&path).expect("open capture")))
            .expect("read capture");
        assert_eq!(events.len(), 1);
        std::fs::remove_file(path).expect("remove capture");
    }
}
