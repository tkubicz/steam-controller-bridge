//! Background capture loading, report generation, and GUI-ready trajectories.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use desktop_bindings::{default_store_path, load_store, BindingProfile, DEFAULT_PROFILE_NAME};

use super::analysis::{analyze, AnalysisReport};
use super::compare::{compare_with_profile, mouse_only_profile, ComparisonReport};
use super::trace::{Motion, Trace};
use super::write_json;

#[derive(Debug, Clone)]
pub(crate) struct ArtifactPaths {
    pub(crate) capture: PathBuf,
    pub(crate) analysis: PathBuf,
    pub(crate) comparison: PathBuf,
}

impl ArtifactPaths {
    pub(crate) fn from_capture(capture: PathBuf) -> Self {
        let parent = capture.parent().unwrap_or_else(|| Path::new(""));
        let stem = capture
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|stem| !stem.is_empty())
            .unwrap_or("lizard");
        Self {
            analysis: parent.join(format!("{stem}-analysis.json")),
            comparison: parent.join(format!("{stem}-comparison.json")),
            capture,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct StageTrajectory {
    pub(crate) name: String,
    pub(crate) reference: Vec<(i64, i64)>,
    pub(crate) bridge: Vec<(i64, i64)>,
}

#[derive(Debug)]
pub(crate) struct LabResults {
    pub(crate) paths: ArtifactPaths,
    pub(crate) reports_written: bool,
    pub(crate) report_write_error: Option<String>,
    pub(crate) analysis: AnalysisReport,
    pub(crate) comparison: ComparisonReport,
    pub(crate) trajectories: Vec<StageTrajectory>,
    pub(crate) accepted_attempts: usize,
    pub(crate) discarded_attempts: usize,
}

pub(crate) struct ResultWorker {
    receiver: mpsc::Receiver<Result<LabResults, String>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl ResultWorker {
    pub(crate) fn start(
        paths: ArtifactPaths,
        profile: BindingProfile,
        write_reports: bool,
    ) -> Self {
        let (sender, receiver) = mpsc::channel();
        let worker = thread::spawn(move || {
            let _ = sender.send(load_results(paths, profile, write_reports));
        });
        Self {
            receiver,
            worker: Some(worker),
        }
    }

    pub(crate) fn try_result(&mut self) -> Option<Result<LabResults, String>> {
        match self.receiver.try_recv() {
            Ok(result) => Some(self.join().and(result)),
            Err(mpsc::TryRecvError::Disconnected) if self.worker.is_some() => {
                Some(self.join().and_then(|()| {
                    Err("result worker stopped without returning an analysis".to_owned())
                }))
            }
            Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => None,
        }
    }

    fn join(&mut self) -> Result<(), String> {
        self.worker.take().map_or(Ok(()), |worker| {
            worker
                .join()
                .map_err(|_| "result worker panicked".to_owned())
        })
    }
}

impl Drop for ResultWorker {
    fn drop(&mut self) {
        let _ = self.join();
    }
}

pub(crate) fn available_profiles() -> (Vec<BindingProfile>, Option<String>) {
    let path = match default_store_path() {
        Ok(path) => path,
        Err(error) => return (vec![BindingProfile::default()], Some(error)),
    };
    match load_store(&path) {
        Ok(store) => (store.profiles, None),
        Err(error) => (
            vec![BindingProfile::default()],
            Some(format!(
                "Using the built-in {DEFAULT_PROFILE_NAME} profile: {error}"
            )),
        ),
    }
}

fn load_results(
    paths: ArtifactPaths,
    profile: BindingProfile,
    write_reports: bool,
) -> Result<LabResults, String> {
    let trace = Trace::read(&paths.capture)?;
    let analysis = analyze(&trace);
    let reference = trace.reference_motion();
    let (comparison, candidate) = compare_with_profile(&trace, mouse_only_profile(profile))?;
    let trajectories =
        trace
            .guided_stages()
            .into_iter()
            .map(|stage| StageTrajectory {
                name: stage.name,
                reference: cumulative_points(reference.iter().filter(|motion| {
                    (stage.start_us..=stage.end_us).contains(&motion.timestamp_us)
                })),
                bridge: cumulative_points(candidate.iter().filter(|motion| {
                    (stage.start_us..=stage.end_us).contains(&motion.timestamp_us)
                })),
            })
            .collect();
    let report_write_error = write_reports
        .then(|| write_report_pair(&paths, &analysis, &comparison))
        .and_then(Result::err);
    let (accepted_attempts, discarded_attempts) = trace.guided_attempt_counts();
    Ok(LabResults {
        paths,
        reports_written: write_reports && report_write_error.is_none(),
        report_write_error,
        analysis,
        comparison,
        trajectories,
        accepted_attempts,
        discarded_attempts,
    })
}

pub(crate) fn write_reports(results: &LabResults) -> Result<(), String> {
    write_report_pair(&results.paths, &results.analysis, &results.comparison)
}

fn write_report_pair(
    paths: &ArtifactPaths,
    analysis: &AnalysisReport,
    comparison: &ComparisonReport,
) -> Result<(), String> {
    write_json(&paths.analysis, analysis)?;
    write_json(&paths.comparison, comparison)
}

fn cumulative_points<'a>(motion: impl Iterator<Item = &'a Motion>) -> Vec<(i64, i64)> {
    let mut point = (0_i64, 0_i64);
    let mut points = vec![point];
    for item in motion {
        point.0 += i64::from(item.x);
        point.1 += i64::from(item.y);
        points.push(point);
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process;

    #[test]
    fn report_paths_are_derived_beside_the_capture() {
        let paths = ArtifactPaths::from_capture(PathBuf::from("captures/run.jsonl"));
        assert_eq!(paths.analysis, PathBuf::from("captures/run-analysis.json"));
        assert_eq!(
            paths.comparison,
            PathBuf::from("captures/run-comparison.json")
        );
    }

    #[test]
    fn trajectory_points_start_at_the_stage_origin() {
        let motion = [
            Motion {
                timestamp_us: 1,
                x: 3,
                y: -2,
            },
            Motion {
                timestamp_us: 2,
                x: -1,
                y: 4,
            },
        ];
        assert_eq!(cumulative_points(motion.iter()), [(0, 0), (3, -2), (2, 2)]);
    }

    #[test]
    fn report_write_failures_remain_retryable_errors() {
        let missing_parent = std::env::temp_dir().join(format!(
            "sc-visualizer-missing-report-dir-{}",
            process::id()
        ));
        let error = write_json(
            &missing_parent.join("analysis.json"),
            &serde_json::json!({}),
        )
        .expect_err("a missing parent must not silently discard the report");
        assert!(error.contains("cannot write"));
    }

    #[test]
    fn a_panicked_result_worker_becomes_a_visible_error() {
        let (sender, receiver) = mpsc::channel();
        let (ready, wait_until_disconnected) = mpsc::channel();
        let worker = thread::spawn(move || {
            drop(sender);
            ready.send(()).unwrap();
            panic!("simulated analysis failure");
        });
        wait_until_disconnected.recv().unwrap();
        let mut result_worker = ResultWorker {
            receiver,
            worker: Some(worker),
        };

        let result = result_worker
            .try_result()
            .expect("a disconnected worker must produce a result");
        assert_eq!(result.unwrap_err(), "result worker panicked");
        assert!(result_worker.worker.is_none());
    }
}
