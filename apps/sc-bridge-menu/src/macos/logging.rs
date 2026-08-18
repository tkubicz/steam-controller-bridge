use super::*;

pub(super) struct StatusLogger {
    pub(super) directory: PathBuf,
    pub(super) path: PathBuf,
    pub(super) started: Instant,
    pub(super) tracker: StatusLogTracker,
    pub(super) pending_batch: Option<String>,
}

impl StatusLogger {
    pub(super) fn new() -> Result<Self, String> {
        let paths = app_paths::current()
            .map_err(|error| format!("cannot locate the user log directory: {error}"))?;
        let path = paths.status_log_file();
        let directory = paths.log_dir;
        fs::create_dir_all(&directory).map_err(|error| {
            format!(
                "cannot create log directory '{}': {error}",
                directory.display()
            )
        })?;
        Ok(Self {
            directory,
            path,
            started: Instant::now(),
            tracker: StatusLogTracker::default(),
            pending_batch: None,
        })
    }

    pub(super) fn write_status(&mut self, status: &BridgeStatus) -> Result<(), String> {
        self.flush_pending()?;
        let records = self.tracker.observe(self.started.elapsed(), status);
        if records.is_empty() {
            return Ok(());
        }
        self.write_records(&records, unix_timestamp())
    }

    #[cfg(test)]
    pub(super) fn write_status_at(
        &mut self,
        status: &BridgeStatus,
        elapsed: Duration,
        timestamp: u64,
    ) -> Result<(), String> {
        self.flush_pending()?;
        let records = self.tracker.observe(elapsed, status);
        if records.is_empty() {
            return Ok(());
        }
        self.write_records(&records, timestamp)
    }

    pub(super) fn write_records(
        &mut self,
        records: &[StatusLogRecord],
        timestamp: u64,
    ) -> Result<(), String> {
        let mut batch = String::new();
        for record in records {
            let _ = writeln!(batch, "timestamp={timestamp} {record}");
        }
        self.write_batch(&batch)
    }

    pub(super) fn write_diagnostics(&mut self, diagnostics: &[String]) -> Result<(), String> {
        if diagnostics.is_empty() {
            return Ok(());
        }
        let timestamp = unix_timestamp();
        let mut batch = String::new();
        for diagnostic in diagnostics {
            let _ = writeln!(batch, "timestamp={timestamp} {diagnostic}");
        }
        self.write_batch(&batch)
    }

    fn write_batch(&mut self, batch: &str) -> Result<(), String> {
        let mut combined = self.pending_batch.take().unwrap_or_default();
        combined.push_str(batch);
        let batch = bounded_log_batch(combined);
        if let Err(error) = write_log_batch(&self.path, &batch) {
            self.pending_batch = Some(batch);
            return Err(error);
        }
        Ok(())
    }

    pub(super) fn flush_pending(&mut self) -> Result<(), String> {
        if self.pending_batch.is_none() {
            return Ok(());
        }
        self.write_batch("")
    }
}

pub(super) fn write_log_batch(path: &Path, batch: &str) -> Result<(), String> {
    rotate_log(path, batch.len() as u64)?;
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    log.write_all(batch.as_bytes())
        .map_err(|error| error.to_string())
}

pub(super) fn bounded_log_batch(mut batch: String) -> String {
    // Writing a log line must not be able to panic. A limit that does not fit
    // usize cannot be exceeded by an in-memory batch anyway, so saturating to
    // usize::MAX simply means "never truncate" on such a platform.
    let limit = usize::try_from(LOG_LIMIT_BYTES).unwrap_or(usize::MAX);
    if batch.len() <= limit {
        return batch;
    }
    let mut end = limit.saturating_sub(LOG_TRUNCATION_MARKER.len());
    while !batch.is_char_boundary(end) {
        end -= 1;
    }
    batch.truncate(end);
    batch.push_str(LOG_TRUNCATION_MARKER);
    batch
}

pub(super) fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(super) fn rotate_log(path: &Path, incoming_bytes: u64) -> Result<(), String> {
    let Ok(metadata) = path.metadata() else {
        return Ok(());
    };
    if metadata.len() == 0 || metadata.len().saturating_add(incoming_bytes) <= LOG_LIMIT_BYTES {
        return Ok(());
    }
    let rotated = path.with_extension("log.1");
    if rotated.exists() {
        fs::remove_file(&rotated).map_err(|error| error.to_string())?;
    }
    fs::rename(path, rotated).map_err(|error| error.to_string())
}

pub(super) fn diagnostics_text(status: &BridgeStatus) -> String {
    format_status_diagnostics(status)
}

pub(super) fn copy_diagnostics(status: &BridgeStatus) -> Result<(), String> {
    copy_text(&diagnostics_text(status))
}
