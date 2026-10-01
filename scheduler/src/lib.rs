// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

/// Background execution of the build script
pub mod build;
/// Coverage analysis module
mod coverage;
#[cfg(test)]
mod tests;
/// Security validation module for input sanitization
pub mod validation;

mod backend;
pub mod collector;
mod coverage_ops;
mod process;
mod state;
mod tasks;
pub mod types;

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use std::{fs, io};

use log::info;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use collector::{DirectoryTargetsCollector, Sha, TargetsCollector};
pub use coverage::COVERAGE_SOURCE_UNAVAILABLE_MARKER;
pub use coverage::{CoverageMetricEntry, CoverageSummary};
pub use types::{
    FuzzerStateCommand, FuzzerStateEntry, StateFile, Task, TaskStatus, TaskStatusInfo,
};

use collector::Sha as ShaInner;
use process::install_ctrlc_handler;
use types::Task as TaskInner;

/// Convenient result alias for the scheduler crate
pub type Result<T> = std::result::Result<T, SchedulerError>;

/// Errors that can occur within the scheduler crate
#[derive(Debug, Error)]
pub enum SchedulerError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("Inconsistent state: {0}")]
    InconsistentState(&'static str),
    #[error("Child process missing stderr pipe")]
    MissingChildStderr,
    #[error("Backend error: {0}")]
    Backend(#[from] reqwest::Error),
    #[error("Container binary path not specified")]
    MissingContainerBinary,
    #[error("Invalid input: {0}")]
    InvalidInput(String),
    #[error("Path traversal detected: {0}")]
    PathTraversal(String),
}

#[derive(Serialize, Deserialize, Clone)]
pub struct SchedulerConfig {
    pub scheduler_name: String,
    pub targets_directory: Vec<String>,
    pub build_script: String,
    pub build_script_interval_hours: Option<u64>,
    pub backend_url: String,
    pub corpus_directory: String,
    pub crashes_directory: String,
    pub coverage_directory: String,
    pub container_image: Option<String>,
    pub container_options: Option<Vec<String>>,
    pub has_coverage: bool,
    /// Directory containing `llvm-cov` and `llvm-profdata` (optional).
    pub llvm_bin_directory: Option<String>,
    pub state_file: Option<String>,
    /// Minutes between two periodic coverage snapshots. Omitted means
    /// [`DEFAULT_SNAPSHOT_INTERVAL_MINUTES`]; `0` keeps the historical
    /// behaviour of only snapshotting on build transitions.
    #[serde(default)]
    pub coverage_snapshot_interval_minutes: Option<u64>,
}

/// Default cadence of the periodic coverage snapshot.
pub const DEFAULT_SNAPSHOT_INTERVAL_MINUTES: u64 = 60;

/// How long after startup the first periodic snapshot may run.
const FIRST_SNAPSHOT_DELAY: Duration = Duration::from_secs(300);

/// Merged profile of an interactive project-wide coverage query.
const PROJECT_PROFDATA: &str = "project.profdata";

/// Merged profile of a background snapshot.
const SNAPSHOT_PROFDATA: &str = "snapshot.profdata";

/// How long a fuzzer is given to acknowledge the `SIGUSR1` profile flush.
const COVERAGE_FLUSH_TIMEOUT: Duration = Duration::from_secs(10);

/// Upper bound on the status heartbeat timeout.
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(10);

/// Turn the configured minutes into a snapshot cadence, `None` meaning disabled.
fn snapshot_interval_from(minutes: Option<u64>) -> Option<Duration> {
    match minutes.unwrap_or(DEFAULT_SNAPSHOT_INTERVAL_MINUTES) {
        0 => None,
        m => Some(Duration::from_secs(m.saturating_mul(60))),
    }
}

/// A scheduler that manages the lifecycle of fuzzing tasks.
pub struct Scheduler<C: TargetsCollector> {
    pub(crate) collector: C,
    pub(crate) targets: HashMap<ShaInner, PathBuf>,
    pub(crate) tasks: HashMap<ShaInner, TaskInner>,
    pub(crate) corpus_directory: PathBuf,
    pub(crate) crashes_directory: PathBuf,
    pub(crate) coverage_directory: PathBuf,
    pub(crate) backend_url: String,
    pub(crate) scheduler_name: String,
    pub(crate) container_image: Option<String>,
    pub(crate) container_binary: Option<PathBuf>,
    pub(crate) container_options: Option<Vec<String>>,
    pub(crate) has_coverage: bool,
    pub(crate) llvm_bin_directory: Option<PathBuf>,
    pub(crate) state_file_path: Option<PathBuf>,
    pub(crate) state_file_lock: Arc<RwLock<()>>,
    pub(crate) snapshot_interval: Option<Duration>,
    pub(crate) started_at: Instant,
    pub(crate) last_snapshot_at: Option<Instant>,
    pub(crate) snapshot_running: Arc<AtomicBool>,
}

impl<C: TargetsCollector> Scheduler<C> {
    /// Get the path to the state file
    pub fn state_file_path(&self) -> Option<&PathBuf> {
        self.state_file_path.as_ref()
    }

    /// Create a new [`Scheduler`].
    ///
    /// # Panics
    ///
    /// Panics if `backend_url` does not point to localhost.
    pub fn new(collector: C, config: SchedulerConfig, container_binary: Option<PathBuf>) -> Self {
        if let Err(e) = validation::validate_backend_url(&config.backend_url) {
            panic!("Security: {e}");
        }

        install_ctrlc_handler();

        let state_file_path = config.state_file.map(PathBuf::from).unwrap_or_else(|| {
            let state_dir = PathBuf::from(format!(
                "{}/.fuzzwizard",
                std::env::var("HOME").unwrap_or_else(|_| "/var/lib/fuzzwizard".to_string())
            ));
            state_dir.join(format!("{}_state.json", config.scheduler_name))
        });

        if let Some(parent) = state_file_path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                info!(
                    "Failed to create state file directory {}: {}",
                    parent.display(),
                    e
                );
            } else {
                let mut perms = fs::metadata(parent)
                    .map(|m| m.permissions())
                    .unwrap_or_else(|_| fs::Permissions::from_mode(0o700));
                perms.set_mode(0o700);
                let _ = fs::set_permissions(parent, perms);
            }
        }

        let empty_state = StateFile {
            fuzzers: Vec::new(),
        };
        if let Ok(json) = serde_json::to_string_pretty(&empty_state) {
            if let Err(e) = fs::write(&state_file_path, json) {
                info!(
                    "Failed to create state file {}: {}",
                    state_file_path.display(),
                    e
                );
            } else {
                info!("Created empty state file: {}", state_file_path.display());
            }
        }

        Self {
            collector,
            targets: HashMap::new(),
            tasks: HashMap::new(),
            corpus_directory: config.corpus_directory.into(),
            crashes_directory: config.crashes_directory.into(),
            backend_url: config.backend_url,
            scheduler_name: config.scheduler_name,
            container_image: config.container_image,
            container_options: config.container_options,
            container_binary,
            coverage_directory: config.coverage_directory.into(),
            has_coverage: config.has_coverage,
            llvm_bin_directory: config.llvm_bin_directory.map(PathBuf::from),
            state_file_path: Some(state_file_path),
            state_file_lock: Arc::new(RwLock::new(())),
            snapshot_interval: snapshot_interval_from(config.coverage_snapshot_interval_minutes),
            started_at: Instant::now(),
            last_snapshot_at: None,
            snapshot_running: Arc::new(AtomicBool::new(false)),
        }
    }
}
