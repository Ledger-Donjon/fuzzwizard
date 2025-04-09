use std::{collections::HashMap, fs, path::PathBuf, time::Duration};

use scheduler::{Scheduler, Sha, TargetsCollector};
use sha3::{Digest, Sha3_256};

struct DirectoryCollector {
    targets_dir: PathBuf,
}

impl TargetsCollector for DirectoryCollector {
    fn get_latest_targets(&self) -> HashMap<Sha, PathBuf> {
        let mut latest_targets = HashMap::new();

        for direntry in fs::read_dir(&self.targets_dir).unwrap() {
            let direntry = direntry.unwrap();

            let target_path = direntry.path();
            let target_hash = Sha3_256::digest(fs::read(&target_path).unwrap());

            latest_targets.insert(target_hash.into(), target_path);
        }

        latest_targets
    }
}

#[test]
fn fuzzer_report_crashes() {
    let collector = DirectoryCollector {
        targets_dir: "./tests/test_targets/buffer_overflow/build".into(),
    };

    let mut scheduler = Scheduler::new(collector);

    scheduler.update();

    std::thread::sleep(Duration::from_secs(1));
    scheduler.report_crashes();
}
