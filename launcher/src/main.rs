use chrono::TimeDelta;
use ftail::Ftail;
use humantime::format_duration;
use log::LevelFilter;
use log::{error, info};
use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug)]
struct ManagedProcess {
    name: String,
    child: Child,
    started_at: Instant,
}
#[derive(Serialize, Deserialize)]
struct Scheduler {
    name: String,
    path: String,
    config: String,
}
#[derive(Serialize, Deserialize)]
struct Config {
    schedulers: Vec<Scheduler>,
    database_url: String,
    logdir: String,
    container_binary: Option<String>,
    slack_alert: bool,
}

fn require_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{} must be set", name))
}

fn spawn_process(name: &str, cmd: &str, args: &[&str]) -> ManagedProcess {
    let child = Command::new(cmd)
        .args(args)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("failed to spawn process");
    ManagedProcess {
        name: name.to_string(),
        child,
        started_at: Instant::now(),
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

fn main() {
    // Validate env vars
    let slack_token = require_env("SLACK_TOKEN");
    let slack_channel = require_env("SLACK_CHANNEL");

    // Read config.json from argument or default to ./config.json in /launcher
    let args: Vec<String> = std::env::args().collect();
    let config_path = if args.len() > 1 {
        PathBuf::from(&args[1])
    } else {
        PathBuf::from("config.json")
    };
    let config_file = std::fs::read_to_string(&config_path)
        .unwrap_or_else(|_| panic!("Failed to read config file: {}", config_path.display()));

    let config_json: Config =
        serde_json::from_str(&config_file).expect("Failed to parse config.json");

    // Configure ftail to write to a file in logdir
    let log_file_path = format!("{}/launcher.log", config_json.logdir);
    // Ensure logdir exists
    if let Err(e) = std::fs::create_dir_all(&config_json.logdir) {
        eprintln!(
            "Failed to create log directory '{}': {}",
            config_json.logdir, e
        );
        std::process::exit(1);
    }

    Ftail::new()
        .single_file(
            std::path::Path::new(&log_file_path),
            true,
            LevelFilter::Info,
        )
        .init()
        .expect("Failed to initialize logger");

    if config_json.schedulers.is_empty() {
        panic!("schedulers array in config.json must not be empty");
    }

    // Spawn backend
    let mut procs: Vec<ManagedProcess> = Vec::new();
    procs.push(spawn_process("backend", "cargo", &["run", "-p", "backend"]));
    info!("backend launched");
    std::thread::sleep(Duration::from_secs(3));
    // Spawn slackwizard
    if config_json.slack_alert {
        procs.push(spawn_process(
            "slackwizard",
            "cargo",
            &["run", "-p", "slackwizard"],
        ));
        info!("slackwizard launched");
    }

    // Spawn scheduler project via `cargo run` in provided directory
    // Launch all schedulers in the array, collect their children and start times
    let mut scheduler_processes: Vec<(String, Instant, std::process::Child)> = Vec::new();
    for scheduler in config_json.schedulers {
        info!(
            "Launching scheduler: {} with config {}",
            scheduler.path, scheduler.config
        );
        let child = Command::new("cargo")
            .env("RUST_LOG", "info")
            .current_dir(&scheduler.path)
            .arg("run")
            .arg(scheduler.config)
            .arg(&config_json.logdir)
            .arg(
                if let Some(ref container_binary) = config_json.container_binary {
                    container_binary.to_string()
                } else {
                    "".to_string()
                },
            )
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("failed to spawn scheduler project");
        scheduler_processes.push((scheduler.name, Instant::now(), child));
    }

    std::thread::sleep(Duration::from_secs(2));

    loop {
        std::thread::sleep(Duration::from_secs(10));
        // Monitor spawned processes
        for i in (0..procs.len()).rev() {
            if let Some(status) = procs[i].child.try_wait().unwrap() {
                let name = procs[i].name.clone();
                let uptime = procs[i].started_at.elapsed().as_secs();
                procs.remove(i);
                let text = format!(
                    "Process {} exited with {:?} after {}s",
                    name,
                    status.code(),
                    uptime
                );
                info!("{}", text);
                send_slack_alert(&slack_token, &slack_channel, &text, config_json.slack_alert);
            }
        }
        // Synchronize scheduler_processes with the config file in case a new scheduler is added at runtime.
        // Reload config file
        let config_file = match std::fs::read_to_string(&config_path) {
            Ok(contents) => contents,
            Err(e) => {
                eprintln!(
                    "\x1b[31m[Error] Could not read config file '{}': {}, config is not updated\x1b[0m",
                    config_path.display(),
                    e
                );
                continue;
            }
        };
        let config_json: Config = match serde_json::from_str(&config_file) {
            Ok(json) => json,
            Err(e) => {
                eprintln!(
                    "\x1b[31m[Error] Failed to parse config.json: {}, config is not updated\x1b[0m",
                    e
                );
                continue;
            }
        };

        // If a scheduler is running but no longer present in the config, stop it
        let scheduler_names_in_config: std::collections::HashSet<String> = config_json
            .schedulers
            .iter()
            .map(|s| s.name.clone())
            .collect();
        for i in (0..scheduler_processes.len()).rev() {
            let (scheduler_name, _started_at, child) = &mut scheduler_processes[i];
            if !scheduler_names_in_config.contains(scheduler_name) {
                info!(
                    "Stopping scheduler '{}' as it is no longer present in the config.",
                    scheduler_name
                );
                // Try to terminate the process gracefully
                if let Err(e) = child.kill() {
                    error!("Failed to kill scheduler '{}': {}", scheduler_name, e);
                }
                // Wait for the process to exit to avoid zombies
                let _ = child.wait();
                scheduler_processes.remove(i);
            }
        }

        // For each scheduler in config, check if it's already running, if not, launch it
        for scheduler in config_json.schedulers {
            let already_running = scheduler_processes
                .iter()
                .any(|(name, _, _)| *name == scheduler.name);
            if !already_running {
                info!(
                    "Launching new scheduler: {} with config {}",
                    scheduler.path, scheduler.config
                );
                let child = Command::new("cargo")
                    .env("RUST_LOG", "info")
                    .current_dir(&scheduler.path)
                    .arg("run")
                    .arg(&scheduler.config)
                    .arg(&config_json.logdir)
                    .arg(
                        if let Some(ref container_binary) = config_json.container_binary {
                            container_binary.to_string()
                        } else {
                            "".to_string()
                        },
                    )
                    .stdout(Stdio::inherit())
                    .stderr(Stdio::inherit())
                    .spawn()
                    .expect("failed to spawn scheduler project");
                scheduler_processes.push((scheduler.name, Instant::now(), child));
            }
        }

        // Remove scheduler_processes whose child has exited
        for i in (0..scheduler_processes.len()).rev() {
            let (scheduler_name, started_at, child) = &mut scheduler_processes[i];
            if let Some(status) = child.try_wait().unwrap() {
                let uptime = started_at.elapsed().as_secs();
                let d = TimeDelta::new(uptime as i64, 0).expect("Failed to create TimeDelta");
                let text = format!(
                    "Scheduler {} exited with {:?} after {:?}.",
                    scheduler_name,
                    status.code(),
                    d.to_std()
                );
                info!("{}", text);
                send_slack_alert(&slack_token, &slack_channel, &text, config_json.slack_alert);
                scheduler_processes.remove(i);
            }
        }

        // Display running processes and schedulers uptime in a single message, formatted as H:M:S
        let mut status_lines = Vec::new();

        for p in procs.iter() {
            let secs = p.started_at.elapsed().as_secs();
            let d = TimeDelta::new(secs as i64, 0)
                .expect("Failed to create TimeDelta")
                .to_std()
                .unwrap();
            status_lines.push(format!(
                "Process '{}' running for {:?}",
                p.name,
                format_duration(d).to_string()
            ));
        }

        for (scheduler_name, started_at, _) in scheduler_processes.iter() {
            let secs = started_at.elapsed().as_secs();
            let d = TimeDelta::new(secs as i64, 0)
                .expect("Failed to create TimeDelta")
                .to_std()
                .unwrap();
            status_lines.push(format!(
                "Scheduler '{}' running for {:?}",
                scheduler_name,
                format_duration(d).to_string()
            ));
        }

        if !status_lines.is_empty() {
            let message = status_lines.join("\n");
            info!("Current running processes and schedulers:\n{}", message);
        }
    }
}
