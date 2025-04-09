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

pub struct Scheduler<R: TargetsCollector> {
    collector: R,
    targets: HashMap<Sha, PathBuf>,
    task_processes: HashMap<Sha, Task>,
}

impl<R: TargetsCollector> Scheduler<R> {
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
                    println!("{:?}", stderr);
                }
                Err(_) => todo!(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{fs::read_dir, time::Duration};

    use super::*;

    struct Collector {
        targets_dir: PathBuf,
    }

    impl TargetsCollector for Collector {
        fn get_latest_targets(&self) -> HashMap<Sha, PathBuf> {
            let mut latest_targets = HashMap::new();

            for direntry in read_dir(&self.targets_dir).unwrap() {
                let direntry = direntry.unwrap();

                let target_path = direntry.path();
                let target_hash = Sha3_256::digest(fs::read(&target_path).unwrap());

                latest_targets.insert(target_hash.into(), target_path);
            }

            latest_targets
        }
    }

    #[test]
    fn test() {
        let collector = Collector {
            targets_dir: "./test_targets".into(),
        };

        let mut scheduler = Scheduler {
            collector,
            targets: HashMap::new(),
            task_processes: HashMap::new(),
        };

        scheduler.update();

        std::thread::sleep(Duration::from_secs(10));

        scheduler.update();

        std::thread::sleep(Duration::from_secs(10));
    }

    #[test]
    fn test2() {
        let collector = Collector {
            targets_dir: "../testing_targets/build".into(),
        };

        let mut scheduler = Scheduler {
            collector,
            targets: HashMap::new(),
            task_processes: HashMap::new(),
        };

        scheduler.update();

        std::thread::sleep(Duration::from_secs(1));
        scheduler.report_crashes();
    }
}
