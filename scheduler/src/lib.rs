use chrono::{TimeDelta, prelude::*};
use humantime::format_duration;
use log::{debug, info};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;
use std::{
    collections::{HashMap, HashSet},
    fs, io,
    io::Read,
    path::PathBuf,
    process::{Child, ChildStderr, Command, Stdio},
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
    /// Create a new [`Scheduler`].
    pub fn new(collector: C, config: SchedulerConfig, container_binary: Option<PathBuf>) -> Self {
        // Ensure Ctrl-C handler is installed once
        install_ctrlc_handler();
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
            info!("stopping task: {:?}", target);
            if let Some(task) = self.tasks.get_mut(target) {
                let _ = task.child.kill(); // Try to kill, but don't fail if already dead
                task.status = TaskStatus::Stopped;
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
            let task = self.start_task(fuzzer_path)?;
            // Replace the stopped task with the new running one
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

                child = Command::new(binary)
                    .args(args.iter().map(|s| s.as_str()))
                    .stderr(Stdio::piped())
                    .spawn()?;
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
            child = Command::new(&fuzzer_path)
                .args([
                    &fuzzer_corpus.display().to_string(),
                    &format!("-artifact_prefix={}{}", fuzzer_crashdir.display(), "/"),
                    // coverage file output option
                    &format!(
                        "-LLVM_PROFILE_FILE={}/%p.profraw",
                        fuzzer_coveragedir.display() // Or wherever you want to store profraws; adjust as needed
                    ),
                ])
                .stderr(Stdio::piped())
                .spawn()?;
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
        Ok(())
    }

    /// Get tasks with their status (running or stopped)
    pub fn get_tasks_with_status(&mut self) -> Vec<(Sha, PathBuf, TaskStatus)> {
        let mut result: Vec<([u8; 32], PathBuf, TaskStatus)> = Vec::new();

        for (hash, task) in self.tasks.iter_mut() {
            // Only check tasks that are still running
            if task.status != TaskStatus::Running {
                result.push((*hash, task.path.clone(), task.status));
                continue;
            }
            match task.child.try_wait() {
                Ok(None) => {
                    result.push((*hash, task.path.clone(), task.status));
                    // Task is still running, keep it as Running
                }
                Ok(Some(_)) => {
                    // Task has exited - mark as crashed and report it
                    result.push((*hash, task.path.clone(), TaskStatus::Crashed));
                }
                Err(_) => {
                    // Task is still running, keep it as Running
                    result.push((*hash, task.path.clone(), task.status));
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
        for (_, task) in self.tasks.iter() {
            info!("collecting coverage for: {:?}", task.path);
            if let Some(name) = &task.container_name {
                if let Some(ref binary) = self.container_binary {
                    let _ = Command::new(binary)
                        .args(["exec", name, "pkill", "-SIGUSR1", "-f", "binary"])
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn()
                        .and_then(|mut child| child.wait());
                }
            } else {
                let fuzzer_name = task
                    .path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .ok_or(SchedulerError::InconsistentState(
                        "invalid fuzzer path name",
                    ))
                    .unwrap_or("None");
                if fuzzer_name != "None" {
                    let _ = Command::new("pkill")
                        .args(["-SIGUSR1", "-f", fuzzer_name])
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn()
                        .and_then(|mut child| child.wait());
                } else {
                    info!("No coverage collection for: {:?}", task.path);
                }
            }
        }
    }
}
