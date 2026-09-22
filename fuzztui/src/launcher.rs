// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use std::{fmt, fs};

use log::{error, info};
use reqwest::blocking::Client;
use scheduler::build::{BuildOutcome, Builder};
use scheduler::{DirectoryTargetsCollector, Scheduler, SchedulerConfig};
use serde::{Deserialize, Serialize};

fn send_admin_event_to_backend(backend_url: &str, event_type: &str, message: &str) {
    let client = Client::new();
    let event = serde_json::json!({
        "event_type": event_type,
        "message": message,
    });

    if let Err(e) = client
        .post(format!("{backend_url}/admin_events"))
        .json(&event)
        .send()
    {
        error!("Failed to send admin event to backend: {e}");
    }
}

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
    pub config: SchedulerConfig,
    builder: Builder,
}

impl SchedulerProcess {
    /// When the last build of this scheduler's targets finished.
    pub fn last_build_at(&self) -> Option<Instant> {
        self.builder.last_build_at()
    }

    /// Since when a build has been running, if one is in flight.
    pub fn building_since(&self) -> Option<Instant> {
        self.builder.running_since()
    }
}

// These 2 structures represent the fields from config.json
#[derive(Serialize, Deserialize)]
struct SchedulerConf {
    name: String,
    config: String,
}

#[derive(Serialize, Deserialize)]
pub struct Config {
    schedulers: Vec<SchedulerConf>,
    database_url: String,
    pub logdir: String,
    container_binary: Option<String>,
    backend_binary: Option<String>,
    notificator_binary: Option<String>,
}

pub struct Launcher {
    config_path: PathBuf,
    config_json: Config,
    pub procs: Vec<ManagedProcess>,
    pub scheduler_processes: Vec<SchedulerProcess>,
    last_config_update: std::time::Instant,
    pub config_status: String,
    backend_url: String,
}

impl fmt::Debug for Launcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Launcher")
            .field("config_path", &self.config_path)
            .field("procs", &self.procs)
            .field("last_config_update", &self.last_config_update)
            .finish()
    }
}

impl Launcher {
    pub fn new(config_path: PathBuf, config_json: Config) -> Self {
        Launcher {
            config_path,
            backend_url: "http://127.0.0.1:3000".to_string(),
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

        // Spawn backend with DATABASE_URL passed via environment variable (not CLI args)
        let db_url = self.config_json.database_url.clone();
        let (backend_cmd, backend_args) =
            if let Some(ref backend_binary) = self.config_json.backend_binary {
                (backend_binary.clone(), Vec::new())
            } else {
                let args = vec!["run".to_string(), "-p".to_string(), "backend".to_string()];
                ("cargo".to_string(), args)
            };
        let backend_env: Vec<(&str, &str)> = vec![("DATABASE_URL", db_url.as_str())];
        self.procs.push(spawn_process(
            "backend",
            &backend_cmd,
            &backend_args
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<&str>>(),
            &backend_env,
        ));
        info!("backend launched");
        send_admin_event_to_backend(&self.backend_url, "backend_launched", "Backend launched");
        std::thread::sleep(Duration::from_secs(3));
        // Spawn notificator (always active, but notifications depend on config)
        let config_path_str = self.config_path.to_string_lossy().to_string();
        let (notificator_cmd, notificator_args) =
            if let Some(ref notificator_binary) = self.config_json.notificator_binary {
                // Use release binary if specified
                let args: Vec<String> = vec![config_path_str.clone()];
                (notificator_binary.clone(), args)
            } else {
                // Fallback to cargo run
                let args = vec![
                    "run".to_string(),
                    "-p".to_string(),
                    "notificator".to_string(),
                    "--".to_string(),
                    config_path_str.clone(),
                ];
                ("cargo".to_string(), args)
            };
        let notificator_env: Vec<(&str, &str)> = Vec::new();
        self.procs.push(spawn_process(
            "notificator",
            &notificator_cmd,
            &notificator_args
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<&str>>(),
            &notificator_env,
        ));
        info!("notificator launched");

        // Launch all schedulers in the array, collect their children and start times
        for scheduler in &self.config_json.schedulers {
            let scheduler_config_path = scheduler.config.clone();
            info!("Reading scheduler config file {scheduler_config_path}\n");
            let (scheduler_started, config) = create_scheduler(
                &scheduler_config_path,
                self.config_json.logdir.clone(),
                self.config_json.container_binary.clone(),
            );

            let mut sched_proc = SchedulerProcess::new(
                scheduler.name.clone(),
                PathBuf::from(scheduler_config_path),
                Instant::now(),
                scheduler_started,
                config,
            );

            let _update = sched_proc
                .scheduler
                .update(self.config_json.logdir.clone().as_str());
            self.scheduler_processes.push(sched_proc);

            info!("Scheduler {} launched", scheduler.name);
            send_admin_event_to_backend(
                &self.backend_url,
                "scheduler_launched",
                &format!("Scheduler {} launched", scheduler.name),
            );
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
                    let backend_url = self.backend_url.clone();
                    send_admin_event_to_backend(&backend_url, "process_crash", &text);
                }
            }
        }
    }

    /// Reload the main config file manually.
    /// This performs the same config reload logic as update_status but without
    /// updating scheduler status or other background tasks.
    pub fn reload_config(&mut self) {
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
                    "\x1b[31m[Error] Failed to parse config.json: {e}, config is not updated\x1b[0m"
                );
                self.config_status = format!("Error: Failed to parse config.json: {e}");
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
                    let (scheduler_started, config) = create_scheduler(
                        &scheduler.config.clone(),
                        self.config_json.logdir.clone(),
                        self.config_json.container_binary.clone(),
                    );
                    sched_proc.path = PathBuf::from(&scheduler.config);
                    sched_proc.started_at = Instant::now();
                    sched_proc.scheduler = scheduler_started;
                    sched_proc.builder = Builder::new(
                        sched_proc.name.clone(),
                        config.build_script.clone(),
                        config.build_script_interval_hours,
                    );
                    sched_proc.config = config;
                    sched_proc.status = ProcessStatus::Running;
                    sched_proc.info = format!("Scheduler '{}' is active", sched_proc.name.clone());

                    info!("Old Scheduler {} updated", sched_proc.name);
                    let _update = sched_proc
                        .scheduler
                        .update(self.config_json.logdir.clone().as_str());
                }
            }
            let already_running = self
                .scheduler_processes
                .iter()
                .any(|p| p.name == scheduler.name);
            if !already_running {
                let (scheduler_started, config) = create_scheduler(
                    &scheduler.config.clone(),
                    self.config_json.logdir.clone(),
                    self.config_json.container_binary.clone(),
                );
                let mut new_scheduler = SchedulerProcess::new(
                    scheduler.name.clone(),
                    PathBuf::from(&scheduler.config),
                    Instant::now(),
                    scheduler_started,
                    config,
                );
                info!("New scheduler updated: {}", scheduler.name);
                let _update = new_scheduler
                    .scheduler
                    .update(self.config_json.logdir.clone().as_str());
                self.scheduler_processes.push(new_scheduler);
            }
        }
    }

    /// Update the status of all subprocesses and schedulers.
    /// This function should be called periodically—callers should check the value of
    /// self.last_config_update and only call this if enough time has elapsed.
    pub fn update_status(&mut self) {
        // Reload config file if it changed, and update self.config_json
        self.update_proc_status();

        // Update info string, targets, coverage, and crash report for each scheduler
        // Only update schedulers that are running
        for sched_proc in &mut self.scheduler_processes {
            // Skip update if scheduler is stopped
            if matches!(sched_proc.status, ProcessStatus::Stopped) {
                continue;
            }

            // Start or reap the build of this scheduler's targets. The script
            // runs on its own thread: waiting for it here would hold the
            // launcher lock, freezing the UI and the other schedulers.
            if let Some(BuildOutcome::Failure(msg)) = sched_proc.builder.poll() {
                error!("{msg}");
                send_admin_event_to_backend(&self.backend_url, "build_error", &msg);
            }

            let _ = sched_proc.scheduler.report_crashes();
            sched_proc.scheduler.collect_coverage();

            let _update = sched_proc
                .scheduler
                .update(self.config_json.logdir.clone().as_str());

            sched_proc.scheduler.snapshot_coverage_if_due();
            sched_proc.scheduler.report_status();
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
                    let backend_url = self.backend_url.clone();
                    send_admin_event_to_backend(&backend_url, "scheduler_removed", &scheduler.info);
                }
            }
        }
    }

    pub fn shutdown(&mut self) {
        for proc in &mut self.procs {
            if let Err(e) = proc.child.kill() {
                error!("Failed to kill process '{}': {}", proc.name, e);
            }
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
    ) -> Self {
        let builder = Builder::new(
            name.clone(),
            config.build_script.clone(),
            config.build_script_interval_hours,
        );

        SchedulerProcess {
            name: name.clone(),
            path,
            started_at,
            scheduler,
            info: format!("Scheduler {} started", name.clone()).to_string(),
            status: ProcessStatus::Running,
            config,
            builder,
        }
    }
}

fn spawn_process(
    name: &str,
    cmd: &str,
    args: &[&str],
    env_vars: &[(&str, &str)],
) -> ManagedProcess {
    let mut command = Command::new(cmd);
    command
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("RUST_LOG", "error");

    for (key, value) in env_vars {
        command.env(key, value);
    }

    let child = command.spawn().expect("failed to spawn process");
    ManagedProcess {
        name: name.to_string(),
        child,
        started_at: Instant::now(),
        info: format!("Process '{name}' started").to_string(),
        status: ProcessStatus::Running,
    }
}

fn create_scheduler(
    config_path: &str,
    logdir: String,
    container_binary: Option<String>,
) -> (Scheduler<DirectoryTargetsCollector>, SchedulerConfig) {
    info!("Reading config file {config_path}\n");
    let config_contents = fs::read_to_string(config_path)
        .unwrap_or_else(|_| panic!("Failed to read config file {config_path}"));

    // If logdir does not exist, create it
    if let Err(e) = std::fs::create_dir_all(&logdir) {
        error!("Failed to create log directory '{}': {}", &logdir, e);
    }

    let config_json: scheduler::SchedulerConfig =
        serde_json::from_str(&config_contents).expect("Failed to parse config file as JSON");

    let collector = DirectoryTargetsCollector::new(
        config_json
            .targets_directory
            .iter()
            .map(PathBuf::from)
            .collect(),
    );
    let scheduler = Scheduler::new(
        collector,
        config_json.clone(),
        container_binary.map(PathBuf::from),
    );

    (scheduler, config_json)
}
