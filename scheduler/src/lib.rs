use sha3::{Digest, Sha3_256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
    process::{Child, Command, Stdio},
};

pub trait TargetsCollector {
    fn get_latest_targets(&self) -> HashMap<Sha, PathBuf>;
}

type Sha = [u8; 32];

struct Target {
    /// Path to the target binary. The path is also used as an identifier.
    path: PathBuf,
    /// Hash of the target binary.
    hash: Sha,
}

struct Scheduler<R: TargetsCollector> {
    collector: R,
    targets: HashMap<Sha, PathBuf>,
    task_processes: HashMap<Sha, Child>,
}

impl<R: TargetsCollector> Scheduler<R> {
    fn update(&mut self) {
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
            self.task_processes.remove(target).unwrap().kill().unwrap();
        }

        println!("{:?}", targets_to_start);

        for &target in targets_to_start {
            let child = Command::new(latest_targets.get(target).unwrap())
                .spawn()
                .unwrap();
            self.task_processes.insert(*target, child);
        }

        for &target in targets_to_check {
            if self.targets.get(target) == latest_targets.get(target) {
                continue;
            }

            self.task_processes.remove(target).unwrap().kill().unwrap();
            let child = Command::new(latest_targets.get(target).unwrap())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            self.task_processes.insert(*target, child);
        }

        println!("{:?}", self.task_processes);

        self.targets = latest_targets;
    }
}

#[cfg(test)]
mod tests {
    use std::{fs::read_dir, hash::Hash, time::Duration};

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
}
