use std::io::{self, Write};
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

fn prompt_targets_path() -> PathBuf {
	print!("Path to targets directory for scheduler: ");
	io::stdout().flush().unwrap();
	let mut input = String::new();
	io::stdin().read_line(&mut input).unwrap();
	let trimmed = input.trim();
	if trimmed.is_empty() { panic!("Targets path is required"); }
	PathBuf::from(trimmed)
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

fn send_slack_alert(token: &str, channel: &str, text: &str) {
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
	let database_url = require_env("DATABASE_URL");
	let slack_token = require_env("SLACK_TOKEN");
	let slack_channel = require_env("SLACK_CHANNEL");
	let _ = database_url; // used by backend/collector
	
	env_logger::init();

	// Ask for scheduler path
	let scheduler_project_dir = prompt_targets_path();
	info!("Scheduler project: {}", scheduler_project_dir.display());

	// Spawn backend
	let mut procs: Vec<ManagedProcess> = Vec::new();
	procs.push(spawn_process("backend", "cargo", &["run", "-p", "backend"]));
	info!("backend launched");
	std::thread::sleep(Duration::from_secs(5));
	// Spawn slackwizard
	procs.push(spawn_process("slackwizard", "cargo", &["run", "-p", "slackwizard"]));
	info!("slackwizard launched");

	// Spawn scheduler project via `cargo run` in provided directory
	let mut scheduler_child = Command::new("cargo")
		.env("RUST_LOG", "info")
		.current_dir(&scheduler_project_dir)
		.arg("run")
		.stdout(Stdio::inherit())
		.stderr(Stdio::inherit())
		.spawn()
		.expect("failed to spawn scheduler project");
	let scheduler_started = Instant::now();

	loop {
		// Monitor spawned processes
		for i in (0..procs.len()).rev() {
			if let Some(status) = procs[i].child.try_wait().unwrap() {
				let name = procs[i].name.clone();
				let uptime = procs[i].started_at.elapsed().as_secs();
				procs.remove(i);
				let text = format!("Process {} exited with {:?} after {}s", name, status.code(), uptime);
				info!("{}", text);
				send_slack_alert(&slack_token, &slack_channel, &text);
			}
		}
		if let Some(status) = scheduler_child.try_wait().unwrap() {
			let uptime = scheduler_started.elapsed().as_secs();
			let text = format!("Scheduler project exited with {:?} after {}s", status.code(), uptime);
			info!("{}", text);
			send_slack_alert(&slack_token, &slack_channel, &text);
			break;
		}

		// Display running processes uptime
		for p in procs.iter() {
			info!("{} running for {}s", p.name, p.started_at.elapsed().as_secs());
		}
		info!("scheduler running for {}s", scheduler_started.elapsed().as_secs());

		std::thread::sleep(Duration::from_secs(5));
	}
} 