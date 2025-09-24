use std::path::PathBuf;
use std::process::{Command, Stdio, Child};
use std::time::{Duration, Instant};
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, AUTHORIZATION, CONTENT_TYPE};
use log::info;

#[derive(Debug)]
struct ManagedProcess {
	name: String,
	child: Child,
	started_at: Instant,
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
	ManagedProcess { name: name.to_string(), child, started_at: Instant::now() }
}

fn send_slack_alert(token: &str, channel: &str, text: &str, should_send: bool) {
	if !should_send {
		return;
	}
	let mut headers = HeaderMap::new();
	headers.insert(CONTENT_TYPE, "application/json".parse().unwrap());
	headers.insert(AUTHORIZATION, format!("Bearer {}", token).parse().unwrap());
	let client = Client::new();
	let _ = client.post("https://slack.com/api/chat.postMessage")
		.headers(headers)
		.json(&serde_json::json!({"channel": channel, "text": text}))
		.send();
}


fn main() {
	// Validate env vars
	let slack_token = require_env("SLACK_TOKEN");
	let slack_channel = require_env("SLACK_CHANNEL");
	
	env_logger::init();

	// Read config.json from argument or default to ./config.json in /launcher
	let args: Vec<String> = std::env::args().collect();
	let config_path = if args.len() > 1 {
		PathBuf::from(&args[1])
	} else {
		PathBuf::from("config.json")
	};
	let config_file = std::fs::read_to_string(&config_path)
		.expect(&format!("Failed to read config file: {}", config_path.display()));
	let config_json: serde_json::Value = serde_json::from_str(&config_file)
		.expect("Failed to parse config.json");
	let should_send_slack_alert = config_json.get("slack_alert")
		.expect("should_send_slack_alert missing in config.json")
		.as_bool()
		.expect("should_send_slack_alert must be a boolean");
	// Support multiple schedulers: config_json["schedulers"] is an array of objects with scheduler_path and scheduler_config
	let schedulers = config_json.get("schedulers")
		.expect("schedulers missing in config.json")
		.as_array()
		.expect("schedulers must be an array");
	if schedulers.is_empty() {
		panic!("schedulers array in config.json must not be empty");
	}
	let logdir = config_json.get("logdir")
		.expect("logdir missing in config.json")
		.as_str()
		.expect("logdir must be a string");
	// Spawn backend
	let mut procs: Vec<ManagedProcess> = Vec::new();
	procs.push(spawn_process("backend", "cargo", &["run", "-p", "backend"]));
	info!("backend launched");
	std::thread::sleep(Duration::from_secs(3));
	// Spawn slackwizard
	if should_send_slack_alert {
		procs.push(spawn_process("slackwizard", "cargo", &["run", "-p", "slackwizard"]));
		info!("slackwizard launched");
	}

	// Spawn scheduler project via `cargo run` in provided directory
	// Launch all schedulers in the array, collect their children and start times
	let mut scheduler_processes: Vec<(String, Instant, std::process::Child)> = Vec::new();
	for scheduler in schedulers {
		let scheduler_project_dir = scheduler.get("scheduler_path")
			.expect("scheduler_path missing in scheduler")
			.as_str()
			.expect("scheduler_path must be a string");
		let scheduler_config = scheduler.get("scheduler_config")
			.expect("scheduler_config missing in scheduler")
			.as_str()
			.expect("scheduler_config must be a string");
		let scheduler_name = scheduler.get("scheduler_name")
			.map(|v| v.as_str().unwrap_or("unnamed_scheduler"))
			.unwrap_or("unnamed_scheduler")
			.to_string();
		info!("Launching scheduler: {} with config {}", scheduler_project_dir, scheduler_config);
		let child = Command::new("cargo")
			.env("RUST_LOG", "info")
			.current_dir(&scheduler_project_dir)
			.arg("run")
			.arg(scheduler_config.to_string())
			.arg(logdir.to_string())
			.stdout(Stdio::inherit())
			.stderr(Stdio::inherit())
			.spawn()
			.expect("failed to spawn scheduler project");
		scheduler_processes.push((scheduler_name, Instant::now(), child));
	}

	std::thread::sleep(Duration::from_secs(2));


	loop {
		// Monitor spawned processes
		for i in (0..procs.len()).rev() {
			if let Some(status) = procs[i].child.try_wait().unwrap() {
				let name = procs[i].name.clone();
				let uptime = procs[i].started_at.elapsed().as_secs();
				procs.remove(i);
				let text = format!("Process {} exited with {:?} after {}s", name, status.code(), uptime);
				info!("{}", text);
				send_slack_alert(&slack_token, &slack_channel, &text, should_send_slack_alert);
			}
		}
		// Synchronize scheduler_processes with the config file in case a new scheduler is added at runtime.
		// Reload config file
		let config_file = std::fs::read_to_string(&config_path)
			.expect(&format!("Failed to read config file: {}", config_path.display()));
		let config_json: serde_json::Value = serde_json::from_str(&config_file)
			.expect("Failed to parse config.json");
		let schedulers = config_json.get("schedulers")
			.expect("schedulers missing in config.json")
			.as_array()
			.expect("schedulers must be an array");

		// For each scheduler in config, check if it's already running, if not, launch it
		for scheduler in schedulers {
			let scheduler_name = scheduler.get("scheduler_name")
				.map(|v| v.as_str().unwrap_or("unnamed_scheduler"))
				.unwrap_or("unnamed_scheduler")
				.to_string();
			let already_running = scheduler_processes.iter().any(|(name, _, _)| *name == scheduler_name);
			if !already_running {
				let scheduler_project_dir = scheduler.get("scheduler_path")
					.expect("scheduler_path missing in scheduler")
					.as_str()
					.expect("scheduler_path must be a string");
				let scheduler_config = scheduler.get("scheduler_config")
					.expect("scheduler_config missing in scheduler")
					.as_str()
					.expect("scheduler_config must be a string");
				info!("Launching new scheduler: {} with config {}", scheduler_project_dir, scheduler_config);
				let child = Command::new("cargo")
					.env("RUST_LOG", "info")
					.current_dir(&scheduler_project_dir)
					.arg("run")
					.arg(scheduler_config.to_string())
					.arg(logdir.to_string())
					.stdout(Stdio::inherit())
					.stderr(Stdio::inherit())
					.spawn()
					.expect("failed to spawn scheduler project");
				scheduler_processes.push((scheduler_name, Instant::now(), child));
			}
		}

		// Remove scheduler_processes whose child has exited
		for i in (0..scheduler_processes.len()).rev() {
			let (scheduler_name, started_at, child) = &mut scheduler_processes[i];
			if let Some(status) = child.try_wait().unwrap() {
				let uptime = started_at.elapsed().as_secs();
				let text = format!("Scheduler {} exited with {:?} after {}s.", scheduler_name, status.code(), uptime);
				info!("{}", text);
				send_slack_alert(&slack_token, &slack_channel, &text, should_send_slack_alert);
				scheduler_processes.remove(i);
			}
		}
		
		// Display running processes and schedulers uptime in a single message, formatted as H:M:S
		let mut status_lines = Vec::new();

		for p in procs.iter() {
			let secs = p.started_at.elapsed().as_secs();
			let hours = secs / 3600;
			let minutes = (secs % 3600) / 60;
			let seconds = secs % 60;
			status_lines.push(format!("Process '{}' running for {:02}:{:02}:{:02}", p.name, hours, minutes, seconds));
		}

		for (scheduler_name, started_at		, _) in scheduler_processes.iter() {
			let secs = started_at.elapsed().as_secs();
			let hours = secs / 3600;
			let minutes = (secs % 3600) / 60;
			let seconds = secs % 60;
			status_lines.push(format!("Scheduler '{}' running for {:02}:{:02}:{:02}", scheduler_name, hours, minutes, seconds));
		}

		if !status_lines.is_empty() {
			let message = status_lines.join("\n");
			info!("Current running processes and schedulers:\n{}", message);
		}

		std::thread::sleep(Duration::from_secs(60));
	}
} 