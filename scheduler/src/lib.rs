use sha3::{Digest, Sha3_256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
};

pub trait TargetsCollector {
    fn get_latest_targets(&self) -> HashMap<Sha, PathBuf>;
}

type Sha = [u8; 32];

struct TargetList<R: TargetsCollector> {
    collector: R,
    targets: Vec<(Sha, PathBuf)>,
}

impl<R: TargetsCollector> TargetList<R> {
    fn update(&mut self) {
        let latest_targets = self.collector.get_latest_targets();

        // TODO: do the diffs and update
    }
}

#[cfg(test)]
mod tests {
    use super::*;
}
