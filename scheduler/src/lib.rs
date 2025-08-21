use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Read,
    path::PathBuf,
    process::{Child, ChildStderr, Command, Stdio},
};
use log::{debug, info};
use serde::Serialize;
use std::time::Instant;

pub trait TargetsCollector {
    fn get_latest_targets(&self) -> HashMap<Sha, PathBuf>;
}

pub type Sha = [u8; 32];

#[derive(Debug)]
struct Task {
    child: Child,
    stderr: ChildStderr,
    path: PathBuf,
    started_at: Instant,
}

pub struct Scheduler<C: TargetsCollector> {
    collector: C,
    targets: HashMap<Sha, PathBuf>,
    task_processes: HashMap<Sha, Task>,
    corpus_directory: PathBuf,
}

impl<C: TargetsCollector> Scheduler<C> {
    pub fn new(collector: C, corpus_directory: PathBuf) -> Self {
        Self {
            collector,
            targets: HashMap::new(),
            task_processes: HashMap::new(),
            corpus_directory,
        }
    }

    pub fn update(&mut self) {
        let latest_targets = self.collector.get_latest_targets();

        debug!("latest_targets: {:?}", latest_targets);

        let old_targets_sha = self.targets.keys().collect::<HashSet<_>>();
        let latest_targets_sha = latest_targets.keys().collect::<HashSet<_>>();

        // NOTE: Can be optimized by iterating only once
        let targets_to_stop = old_targets_sha.difference(&latest_targets_sha);
        let targets_to_start = latest_targets_sha.difference(&old_targets_sha);
        let targets_to_check = latest_targets_sha.intersection(&old_targets_sha);

        // TODO: Handle errors
        for &target in targets_to_stop {
            self.task_processes
                .remove(target)
                .unwrap()
                .child
                .kill()
                .unwrap();
        }

        debug!("targets_to_start: {:?}", targets_to_start);


        for &target in targets_to_start {
           
            let path = latest_targets.get(target).unwrap().canonicalize().unwrap();
            let fuzzer_name = path.file_name().unwrap().to_str().unwrap();
            let fuzzer_corpus = self.corpus_directory.join(fuzzer_name);
            if !fuzzer_corpus.exists() {
                fs::create_dir_all(&fuzzer_corpus).unwrap();
            }
            let mut child = Command::new(latest_targets.get(target).unwrap())
            .args(&[fuzzer_corpus.display().to_string()])
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

            let child_stderr = child.stderr.take().unwrap();
            self.task_processes.insert(
                *target,
                Task {
                    child,
                    stderr: child_stderr,
                    path,
                    started_at: Instant::now(),
                },
            );
        }

        for &target in targets_to_check {
            if self.targets.get(target) == latest_targets.get(target) {
                continue;
            }

            self.task_processes
                .remove(target)
                .unwrap()
                .child
                .kill()
                .unwrap();

            let path = latest_targets.get(target).unwrap().canonicalize().unwrap();
            let fuzzer_name = path.file_name().unwrap().to_str().unwrap();
            let fuzzer_corpus = self.corpus_directory.join(fuzzer_name);
            if !fuzzer_corpus.exists() {
                fs::create_dir_all(&fuzzer_corpus).unwrap();
            }
            let mut child = Command::new(latest_targets.get(target).unwrap())
                .args(&[fuzzer_corpus.display().to_string()])
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let child_stderr = child.stderr.take().unwrap();
            
            self.task_processes.insert(
                *target,
                Task {
                    child,
                    stderr: child_stderr,
                    path,
                    started_at: Instant::now(),
                },
            );
        }

        debug!("self.task_processes: {:?}", self.task_processes);

        self.targets = latest_targets;

        // Display running tasks and their uptime
        self.log_running_tasks();
    }

    pub fn report_crashes(&mut self) {
        let mut tasks_to_remove = Vec::new();

        for (hash, task) in self.task_processes.iter_mut() {
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
                    let crashing_instance = fs::read(crash_corpus).unwrap_or("NO CRASH INFO".into());


                    // Send crashing input to the backend
                    let client = reqwest::blocking::Client::new();
                    let binary =  fs::read(task.path.clone()).unwrap();
                    let res = client
                        .post("http://localhost:3000/upload_crash")
                        .json(&CrashReport {
                            report: stderr.as_bytes().to_vec(),
                            input: crashing_instance,
                            filename:task.path.display().to_string(),
                            program:binary,
                        })
                        .send()
                        .unwrap();
                    info!("upload_crash response: {:?}", res);

                    tasks_to_remove.push(hash.clone());
                }
                Err(_) => todo!(),
            }
        }

        for task_to_remove in tasks_to_remove.iter() {
            self.task_processes.remove(task_to_remove);
        }
    }

    pub fn running_tasks_info(&self) -> Vec<(Sha, PathBuf, std::time::Duration)> {
        self.task_processes
            .iter()
            .map(|(sha, task)| (*sha, task.path.clone(), task.started_at.elapsed()))
            .collect()
    }

    pub fn log_running_tasks(&self) {
        if self.task_processes.is_empty() {
            info!("No tasks running");
            return;
        }
        info!("Running tasks:");
        for (_sha, task) in self.task_processes.iter() {
            info!(
                "- {} running for {}s",
                task.path.display(),
                task.started_at.elapsed().as_secs()
            );
        }
    }
}

#[derive(Serialize, Clone)]
struct CrashReport {
    report: Vec<u8>,
    input: Vec<u8>,
    filename: String,
    program: Vec<u8>
}
