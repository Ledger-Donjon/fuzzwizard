use std::{collections::HashMap, fs, os::unix::fs::PermissionsExt, path::PathBuf, time::Duration};

use assert_fs::prelude::{FileWriteStr, PathChild};
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
fn directory_collector() {
    let temp_dir = assert_fs::TempDir::new().unwrap();

    let collector = DirectoryCollector {
        targets_dir: temp_dir.to_path_buf(),
    };

    let mut scheduler = Scheduler::new(collector);

    let a_sh = temp_dir.child("a.sh");
    a_sh.write_str(
        r#"#!/bin/sh

while /bin/true; do
    echo a
    sleep 1
done
"#,
    )
    .unwrap();
    fs::set_permissions(a_sh.path(), fs::Permissions::from_mode(0o775)).unwrap();

    let b_sh = temp_dir.child("b.sh");
    b_sh.write_str(
        r#"#!/bin/sh

while /bin/true; do
    echo b
    sleep 1
done
"#,
    )
    .unwrap();
    fs::set_permissions(b_sh.path(), fs::Permissions::from_mode(0o775)).unwrap();

    scheduler.update();

    std::thread::sleep(Duration::from_secs(10));

    fs::remove_file(a_sh.path()).unwrap();

    let c_sh = temp_dir.child("c.sh");
    c_sh.write_str(
        r#"#!/bin/sh

while /bin/true; do
    echo c
    sleep 1
done
"#,
    )
    .unwrap();
    fs::set_permissions(c_sh.path(), fs::Permissions::from_mode(0o775)).unwrap();

    scheduler.update();

    std::thread::sleep(Duration::from_secs(10));
}
