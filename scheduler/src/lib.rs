use log::{debug, info};
use serde::Serialize;
use std::time::Instant;
use std::{
    collections::{HashMap, HashSet},
    fs, io,
    io::Read,
    path::PathBuf,
    process::{Child, ChildStderr, Command, Stdio},
};
use thiserror::Error;
use rand::Rng;
use std::io::Write;
use chrono::{prelude::*, TimeDelta};
use hex;

/// Errors that can occur within the scheduler crate
#[derive(Debug, Error)]
pub enum SchedulerError {
    /// I/O related failures (filesystem, process spawn, pipes, etc.)
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    /// Collector returned an inconsistent state
    #[error("Inconsistent state: {0}")]
    InconsistentState(&'static str),
    /// Child process had no stderr pipe attached
    #[error("Child process missing stderr pipe")]
    MissingChildStderr,
    /// Failed HTTP request to backend
    #[error("Backend error: {0}")]
    Backend(#[from] reqwest::Error),
}

/// Convenient result alias for the scheduler crate
pub type Result<T> = std::result::Result<T, SchedulerError>;

/// A trait for collecting targets to be fuzzed by the scheduler.
pub trait TargetsCollector {
    /// Collect the latest fuzzer binaries to be run.
    fn collect_targets(&self) -> HashMap<Sha, PathBuf>;
}

pub type Sha = [u8; 32];

/// A scheduler that manages the lifecycle of fuzzing tasks.
///
/// For now the scheduler does not implement any scheduling algorithm, it just
/// manages the fuzzing tasks.
pub struct Scheduler<C: TargetsCollector> {
    collector: C,
    /// Map of current targets keyed by the hash of the binary
    targets: HashMap<Sha, PathBuf>,
    /// Map of current tasks keyed by the hash of the binary
    tasks: HashMap<Sha, Task>,
    /// Directory where fuzzer corpuses are stored
    corpus_directory: PathBuf,
    /// Directory where fuzzer crashes are stored
    crashes_directory: PathBuf,
    /// URL of the backend to report crashes
    backend_url: String,
    /// Name of the scheduler
    scheduler_name: String,
}

impl<C: TargetsCollector> Scheduler<C> {
    /// Create a new [`Scheduler`].
    pub fn new(collector: C, corpus_directory: PathBuf, crashes_directory: PathBuf, backend_url: String, scheduler_name: String) -> Self {
        Self {
            collector,
            targets: HashMap::new(),
            tasks: HashMap::new(),
            corpus_directory,
            crashes_directory,
            backend_url,
            scheduler_name,
        }
    }

    /// Update the scheduler with the latest targets collected by the collector.
    pub fn update(&mut self, logdir: &str) -> Result<()> {
        let latest_targets = self.collector.collect_targets();

        debug!("latest_targets: {:?}", latest_targets);

        let old_targets_sha = self.targets.keys().collect::<HashSet<_>>();
        let latest_targets_sha = latest_targets.keys().collect::<HashSet<_>>();

        // NOTE: Can be optimized by iterating only once
        let targets_to_stop = old_targets_sha.difference(&latest_targets_sha);
        let targets_to_start = latest_targets_sha.difference(&old_targets_sha);
        let targets_to_check = latest_targets_sha.intersection(&old_targets_sha);

        for &target in targets_to_stop {
            if let Some(mut task) = self.tasks.remove(target) {
                task.child.kill()?;
            } else {
                return Err(SchedulerError::InconsistentState(
                    "task missing during stop",
                ));
            }
        }

        debug!("targets_to_start: {:?}", targets_to_start);
        let mut logfile = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(format!("{}/running_tasks.log", logdir))?;

        for &target in targets_to_start {
            let Some(path) = latest_targets.get(target) else {
                return Err(SchedulerError::InconsistentState(
                    "missing start target path",
                ));
            };
            let fuzzer_path = path.canonicalize()?;
            let task = self.start_task(fuzzer_path)?;
            self.tasks.insert(*target, task);
            let current_date = Local::now().format("%Y-%m-%d-%H-%M-%S").to_string();
            writeln!(logfile, "[{}]{} : 0x{}", current_date, path.display(), hex::encode(target))?;
        }

        for &target in targets_to_check {
            if self.targets.get(target) == latest_targets.get(target) {
                continue;
            }

            if let Some(mut task) = self.tasks.remove(target) {
                task.child.kill()?;
            } else {
                return Err(SchedulerError::InconsistentState(
                    "task missing during restart",
                ));
            }

            let Some(path) = latest_targets.get(target) else {
                return Err(SchedulerError::InconsistentState(
                    "missing restart target path",
                ));
            };
            let fuzzer_path = path.canonicalize()?;
            let task = self.start_task(fuzzer_path)?;
            self.tasks.insert(*target, task);
        }

        debug!("self.tasks: {:?}", self.tasks);

        self.targets = latest_targets;

        Ok(())
    }

    fn start_task(&self, fuzzer_path: PathBuf) -> Result<Task> {
        let fuzzer_name = fuzzer_path.file_name().and_then(|s| s.to_str()).ok_or(
            SchedulerError::InconsistentState("invalid fuzzer path name"),
        )?;
        let fuzzer_corpus = self.corpus_directory.join(fuzzer_name);
        if !fuzzer_corpus.exists() {
            fs::create_dir_all(&fuzzer_corpus)?;
            //TODO : init with a random file here
            let random_file = fuzzer_corpus.join("crash_init");
            let mut file = fs::File::create(random_file)?;
            let mut rng = rand::rng();
            let mut data: [u8; 8192] = [0u8; 8192];
            rng.fill(&mut data);
            file.write_all(&data)?;
        }
        let fuzzer_crashdir = self.crashes_directory.join(fuzzer_name);
        if !fuzzer_crashdir.exists() {
            fs::create_dir_all(&fuzzer_crashdir)?;
        }


        let mut child = Command::new(&fuzzer_path)
            .args(&[
                &fuzzer_corpus.display().to_string(),
                &format!("-artifact_prefix={}{}", fuzzer_crashdir.display().to_string(), "/")
            ])
            .stderr(Stdio::piped())
            .spawn()?;

        let Some(child_stderr) = child.stderr.take() else {
            return Err(SchedulerError::MissingChildStderr);
        };

        Ok(Task {
            child,
            stderr: child_stderr,
            path: fuzzer_path,
            started_at: Instant::now(),
        })
    }

    pub fn report_crashes(&mut self) -> Result<()> {
        let mut tasks_to_remove = Vec::new();

        for (hash, task) in self.tasks.iter_mut() {
            match task.child.try_wait() {
                Ok(None) => {}
                Ok(Some(_)) => {
                    let mut stderr = String::new();
                    task.stderr.read_to_string(&mut stderr)?;
                    //info!("crash stderr: {:?}", stderr);

                    // Get crashing corpus instance
                    let crash = stderr
                        .lines()
                        .filter(|l| l.starts_with("artifact_prefix="))
                        .next()
                        .unwrap_or("Crash detected but no element in stderr");
                    let pattern = "Test unit written to ";
                    let crash_corpus = &crash[crash.find(pattern).unwrap_or(0) + pattern.len()..];
                    let crashing_instance =
                        fs::read(crash_corpus).unwrap_or_else(|_| b"NO CRASH INFO".to_vec());

                    // Send crashing input to the backend
                    let client = reqwest::blocking::Client::new();
                    let binary = fs::read(task.path.clone())?;
                    let _res = client
                        .post(format!("{}/crashes", self.backend_url))
                        .json(&CrashReport {
                            report: stderr.as_bytes().to_vec(),
                            input: crashing_instance,
                            filename: task.path.display().to_string(),
                            program: binary,
                            scheduler_name: self.scheduler_name.clone(),
                        })
                        .send()?;
                    tasks_to_remove.push(*hash);
                }
                Err(e) => return Err(SchedulerError::Io(e)),
            }
        }

        for task_to_remove in tasks_to_remove.iter() {
            self.tasks.remove(task_to_remove);
        }
        Ok(())
    }

    pub fn running_tasks_info(&self) -> Vec<(Sha, PathBuf, std::time::Duration)> {
        self.tasks
            .iter()
            .map(|(sha, task)| (*sha, task.path.clone(), task.started_at.elapsed()))
            .collect()
    }

    pub fn log_running_tasks(&self) {
        if self.tasks.is_empty() {
            info!("No tasks running");
            return;
        }
        info!("Running tasks:");
        for (sha, task) in self.tasks.iter() {
            let secs = task.started_at.elapsed().as_secs();
            let d = TimeDelta::new(secs as i64, 0).expect("Failed to create TimeDelta");
            info!(
                "- '{}:0x{}' has been running for {:?}",
                task.path.display(),
                hex::encode(sha),
                d.to_std()
            );
        }
    }
}

/// A fuzzing task to be scheduled by the [`Scheduler`]
#[derive(Debug)]
struct Task {
    /// Path of the fuzzer binary
    path: PathBuf,
    /// The fuzzer process
    child: Child,
    /// The stderr of the fuzzer process
    stderr: ChildStderr,
    started_at: Instant,
}

#[derive(Serialize, Clone)]
struct CrashReport {
    report: Vec<u8>,
    input: Vec<u8>,
    filename: String,
    program: Vec<u8>,
    scheduler_name: String,
}
