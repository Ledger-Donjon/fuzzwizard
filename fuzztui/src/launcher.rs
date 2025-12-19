use log::{error, info};
use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use std::{collections::HashMap, fmt, fs};

use scheduler::{Scheduler, SchedulerConfig, Sha, TargetsCollector};

#[derive(Debug, Clone, Copy)]
pub enum ProcessStatus {
    Running,
    Stopped,
}

#[derive(Debug)]
pub struct ManagedProcess {
    pub name: String,
    pub child: Child,
    pub started_at: Instant,
    pub info: String,
    pub status: ProcessStatus,
}

pub struct SchedulerProcess {
    pub name: String,
    path: PathBuf,
    pub started_at: Instant,
    pub scheduler: Scheduler<DirectoryTargetsCollector>,
    pub info: String,
    pub status: ProcessStatus,
    config: SchedulerConfig,
    pub last_build_at: Option<Instant>,
}

// C'est 2 structures représenent les champs de la config.json
#[derive(Serialize, Deserialize)]
struct SchedulerConf {
    name: String,
    path: String,
    config: String,
}

#[derive(Serialize, Deserialize)]
pub struct Config {
    schedulers: Vec<SchedulerConf>,
    database_url: String,
    pub logdir: String,
    container_binary: Option<String>,
    slack_alert: bool,
}

pub struct Launcher {
    slack_token: String,
    slack_channel: String,
    config_path: PathBuf,
    config_json: Config,
    pub procs: Vec<ManagedProcess>,
    pub scheduler_processes: Vec<SchedulerProcess>,
    last_config_update: std::time::Instant,
    pub config_status: String,
}

impl fmt::Debug for Launcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Launcher")
            .field("slack_token", &"***redacted***")
            .field("slack_channel", &self.slack_channel)
            .field("config_path", &self.config_path)
            .field("procs", &self.procs)
            .field("last_config_update", &self.last_config_update)
            .finish()
    }
}

impl Launcher {
    pub fn new(
        slack_token: String,
        slack_channel: String,
        config_path: PathBuf,
        config_json: Config,
    ) -> Self {
        Launcher {
            slack_token,
            slack_channel,
            config_path,
            config_json,
            procs: Vec::new(),
            scheduler_processes: Vec::new(),
            last_config_update: std::time::Instant::now(),
            config_status: String::from("Config loaded successfully"),
        }
    }

    pub fn init(&mut self) {
        if self.config_json.schedulers.is_empty() {
            panic!("schedulers array in config.json must not be empty");
        }

        // Spawn backend
        self.procs
            .push(spawn_process("backend", "cargo", &["run", "-p", "backend"]));
        info!("backend launched");
        std::thread::sleep(Duration::from_secs(3));
        // Spawn slackwizard
        if self.config_json.slack_alert {
            self.procs.push(spawn_process(
                "slackwizard",
                "cargo",
                &["run", "-p", "slackwizard"],
            ));
            info!("slackwizard launched");
        }

        // Launch all schedulers in the array, collect their children and start times
        for scheduler in &self.config_json.schedulers {
            let scheduler_config_path = scheduler.config.clone();
            info!("Reading scheduler config file {}\n", scheduler_config_path);
            let (scheduler_started, config, last_build_at) = create_scheduler(
                &scheduler_config_path,
                self.config_json.logdir.clone(),
                self.config_json.container_binary.clone(),
            );

            self.scheduler_processes.push(SchedulerProcess::new(
                scheduler.name.clone(),
                PathBuf::from(scheduler_config_path),
                Instant::now(),
                scheduler_started,
                config,
                last_build_at,
            ));
            std::thread::sleep(Duration::from_secs(2));
        }
    }

    pub fn update_proc_status(&mut self) {
        for i in (0..self.procs.len()).rev() {
            if matches!(self.procs[i].status, ProcessStatus::Running) {
                if let Some(status) = self.procs[i].child.try_wait().unwrap() {
                    let name = self.procs[i].name.clone();
                    let uptime = self.procs[i].started_at.elapsed().as_secs();
                    let text = format!(
                        "Process {} exited with {:?} after {}s",
                        name,
                        status.code(),
                        uptime
                    );
                    self.procs[i].info = text.clone();
                    self.procs[i].status = ProcessStatus::Stopped;
                    send_slack_alert(
                        &self.slack_token,
                        &self.slack_channel,
                        &text.clone(),
                        self.config_json.slack_alert,
                    );
                }
            }
        }
    }

    /// Update the status of all subprocesses and schedulers.
    /// This function should be called periodically—callers should check the value of
    /// self.last_config_update and only call this if enough time has elapsed.
    pub fn update_status(&mut self) {
        // Reload config file if it changed, and update self.config_json
        self.update_proc_status();

        let config_file = match std::fs::read_to_string(&self.config_path) {
            Ok(contents) => contents,
            Err(e) => {
                error!(
                    "\x1b[31m[Error] Could not read config file '{}': {}, config is not updated\x1b[0m",
                    self.config_path.display(),
                    e
                );
                self.config_status = format!(
                    "Error: Could not read config file '{}': {}",
                    self.config_path.display(),
                    e
                );
                return; // Early return
            }
        };
        let config_json: Config = match serde_json::from_str(&config_file) {
            Ok(json) => json,
            Err(e) => {
                error!(
                    "\x1b[31m[Error] Failed to parse config.json: {}, config is not updated\x1b[0m",
                    e
                );
                self.config_status = format!("Error: Failed to parse config.json: {}", e);
                return;
            }
        };
        self.config_json = config_json;
        self.config_status = String::from("Config loaded successfully");
        self.last_config_update = std::time::Instant::now();

        // Remove schedulers that are gone from config
        let scheduler_names_in_config: std::collections::HashSet<String> = self
            .config_json
            .schedulers
            .iter()
            .map(|s| s.name.clone())
            .collect();
        for i in (0..self.scheduler_processes.len()).rev() {
            if matches!(self.scheduler_processes[i].status, ProcessStatus::Running)
                && !scheduler_names_in_config.contains(&self.scheduler_processes[i].name)
            {
                self.scheduler_processes[i].scheduler.kill_tasks();
                // TODO: send slack alert if desired
                self.scheduler_processes[i].info = format!(
                    "scheduler '{}' stopped: removed from config",
                    self.scheduler_processes[i].name.clone()
                );
                self.scheduler_processes[i].status = ProcessStatus::Stopped;
            }
        }

        // Launch schedulers that are new in config
        for scheduler in &self.config_json.schedulers {
            for sched_proc in &mut self.scheduler_processes {
                if sched_proc.name == scheduler.name
                    && matches!(sched_proc.status, ProcessStatus::Stopped)
                {
                    let (scheduler_started, config, last_build_at) = create_scheduler(
                        &scheduler.config.clone(),
                        self.config_json.logdir.clone(),
                        self.config_json.container_binary.clone(),
                    );
                    sched_proc.path = PathBuf::from(scheduler.path.clone());
                    sched_proc.started_at = Instant::now();
                    sched_proc.scheduler = scheduler_started;
                    sched_proc.config = config;
                    sched_proc.last_build_at = last_build_at;
                    sched_proc.status = ProcessStatus::Running;
                    sched_proc.info = format!("Scheduler '{}' is active", sched_proc.name.clone());
                }
            }
            let already_running = self
                .scheduler_processes
                .iter()
                .any(|p| p.name == scheduler.name);
            if !already_running {
                let (scheduler_started, config, last_build_at) = create_scheduler(
                    &scheduler.config.clone(),
                    self.config_json.logdir.clone(),
                    self.config_json.container_binary.clone(),
                );
                self.scheduler_processes.push(SchedulerProcess::new(
                    scheduler.name.clone(),
                    PathBuf::from(scheduler.path.clone()),
                    Instant::now(),
                    scheduler_started,
                    config,
                    last_build_at,
                ));
            }
        }

        // Update info string, targets, coverage, and crash report for each scheduler
        for sched_proc in &mut self.scheduler_processes {
            // Optionally: re-build targets if configuration changed (non-blocking)
            // The build happens in background, we update last_build_at optimistically
            let build_result = build_targets(sched_proc.config.clone(), sched_proc.last_build_at);
            if let Some(build_time) = build_result {
                sched_proc.last_build_at = Some(build_time);
            }

            let _update = sched_proc
                .scheduler
                .update(self.config_json.logdir.clone().as_str());
            let _ = sched_proc.scheduler.report_crashes();

            sched_proc.scheduler.collect_coverage();
        }
    }

    /// Collect information about processes and check if they are still running
    pub fn collect_proc_info(&mut self) {
        for proc in &mut self.procs {
            match proc.child.try_wait() {
                Ok(None) => {
                    // Process is still running
                    proc.status = ProcessStatus::Running;
                }
                Ok(Some(status)) => {
                    // Process has exited
                    let name = proc.name.clone();
                    let uptime = proc.started_at.elapsed().as_secs();
                    proc.status = ProcessStatus::Stopped;
                    proc.info = format!(
                        "Process '{}' exited with {:?} after {}s",
                        name,
                        status.code(),
                        uptime
                    );
                    send_slack_alert(
                        &self.slack_token,
                        &self.slack_channel,
                        &proc.info.clone(),
                        self.config_json.slack_alert,
                    );
                }
                Err(_) => {
                    // Error checking status, assume stopped
                    proc.status = ProcessStatus::Stopped;
                    proc.info = format!("Process '{}' status check failed", proc.name);
                }
            }
        }
    }

    /// Collect information about schedulers and check if they are still active
    pub fn collect_scheduler(&mut self) {
        // Get the set of scheduler names that should be running from config
        let scheduler_names_in_config: std::collections::HashSet<String> = self
            .config_json
            .schedulers
            .iter()
            .map(|s| s.name.clone())
            .collect();

        for scheduler in &mut self.scheduler_processes {
            // Check if scheduler is still in config
            if scheduler_names_in_config.contains(&scheduler.name) {
                // Scheduler is still in config, ensure it's marked as running
                if matches!(scheduler.status, ProcessStatus::Stopped) {
                    // Scheduler was stopped but is back in config, mark as running
                    scheduler.status = ProcessStatus::Running;
                    scheduler.info = format!("Scheduler '{}' is active", scheduler.name);
                } else {
                    // Scheduler is running and still in config, keep it as running
                    scheduler.status = ProcessStatus::Running;
                }
            } else {
                // Scheduler is no longer in config, mark as stopped
                if matches!(scheduler.status, ProcessStatus::Running) {
                    scheduler.scheduler.kill_tasks();
                    scheduler.status = ProcessStatus::Stopped;
                    scheduler.info = format!(
                        "Scheduler '{}' stopped: removed from config",
                        scheduler.name
                    );
                    send_slack_alert(
                        &self.slack_token,
                        &self.slack_channel,
                        &scheduler.info.clone(),
                        self.config_json.slack_alert,
                    );
                }
            }
        }
    }

    pub fn shutdown(&mut self) {
        for proc in &mut self.procs {
            proc.child.kill().unwrap();
        }
        for scheduler in &mut self.scheduler_processes {
            scheduler.scheduler.kill_tasks();
        }
    }
}

impl std::fmt::Display for ProcessStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProcessStatus::Running => write!(f, "Running"),
            ProcessStatus::Stopped => write!(f, "Stopped"),
        }
    }
}

impl SchedulerProcess {
    pub fn new(
        name: String,
        path: PathBuf,
        started_at: Instant,
        scheduler: Scheduler<DirectoryTargetsCollector>,
        config: SchedulerConfig,
        last_build_at: Option<Instant>,
    ) -> Self {
        SchedulerProcess {
            name: name.clone(),
            path,
            started_at,
            scheduler,
            info: format!("Scheduler {} started", name.clone()).to_string(),
            status: ProcessStatus::Running,
            config,
            last_build_at,
        }
    }
}

pub struct DirectoryTargetsCollector {
    targets_directories: Vec<PathBuf>,
}

impl TargetsCollector for DirectoryTargetsCollector {
    fn collect_targets(&self) -> HashMap<Sha, PathBuf> {
        let mut targets = HashMap::new();
        for directory in self.targets_directories.iter().cloned() {
            for direntry in fs::read_dir(directory).unwrap() {
                let direntry = direntry.unwrap();
                let target_path = direntry.path();
                if !target_path.is_file() {
                    continue;
                }
                if target_path.metadata().unwrap().permissions().mode() & 0o111 == 0 {
                    continue;
                }

                let target_hash = Sha3_256::digest(fs::read(&target_path).unwrap());
                targets.insert(target_hash.into(), target_path);
            }
        }
        targets
    }
}

pub fn require_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{} must be set", name))
}

fn spawn_process(name: &str, cmd: &str, args: &[&str]) -> ManagedProcess {
    let child = Command::new(cmd)
        .args(args)
        .stdout(Stdio::null()) // Redirect stdout to /dev/null to avoid breaking TUI
        .stderr(Stdio::null()) // Redirect stderr to /dev/null to avoid breaking TUI
        .env("RUST_LOG", "error") // Only show errors, not info logs
        .spawn()
        .expect("failed to spawn process");
    ManagedProcess {
        name: name.to_string(),
        child,
        started_at: Instant::now(),
        info: format!("Process '{}' started", name).to_string(),
        status: ProcessStatus::Running,
    }
}

fn send_slack_alert(token: &str, channel: &str, text: &str, should_send: bool) {
    if !should_send {
        return;
    }
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, "application/json".parse().unwrap());
    headers.insert(AUTHORIZATION, format!("Bearer {}", token).parse().unwrap());
    let client = Client::new();
    let _ = client
        .post("https://slack.com/api/chat.postMessage")
        .headers(headers)
        .json(&serde_json::json!({"channel": channel, "text": text}))
        .send();
}

fn build_targets(config_json: scheduler::SchedulerConfig, last_build_at: Option<Instant>) -> Option<Instant> {
    // Run build_script in background thread (non-blocking)
    let now = Instant::now();
    let scheduler_name = config_json.scheduler_name.clone();
    let build_script = config_json.build_script.clone();

    if last_build_at.is_some() && now.duration_since(last_build_at.unwrap()) < Duration::from_secs(600) {
        info!("Build script already ran recently, skipping");
        return last_build_at;
    }

    // Spawn non-blocking build in background thread
    thread::spawn(move || {
        let status = Command::new("sh")
            .arg("-c")
            .arg(&build_script)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .and_then(|mut child| child.wait());

        match status {
            Ok(s) if s.success() => {
                info!("build_script ran successfully for {}", scheduler_name);
            }
            Ok(s) => {
                error!(
                    "build_script exited with status: {} for {}",
                    s, scheduler_name
                );
            }
            Err(e) => {
                error!("Failed to run build_script for {}: {}", scheduler_name, e);
            }
        }
    });

    // Return immediately with current time, build happens in background
    Some(now)
}

fn create_scheduler(
    config_path: &str,
    logdir: String,
    container_binary: Option<String>,
) -> (
    Scheduler<DirectoryTargetsCollector>,
    SchedulerConfig,
    Option<Instant>,
) {
    info!("Reading config file {}\n", config_path);
    let config_contents = fs::read_to_string(config_path)
        .unwrap_or_else(|_| panic!("Failed to read config file {}", config_path));

    // If logdir does not exist, create it
    if let Err(e) = std::fs::create_dir_all(&logdir) {
        error!("Failed to create log directory '{}': {}", &logdir, e);
        std::process::exit(1);
    }

    let config_json: scheduler::SchedulerConfig =
        serde_json::from_str(&config_contents).expect("Failed to parse config file as JSON");

    let collector = DirectoryTargetsCollector {
        targets_directories: config_json
            .targets_directory
            .iter()
            .map(PathBuf::from)
            .collect(),
    };
    let last_build_at = build_targets(config_json.clone(), None);
    let scheduler = Scheduler::new(
        collector,
        config_json.clone(),
        container_binary.map(PathBuf::from),
    );

    (scheduler, config_json.clone(), last_build_at)
}
