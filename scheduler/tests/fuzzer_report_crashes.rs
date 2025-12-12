use std::{collections::HashMap, fs, path::PathBuf, time::Duration};

use scheduler::{Scheduler, SchedulerConfig, Sha, TargetsCollector};
use sha3::{Digest, Sha3_256};

struct DirectoryCollector {
    targets_dir: PathBuf,
}

impl TargetsCollector for DirectoryCollector {
    fn collect_targets(&self) -> HashMap<Sha, PathBuf> {
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
        targets_dir: "./tests/test_targets/test_fuzzers/build".into(),
    };

    let config = SchedulerConfig {
        scheduler_name: "test_scheduler".to_string(),
        targets_directory: vec!["./tests/test_targets/test_fuzzers/build".into()],
        build_script: "".to_string(),
        backend_url: "http://127.0.0.1:0".to_string(),
        corpus_directory: std::env::temp_dir().to_string_lossy().into(),
        crashes_directory: std::env::temp_dir()
            .join("crashes")
            .to_string_lossy()
            .into(),
        coverage_directory: std::env::temp_dir()
            .join("coverage")
            .to_string_lossy()
            .into(),
        container_image: None,
        container_options: None,
        has_coverage: false,
    };

    let mut scheduler = Scheduler::new(collector, config, None);

    let _ = scheduler.update("./logs");

    std::thread::sleep(Duration::from_secs(1));
    let _ = scheduler.report_crashes();
}
