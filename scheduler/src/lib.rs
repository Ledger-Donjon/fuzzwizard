use log::{debug, info};
use serde::Serialize;
use std::time::Instant;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Read,
    path::PathBuf,
    process::{Child, ChildStderr, Command, Stdio},
};

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
    /// URL of the backend to report crashes
    backend_url: String,
}

impl<C: TargetsCollector> Scheduler<C> {
    /// Create a new [`Scheduler`].
    pub fn new(collector: C, corpus_directory: PathBuf, backend_url: String) -> Self {
        Self {
            collector,
            targets: HashMap::new(),
            tasks: HashMap::new(),
            corpus_directory,
            backend_url,
        }
    }

    /// Update the scheduler with the latest targets collected by the collector.
    pub fn update(&mut self) {
        let latest_targets = self.collector.collect_targets();

        debug!("latest_targets: {:?}", latest_targets);

        let old_targets_sha = self.targets.keys().collect::<HashSet<_>>();
        let latest_targets_sha = latest_targets.keys().collect::<HashSet<_>>();

        // NOTE: Can be optimized by iterating only once
        let targets_to_stop = old_targets_sha.difference(&latest_targets_sha);
        let targets_to_start = latest_targets_sha.difference(&old_targets_sha);
        let targets_to_check = latest_targets_sha.intersection(&old_targets_sha);

        // TODO: Handle errors
        for &target in targets_to_stop {
            self.tasks.remove(target).unwrap().child.kill().unwrap();
        }

        debug!("targets_to_start: {:?}", targets_to_start);

        for &target in targets_to_start {
            let fuzzer_path = latest_targets.get(target).unwrap().canonicalize().unwrap();
            let task = self.start_task(fuzzer_path);
            self.tasks.insert(*target, task);
        }

        for &target in targets_to_check {
            if self.targets.get(target) == latest_targets.get(target) {
                continue;
            }

            self.tasks.remove(target).unwrap().child.kill().unwrap();

            let fuzzer_path = latest_targets.get(target).unwrap().canonicalize().unwrap();
            let task = self.start_task(fuzzer_path);
            self.tasks.insert(*target, task);
        }

        debug!("self.tasks: {:?}", self.tasks);

        self.targets = latest_targets;

        // Display running tasks and their uptime
        self.log_running_tasks();
    }

    fn start_task(&self, fuzzer_path: PathBuf) -> Task {
        let fuzzer_name = fuzzer_path.file_name().unwrap().to_str().unwrap();
        let fuzzer_corpus = self.corpus_directory.join(fuzzer_name);
        if !fuzzer_corpus.exists() {
            fs::create_dir_all(&fuzzer_corpus).unwrap();
        }

        let mut child = Command::new(&fuzzer_path)
            .args(&[fuzzer_corpus.display().to_string()])
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        let child_stderr = child.stderr.take().unwrap();

        Task {
            child,
            stderr: child_stderr,
            path: fuzzer_path,
            started_at: Instant::now(),
        }
    }

    pub fn report_crashes(&mut self) {
        let mut tasks_to_remove = Vec::new();

        for (hash, task) in self.tasks.iter_mut() {
            match task.child.try_wait() {
                Ok(None) => {}
                Ok(Some(_)) => {
                    let mut stderr = String::new();
                    task.stderr.read_to_string(&mut stderr).unwrap();
                    info!("crash stderr: {:?}", stderr);

                    // Get crashing corpus instance
                    let crash = stderr
                        .lines()
                        .filter(|l| l.starts_with("artifact_prefix="))
                        .next()
                        .unwrap_or("Crash detected but no element in stderr");
                    let pattern = "Test unit written to ";
                    let crash_corpus = &crash[crash.find(pattern).unwrap_or(0) + pattern.len()..];
                    let crashing_instance =
                        fs::read(crash_corpus).unwrap_or("NO CRASH INFO".into());

                    // Send crashing input to the backend
                    let client = reqwest::blocking::Client::new();
                    let binary = fs::read(task.path.clone()).unwrap();
                    let res = client
                        .post(format!("{}/upload_crash", self.backend_url))
                        .json(&CrashReport {
                            report: stderr.as_bytes().to_vec(),
                            input: crashing_instance,
                            filename: task.path.display().to_string(),
                            program: binary,
                        })
                        .send()
                        .unwrap();

                    tasks_to_remove.push(*hash);
                }
                Err(_) => todo!(),
            }
        }

        for task_to_remove in tasks_to_remove.iter() {
            self.tasks.remove(task_to_remove);
        }
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
        for (_sha, task) in self.tasks.iter() {
            info!(
                "- {} running for {}s",
                task.path.display(),
                task.started_at.elapsed().as_secs()
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
}
