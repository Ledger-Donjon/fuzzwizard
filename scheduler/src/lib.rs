use sha3::{Digest, Sha3_256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Read,
    path::PathBuf,
    process::{Child, ChildStderr, Command, Stdio},
};

pub trait TargetsCollector {
    fn get_latest_targets(&self) -> HashMap<Sha, PathBuf>;
}

pub type Sha = [u8; 32];

#[derive(Debug)]
struct Task {
    child: Child,
    stderr: ChildStderr,
}

pub struct Scheduler<C: TargetsCollector> {
    collector: C,
    targets: HashMap<Sha, PathBuf>,
    task_processes: HashMap<Sha, Task>,
}

impl<C: TargetsCollector> Scheduler<C> {
    pub fn new(collector: C) -> Self {
        Self {
            collector,
            targets: HashMap::new(),
            task_processes: HashMap::new(),
        }
    }

    pub fn update(&mut self) {
        let latest_targets = self.collector.get_latest_targets();

        println!("{:?}", latest_targets);

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

        println!("{:?}", targets_to_start);

        for &target in targets_to_start {
            let mut child = Command::new(latest_targets.get(target).unwrap())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let child_stderr = child.stderr.take().unwrap();
            self.task_processes.insert(
                *target,
                Task {
                    child,
                    stderr: child_stderr,
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
            let mut child = Command::new(latest_targets.get(target).unwrap())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let child_stderr = child.stderr.take().unwrap();
            self.task_processes.insert(
                *target,
                Task {
                    child,
                    stderr: child_stderr,
                },
            );
        }

        println!("{:?}", self.task_processes);

        self.targets = latest_targets;
    }

    pub fn report_crashes(&mut self) {
        for (_, task) in self.task_processes.iter_mut() {
            match task.child.try_wait() {
                Ok(None) => {}
                Ok(Some(_)) => {
                    let mut stderr = String::new();
                    task.stderr.read_to_string(&mut stderr).unwrap();
                    // println!("{:?}", stderr);

                    // Get crashing corpus instance
                    let crash = stderr.lines().filter(|l| l.starts_with("artifact_prefix=")).next().unwrap();
                    let pattern = "Test unit written to ";
                    let crash_corpus = &crash[crash.find(pattern).unwrap() + pattern.len()..];
                    let crashing_instance = fs::read(crash_corpus).unwrap();
                    
                    // Send crashing input to the backend
                    // reqwest.post(...)
                }
                Err(_) => todo!(),
            }
        }
    }
}
