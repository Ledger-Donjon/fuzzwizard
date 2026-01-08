// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: GPL-3.0-only

use chrono::{TimeDelta, prelude::*};
use humantime::format_duration;
use log::{debug, info, warn};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use std::{
    collections::{HashMap, HashSet},
    fs, io,
    io::Read,
    path::PathBuf,
    process::{Child, ChildStderr, Command, Stdio},
    sync::mpsc,
    thread,
};
use thiserror::Error;

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
    #[error("Container binary path not specified")]
    MissingContainerBinary,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct SchedulerConfig {
    pub scheduler_name: String,
    pub targets_directory: Vec<String>,
    pub build_script: String,
    pub backend_url: String,
    pub corpus_directory: String,
    pub crashes_directory: String,
    pub coverage_directory: String,
    pub container_image: Option<String>,
    pub container_options: Option<Vec<String>>,
    pub has_coverage: bool,
    pub state_file: Option<String>,
}

/// Represents the state of a single fuzzer in the state file
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum FuzzerStateCommand {
    /// User command to start the fuzzer
    START,
    /// User command to stop the fuzzer
    STOP,
    /// User command to restart the fuzzer
    RESTART,
    /// Automatic state: fuzzer is running
    RUNNING,
    /// Automatic state: fuzzer has crashed
    CRASHED,
    /// Automatic state: fuzzer has been stopped
    STOPPED,
}

/// Represents a fuzzer entry in the state file
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FuzzerStateEntry {
    pub path: String,
    pub sha: String,
    pub state: FuzzerStateCommand,
    /// Container name if the fuzzer is running in a container (for easy attachment)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container_name: Option<String>,
    /// Environment variables to set when starting the fuzzer
    #[serde(skip_serializing_if = "Option::is_none")]
    pub var_env: Option<HashMap<String, String>>,
    /// Command line options to pass to the fuzzer
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cmd_options: Option<Vec<String>>,
}

/// Represents the complete state file
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StateFile {
    pub fuzzers: Vec<FuzzerStateEntry>,
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
    /// Directory where fuzzer coverage are stored
    coverage_directory: PathBuf,
    /// URL of the backend to report crashes
    backend_url: String,
    /// Name of the scheduler
    scheduler_name: String,
    container_image: Option<String>,
    container_binary: Option<PathBuf>,
    container_options: Option<Vec<String>>,
    has_coverage: bool,
    /// Path to the state file (if configured)
    state_file_path: Option<PathBuf>,
}

#[derive(Serialize, Clone)]
struct CrashReport {
    report: Vec<u8>,
    input: Vec<u8>,
    filename: String,
    program: Vec<u8>,
    scheduler_name: String,
}

#[derive(Clone, Debug)]
struct ProcessRecord {
    pid: u32,
    container_name: Option<String>,
    fuzzer_name: String,
}

/// Status of a task
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Running,
    Crashed,
    Stopped,
}

/// Information about a task's status and timestamps (without the process handle)
#[derive(Debug, Clone)]
pub struct TaskStatusInfo {
    /// SHA hash of the fuzzer binary
    pub sha: Sha,
    /// Path of the fuzzer binary
    pub path: PathBuf,
    /// Status of the task
    pub status: TaskStatus,
    /// Timestamp when the task started
    pub started_at: Instant,
    /// Timestamp when the task crashed (if applicable)
    pub crashed_at: Option<Instant>,
    /// Timestamp when the task was stopped (if applicable)
    pub stopped_at: Option<Instant>,
}

/// A fuzzing task to be scheduled by the [`Scheduler`]
#[derive(Debug)]
pub struct Task {
    /// Path of the fuzzer binary
    path: PathBuf,
    /// The fuzzer process
    child: Child,
    /// The stderr of the fuzzer process
    stderr: ChildStderr,
    started_at: Instant,
    container_name: Option<String>,
    /// Status of the task (running, crashed, or stopped)
    pub status: TaskStatus,
    /// Timestamp when the task crashed (if applicable)
    crashed_at: Option<Instant>,
    /// Timestamp when the task was stopped (if applicable)
    stopped_at: Option<Instant>,
}

static PROCESS_REGISTRY: OnceLock<Arc<Mutex<Vec<ProcessRecord>>>> = OnceLock::new();

fn install_ctrlc_handler() {
    let registry = PROCESS_REGISTRY
        .get_or_init(|| Arc::new(Mutex::new(Vec::new())))
        .clone();
    let _ = ctrlc::set_handler(move || {
        if let Ok(records) = registry.lock() {
            for rec in records.iter() {
                if let Some(name) = &rec.container_name {
                    // Try docker first, then podman (non-blocking spawn)
                    let name_clone = name.clone();
                    let _ = Command::new("docker")
                        .args(["rm", "-f", &name_clone])
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status()
                        .or_else(|_| {
                            // Fallback to podman
                            Command::new("podman")
                                .args(["rm", "-f", &name_clone])
                                .stdout(std::process::Stdio::null())
                                .stderr(std::process::Stdio::null())
                                .status()
                        });
                };
                // Spawn kill command non-blocking
                let pid = rec.pid;
                let _ = std::thread::spawn(move || {
                    let _ = Command::new("kill")
                        .args(["-TERM", &pid.to_string()])
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn()
                        .and_then(|mut child| child.wait());
                });
            }
        }
        // Give a small delay for commands to start, then exit
        std::thread::sleep(std::time::Duration::from_millis(100));
        std::process::exit(130);
    });
}

impl<C: TargetsCollector> Scheduler<C> {
    /// Get the path to the state file
    pub fn state_file_path(&self) -> Option<&PathBuf> {
        self.state_file_path.as_ref()
    }

    /// Create a new [`Scheduler`].
    pub fn new(collector: C, config: SchedulerConfig, container_binary: Option<PathBuf>) -> Self {
        // Ensure Ctrl-C handler is installed once
        install_ctrlc_handler();

        // Determine state file path: use config value or default to /tmp/{scheduler_name}_state.json
        let state_file_path = config
            .state_file
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(format!("/tmp/{}_state.json", config.scheduler_name)));

        // Create empty state file at startup (overwrite any existing file)
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
            state_file_path: Some(state_file_path),
        }
    }

    /// Read the state file and return the state entries
    fn read_state_file(&self) -> Result<Option<StateFile>> {
        let Some(ref state_path) = self.state_file_path else {
            return Ok(None);
        };

        // Try to read the file
        let contents = match fs::read_to_string(state_path) {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // File doesn't exist yet, create empty one
                let empty_state = StateFile {
                    fuzzers: Vec::new(),
                };
                if let Ok(json) = serde_json::to_string_pretty(&empty_state) {
                    let _ = fs::write(state_path, json);
                }
                return Ok(Some(empty_state));
            }
            Err(e) => {
                // Other error, log and return empty state
                info!(
                    "Failed to read state file {}: {}, using empty state",
                    state_path.display(),
                    e
                );
                return Ok(Some(StateFile {
                    fuzzers: Vec::new(),
                }));
            }
        };

        // Try to parse JSON
        match serde_json::from_str::<StateFile>(&contents) {
            Ok(state) => Ok(Some(state)),
            Err(e) => {
                // File is corrupted or invalid, log and return empty state
                info!(
                    "Failed to parse state file {}: {}, using empty state",
                    state_path.display(),
                    e
                );
                Ok(Some(StateFile {
                    fuzzers: Vec::new(),
                }))
            }
        }
    }

    /// Write the state file with current fuzzer states
    fn write_state_file(&self) -> Result<()> {
        let Some(ref state_path) = self.state_file_path else {
            return Ok(());
        };

        // Ensure parent directory exists
        if let Some(parent) = state_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Read existing state file to preserve var_env and cmd_options
        let existing_state = self.read_state_file()?;
        let mut existing_entries: HashMap<String, &FuzzerStateEntry> = HashMap::new();
        if let Some(ref state) = existing_state {
            for entry in &state.fuzzers {
                existing_entries.insert(entry.sha.clone(), entry);
            }
        }

        // Build state from current tasks
        let mut fuzzers = Vec::new();
        for (sha, task) in &self.tasks {
            let state = match task.status {
                TaskStatus::Running => FuzzerStateCommand::RUNNING,
                TaskStatus::Crashed => FuzzerStateCommand::CRASHED,
                TaskStatus::Stopped => FuzzerStateCommand::STOPPED,
            };
            let sha_str = hex::encode(sha);
            // Preserve var_env and cmd_options from existing entry if present
            let existing_entry = existing_entries.get(&sha_str);
            fuzzers.push(FuzzerStateEntry {
                path: task.path.display().to_string(),
                sha: sha_str,
                state,
                container_name: task.container_name.clone(),
                var_env: existing_entry.and_then(|e| e.var_env.clone()),
                cmd_options: existing_entry.and_then(|e| e.cmd_options.clone()),
            });
        }

        // Also include targets that are not yet started
        for (sha, path) in &self.targets {
            if !self.tasks.contains_key(sha) {
                let sha_str = hex::encode(sha);
                // Preserve var_env and cmd_options from existing entry if present
                let existing_entry = existing_entries.get(&sha_str);
                fuzzers.push(FuzzerStateEntry {
                    path: path.display().to_string(),
                    sha: sha_str,
                    state: FuzzerStateCommand::STOPPED,
                    container_name: None,
                    var_env: existing_entry.and_then(|e| e.var_env.clone()),
                    cmd_options: existing_entry.and_then(|e| e.cmd_options.clone()),
                });
            }
        }

        let state_file = StateFile { fuzzers };

        // Write atomically: write to temp file then rename
        let temp_path = state_path.with_extension("tmp");
        let json = serde_json::to_string_pretty(&state_file).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Failed to serialize state: {}", e),
            )
        })?;
        fs::write(&temp_path, json)?;
        fs::rename(&temp_path, state_path)?;

        Ok(())
    }

    /// Apply user commands from the state file
    fn apply_state_file_commands(&mut self, state_file: &StateFile, logdir: &str) -> Result<()> {
        // Create a map of SHA -> entry for quick lookup
        let mut entries: HashMap<String, &FuzzerStateEntry> = HashMap::new();
        for entry in &state_file.fuzzers {
            entries.insert(entry.sha.clone(), entry);
        }

        // Process each command
        for (sha_str, entry) in &entries {
            // Parse SHA from hex string
            let sha_bytes = match hex::decode(sha_str) {
                Ok(bytes) if bytes.len() == 32 => {
                    let mut sha: Sha = [0u8; 32];
                    sha.copy_from_slice(&bytes);
                    sha
                }
                _ => {
                    debug!("Invalid SHA in state file: {}", sha_str);
                    continue;
                }
            };

            match entry.state {
                FuzzerStateCommand::START => {
                    // Start the fuzzer if it's STOPPED or CRASHED (not RUNNING)
                    let should_start = if let Some(task) = self.tasks.get(&sha_bytes) {
                        task.status != TaskStatus::Running
                    } else {
                        true // Not in tasks, so we should start it
                    };

                    if should_start {
                        // Find the path for this SHA
                        if let Some(path) = self.targets.get(&sha_bytes) {
                            info!(
                                "Starting fuzzer from state file: {} (0x{})",
                                path.display(),
                                sha_str
                            );
                            let fuzzer_path = path.canonicalize()?;
                            let var_env = entry.var_env.as_ref();
                            let cmd_options = entry.cmd_options.as_ref();
                            let task = self.start_task(fuzzer_path, var_env, cmd_options)?;
                            self.tasks.insert(sha_bytes, task);
                            let mut logfile = fs::OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open(format!("{}/running_tasks.log", logdir))?;
                            let current_date = Local::now().format("%Y-%m-%d-%H-%M-%S").to_string();
                            writeln!(
                                logfile,
                                "[{}]{} : 0x{}",
                                current_date,
                                path.display(),
                                sha_str
                            )?;
                        }
                    }
                }
                FuzzerStateCommand::STOP => {
                    // Stop the fuzzer if it's running
                    if let Some(task) = self.tasks.get_mut(&sha_bytes)
                        && task.status == TaskStatus::Running
                    {
                        info!("Stopping fuzzer from state file: 0x{}", sha_str);
                        let _ = task.child.kill();
                        task.status = TaskStatus::Stopped;
                        task.stopped_at = Some(Instant::now());
                    }
                }
                FuzzerStateCommand::STOPPED => {
                    // This is an automatic state, not a user command, so we ignore it
                }
                FuzzerStateCommand::RESTART => {
                    // Restart the fuzzer if it's already RUNNING
                    // If it's STOPPED or CRASHED, we need to change state to START first
                    if let Some(task) = self.tasks.get(&sha_bytes) {
                        if task.status == TaskStatus::Running {
                            // Restart a running fuzzer
                            info!("Restarting running fuzzer from state file: 0x{}", sha_str);
                            if let Some(mut task) = self.tasks.remove(&sha_bytes) {
                                let _ = task.child.kill();
                                if let Some(path) = self.targets.get(&sha_bytes) {
                                    let fuzzer_path = path.canonicalize()?;
                                    let var_env = entry.var_env.as_ref();
                                    let cmd_options = entry.cmd_options.as_ref();
                                    let new_task =
                                        self.start_task(fuzzer_path, var_env, cmd_options)?;
                                    self.tasks.insert(sha_bytes, new_task);
                                    let mut logfile = fs::OpenOptions::new()
                                        .create(true)
                                        .append(true)
                                        .open(format!("{}/running_tasks.log", logdir))?;
                                    let current_date =
                                        Local::now().format("%Y-%m-%d-%H-%M-%S").to_string();
                                    writeln!(
                                        logfile,
                                        "[{}]{} : 0x{} (restarted)",
                                        current_date,
                                        path.display(),
                                        sha_str
                                    )?;
                                }
                            }
                        } else {
                            // Fuzzer is STOPPED or CRASHED, need to change state to START first
                            info!(
                                "RESTART requested for STOPPED/CRASHED fuzzer 0x{}, but state must be changed to START first",
                                sha_str
                            );
                        }
                    } else {
                        // Fuzzer not in tasks, need to change state to START first
                        info!(
                            "RESTART requested for non-existent fuzzer 0x{}, but state must be changed to START first",
                            sha_str
                        );
                    }
                }
                FuzzerStateCommand::RUNNING | FuzzerStateCommand::CRASHED => {
                    // These are automatic states, not user commands, so we ignore them
                }
            }
        }

        Ok(())
    }

    /// Update the scheduler with the latest targets collected by the collector.
    pub fn update(&mut self, logdir: &str) -> Result<()> {
        // Read and apply user commands from state file if it exists
        if let Some(state_file) = self.read_state_file()? {
            self.apply_state_file_commands(&state_file, logdir)?;
        }

        let latest_targets = self.collector.collect_targets();

        debug!("latest_targets: {:?}", latest_targets);

        let old_targets_sha = self.targets.keys().collect::<HashSet<_>>();
        let latest_targets_sha = latest_targets.keys().collect::<HashSet<_>>();

        // NOTE: Can be optimized by iterating only once
        let targets_to_stop = old_targets_sha.difference(&latest_targets_sha);
        let targets_to_start = latest_targets_sha.difference(&old_targets_sha);
        let targets_to_check = latest_targets_sha.intersection(&old_targets_sha);

        for &target in targets_to_stop {
            info!("stopping task: {:?}", target);
            if let Some(task) = self.tasks.get_mut(target) {
                let _ = task.child.kill(); // Try to kill, but don't fail if already dead
                task.status = TaskStatus::Stopped;
                task.stopped_at = Some(Instant::now());
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
            let task = self.start_task(fuzzer_path, None, None)?;
            self.tasks.insert(*target, task);
            let current_date = Local::now().format("%Y-%m-%d-%H-%M-%S").to_string();
            writeln!(
                logfile,
                "[{}]{} : 0x{}",
                current_date,
                path.display(),
                hex::encode(target)
            )?;
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
            let task = self.start_task(fuzzer_path, None, None)?;
            // Replace the stopped task with the new running one
            self.tasks.insert(*target, task);
        }

        debug!("self.tasks: {:?}", self.tasks);

        self.targets = latest_targets;

        // Write the state file with current states
        self.write_state_file()?;

        Ok(())
    }

    fn start_task(
        &self,
        fuzzer_path: PathBuf,
        var_env: Option<&HashMap<String, String>>,
        cmd_options: Option<&Vec<String>>,
    ) -> Result<Task> {
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
        let fuzzer_coveragedir = self.coverage_directory.join(fuzzer_name);
        if !fuzzer_coveragedir.exists() {
            fs::create_dir_all(&fuzzer_coveragedir)?;
        }
        let mut child: Child;
        let mut task_container_name: Option<String> = None;

        if let Some(ref container_image) = self.container_image {
            if let Some(ref binary) = self.container_binary {
                let mut rng = rand::rng();
                let mut random_name: [u8; 8] = [0u8; 8];
                let mut random_cov: [u8; 8] = [0u8; 8];
                rng.fill(&mut random_name);
                rng.fill(&mut random_cov);
                let name = format!("docker-fw-{}-{}", hex::encode(random_name), fuzzer_name);
                let mut args: Vec<String> = vec![
                    "run".into(),
                    "--rm".into(),
                    "--name".into(),
                    name.clone(),
                    // Set up environment variable for coverage outputs in the container
                    "-e".into(),
                    format!(
                        "LLVM_PROFILE_FILE=/fuzzer/coverage/%p_{}.profraw",
                        hex::encode(random_cov)
                    ),
                    "-v".into(),
                    format!("{}:/fuzzer/binary", fuzzer_path.display()),
                    "-v".into(),
                    format!("{}:/fuzzer/crash_dir/", fuzzer_crashdir.display()),
                    "-v".into(),
                    format!("{}:/fuzzer/corpus_dir/", fuzzer_corpus.display()),
                    "-v".into(),
                    format!("{}:/fuzzer/coverage/", fuzzer_coveragedir.display()),
                ];

                // Then image and command
                args.push(container_image.clone());
                if let Some(ref opts) = self.container_options
                    && !opts.is_empty()
                {
                    for opt in opts {
                        if !opt.is_empty() {
                            args.push(opt.clone());
                        }
                    }
                }

                args.push("/fuzzer/binary".into());
                args.push("/fuzzer/corpus_dir/".into());
                args.push("-artifact_prefix=/fuzzer/crash_dir/".into());

                // Add custom command options from state file if provided
                if let Some(custom_opts) = cmd_options {
                    args.extend(custom_opts.iter().cloned());
                }

                // Add environment variables from state file if provided (using -e for docker)
                if let Some(env_vars) = var_env {
                    for (key, value) in env_vars {
                        args.push("-e".into());
                        args.push(format!("{}={}", key, value));
                    }
                }

                let mut cmd = Command::new(binary);
                cmd.args(args.iter().map(|s| s.as_str()));

                child = cmd.stderr(Stdio::piped()).spawn()?;
                if let Some(reg) = PROCESS_REGISTRY.get()
                    && let Ok(mut v) = reg.lock()
                {
                    v.push(ProcessRecord {
                        pid: child.id(),
                        container_name: Some(name.clone()),
                        fuzzer_name: fuzzer_name.to_string(),
                    });
                }
                task_container_name = Some(name);
            } else {
                return Err(SchedulerError::MissingContainerBinary);
            }
        } else {
            // Build command arguments
            let mut args = vec![
                fuzzer_corpus.display().to_string(),
                format!("-artifact_prefix={}{}", fuzzer_crashdir.display(), "/"),
            ];

            // Add coverage file output option if coverage is enabled
            if self.has_coverage {
                args.push(format!(
                    "-LLVM_PROFILE_FILE={}/%p.profraw",
                    fuzzer_coveragedir.display()
                ));
            }

            // Add custom command options from state file if provided
            if let Some(custom_opts) = cmd_options {
                args.extend(custom_opts.iter().cloned());
            }

            let mut cmd = Command::new(&fuzzer_path);
            cmd.args(&args);

            // Set environment variables from state file if provided
            if let Some(env_vars) = var_env {
                for (key, value) in env_vars {
                    cmd.env(key, value);
                }
            }

            // Always set coverage environment variable if coverage is enabled
            if self.has_coverage {
                cmd.env(
                    "LLVM_PROFILE_FILE",
                    format!("{}/%p.profraw", fuzzer_coveragedir.display()),
                );
            }

            child = cmd.stderr(Stdio::piped()).spawn()?;
            if let Some(reg) = PROCESS_REGISTRY.get()
                && let Ok(mut v) = reg.lock()
            {
                v.push(ProcessRecord {
                    pid: child.id(),
                    container_name: None,
                    fuzzer_name: fuzzer_name.to_string(),
                });
            }
        }
        let Some(child_stderr) = child.stderr.take() else {
            return Err(SchedulerError::MissingChildStderr);
        };
        Ok(Task {
            child,
            stderr: child_stderr,
            path: fuzzer_path,
            started_at: Instant::now(),
            status: TaskStatus::Running,
            container_name: task_container_name,
            crashed_at: None,
            stopped_at: None,
        })
    }

    pub fn kill_tasks(&mut self) {
        let registry = PROCESS_REGISTRY
            .get_or_init(|| Arc::new(Mutex::new(Vec::new())))
            .clone();
        if let Ok(records) = registry.lock() {
            for rec in records.iter() {
                info!("Killing task: {:?}", rec.fuzzer_name);
                if let Some(name) = &rec.container_name
                    && let Some(ref binary) = self.container_binary
                {
                    // Spawn non-blocking: fire and forget
                    let binary_clone = binary.clone();
                    let name_clone = name.clone();
                    let _ = std::thread::spawn(move || {
                        let _ = Command::new(&binary_clone)
                            .args(["rm", "-f", &name_clone])
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null())
                            .spawn()
                            .and_then(|mut child| child.wait());
                    });
                }
                // Spawn non-blocking: fire and forget
                let pid = rec.pid;
                let _ = std::thread::spawn(move || {
                    let _ = Command::new("kill")
                        .args(["-TERM", &pid.to_string()])
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn()
                        .and_then(|mut child| child.wait());
                });
            }
        }
    }

    pub fn report_crashes(&mut self) -> Result<()> {
        for (_hash, task) in self.tasks.iter_mut() {
            // Only check tasks that are still running
            if task.status != TaskStatus::Running {
                continue;
            }

            match task.child.try_wait() {
                Ok(None) => {
                    // Task is still running, keep it as Running
                }
                Ok(Some(_)) => {
                    info!("CRASH - task has exited: {:?}", task.child.id());
                    // Task has exited - mark as crashed and report it
                    task.status = TaskStatus::Crashed;
                    task.crashed_at = Some(Instant::now());

                    let mut stderr = String::new();
                    task.stderr.read_to_string(&mut stderr)?;
                    info!(
                        "crash task: {:?}, {:?}, {:?}, {:?}",
                        task,
                        task.child.id(),
                        task.child.stdout,
                        task.child.stderr
                    );

                    // Get crashing corpus instance
                    let crash = stderr
                        .lines()
                        .find(|l| l.starts_with("artifact_prefix="))
                        .unwrap_or("Crash detected but no element in stderr");
                    let pattern = "Test unit written to ";
                    let crash_corpus = &crash[crash.find(pattern).unwrap_or(0) + pattern.len()..];

                    // If the task was run in a docker container, crash_corpus will be like /fuzzer/crash_dir/...
                    // We want to rewrite that path to use self.crashes_directory instead.
                    let mut crash_corpus = crash_corpus.to_string();
                    if crash_corpus.starts_with("/fuzzer/crash_dir/") {
                        let fuzzer_name = task.path.file_name().and_then(|s| s.to_str()).ok_or(
                            SchedulerError::InconsistentState("invalid fuzzer path name"),
                        )?;
                        let filename = &crash_corpus["/fuzzer/crash_dir/".len()..];
                        crash_corpus = self
                            .crashes_directory
                            .join(fuzzer_name)
                            .join(filename)
                            .to_string_lossy()
                            .to_string();
                    }
                    info!("crash_corpus: {:?}", crash_corpus);
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
                    // Task is kept in self.tasks with status Crashed, not removed
                }
                Err(e) => return Err(SchedulerError::Io(e)),
            }
        }
        // Update state file after checking for crashes
        self.write_state_file()?;
        Ok(())
    }

    /// Get tasks with their status and timestamps
    pub fn get_tasks_with_status(&mut self) -> Vec<TaskStatusInfo> {
        let mut result: Vec<TaskStatusInfo> = Vec::new();

        for (hash, task) in self.tasks.iter_mut() {
            // Only check tasks that are still running
            if task.status != TaskStatus::Running {
                result.push(TaskStatusInfo {
                    sha: *hash,
                    path: task.path.clone(),
                    status: task.status,
                    started_at: task.started_at,
                    crashed_at: task.crashed_at,
                    stopped_at: task.stopped_at,
                });
                continue;
            }
            match task.child.try_wait() {
                Ok(None) => {
                    result.push(TaskStatusInfo {
                        sha: *hash,
                        path: task.path.clone(),
                        status: task.status,
                        started_at: task.started_at,
                        crashed_at: task.crashed_at,
                        stopped_at: task.stopped_at,
                    });
                    // Task is still running, keep it as Running
                }
                Ok(Some(_)) => {
                    // Task has exited - mark as crashed and report it
                    let crashed_at = Some(Instant::now());
                    task.crashed_at = crashed_at;
                    task.status = TaskStatus::Crashed;
                    result.push(TaskStatusInfo {
                        sha: *hash,
                        path: task.path.clone(),
                        status: task.status,
                        started_at: task.started_at,
                        crashed_at: task.crashed_at,
                        stopped_at: task.stopped_at,
                    });
                }
                Err(_) => {
                    // Task is still running, keep it as Running
                    result.push(TaskStatusInfo {
                        sha: *hash,
                        path: task.path.clone(),
                        status: task.status,
                        started_at: task.started_at,
                        crashed_at: task.crashed_at,
                        stopped_at: task.stopped_at,
                    });
                }
            }
        }
        result
    }

    pub fn log_running_tasks(&self) {
        if self.tasks.is_empty() {
            info!("No tasks running");
            return;
        }
        info!("Running tasks:");
        for (sha, task) in self.tasks.iter() {
            let secs: u64 = task.started_at.elapsed().as_secs();
            let d = TimeDelta::new(secs as i64, 0).expect("Failed to create TimeDelta");
            info!(
                "- '{}:0x{}' has been running for {:?}",
                task.path.display(),
                hex::encode(sha),
                format_duration(d.to_std().unwrap()).to_string()
            );
        }
    }

    pub fn collect_coverage(&self) {
        if !self.has_coverage {
            return;
        }
        const TIMEOUT_SECS: u64 = 10; // 10 seconds timeout

        for (_, task) in self.tasks.iter() {
            // Only collect coverage for running tasks
            if task.status != TaskStatus::Running {
                continue;
            }

            info!("collecting coverage for: {:?}", task.path);

            if let Some(name) = &task.container_name {
                if let Some(ref binary) = self.container_binary {
                    let binary_clone = binary.clone();
                    let name_for_thread = name.clone();
                    let name_for_log = name.clone();
                    let (tx, rx) = mpsc::channel();

                    // Spawn command in a thread
                    thread::spawn(move || {
                        match Command::new(&binary_clone)
                            .args([
                                "exec",
                                &name_for_thread,
                                "pkill",
                                "-SIGUSR1",
                                "-f",
                                "binary",
                            ])
                            .stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .spawn()
                        {
                            Ok(mut child) => {
                                let status = child.wait();
                                let _ = tx.send(status);
                            }
                            Err(e) => {
                                let _ = tx.send(Err(e));
                            }
                        }
                    });

                    // Wait for completion with timeout
                    match rx.recv_timeout(Duration::from_secs(TIMEOUT_SECS)) {
                        Ok(Ok(status)) => {
                            if status.success() {
                                info!(
                                    "Coverage collection successful for container: {}",
                                    name_for_log
                                );
                            } else {
                                warn!(
                                    "Coverage collection failed for container {}: {:?}",
                                    name_for_log, status
                                );
                            }
                        }
                        Ok(Err(e)) => {
                            warn!(
                                "Failed to execute coverage collection for container {}: {}",
                                name_for_log, e
                            );
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            warn!(
                                "Coverage collection timeout ({}s) for container: {}",
                                TIMEOUT_SECS, name_for_log
                            );
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            warn!(
                                "Coverage collection thread disconnected for container: {}",
                                name_for_log
                            );
                        }
                    }
                }
            } else {
                let fuzzer_name = task
                    .path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .ok_or(SchedulerError::InconsistentState(
                        "invalid fuzzer path name",
                    ))
                    .unwrap_or("unknown");
                if fuzzer_name != "unknown" {
                    let fuzzer_name_clone = fuzzer_name.to_string();
                    let (tx, rx) = mpsc::channel();

                    // Spawn command in a thread
                    thread::spawn(move || {
                        match Command::new("pkill")
                            .args(["-SIGUSR1", "-f", &fuzzer_name_clone])
                            .stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .spawn()
                        {
                            Ok(mut child) => {
                                let status = child.wait();
                                let _ = tx.send(status);
                            }
                            Err(e) => {
                                let _ = tx.send(Err(e));
                            }
                        }
                    });

                    // Wait for completion with timeout
                    match rx.recv_timeout(Duration::from_secs(TIMEOUT_SECS)) {
                        Ok(Ok(status)) => {
                            if status.success() {
                                info!("Coverage collection successful for fuzzer: {}", fuzzer_name);
                            } else {
                                warn!(
                                    "Coverage collection failed for fuzzer {}: {:?}",
                                    fuzzer_name, status
                                );
                            }
                        }
                        Ok(Err(e)) => {
                            warn!(
                                "Failed to execute coverage collection for fuzzer {}: {}",
                                fuzzer_name, e
                            );
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            warn!(
                                "Coverage collection timeout ({}s) for fuzzer: {}",
                                TIMEOUT_SECS, fuzzer_name
                            );
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            warn!(
                                "Coverage collection thread disconnected for fuzzer: {}",
                                fuzzer_name
                            );
                        }
                    }
                } else {
                    info!("No coverage collection for: {:?}", task.path);
                }
            }
        }
    }
}
