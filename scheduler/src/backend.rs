// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use std::sync::OnceLock;
use std::time::Duration;

use log::debug;
use serde::Serialize;

use crate::coverage;

/// Fuzzing engine reported alongside a coverage snapshot.
const FUZZER_ENGINE: &str = "libfuzzer";

/// Everything a coverage snapshot needs, captured on the scheduler thread so
/// that the `llvm-profdata` / `llvm-cov` run can happen detached.
pub(crate) struct SnapshotJob {
    pub request: coverage::CoverageRequest,
    pub fingerprint: String,
    pub num_fuzzers: i32,
    pub scheduler_name: String,
    pub backend_url: String,
}

#[derive(Serialize)]
struct SnapshotPayload {
    scheduler_name: String,
    fuzzer_engine: &'static str,
    build_fingerprint: String,
    num_fuzzers: i32,
    metrics: Vec<coverage::CoverageMetricEntry>,
    files: Vec<coverage::CoverageFileEntry>,
}

impl SnapshotJob {
    /// Merge the profiles, export the summary and post it. Every failure is
    /// silent beyond a debug log: coverage reporting must never disturb fuzzing.
    pub fn run(self) {
        let Some(summary) = self.request.summary() else {
            debug!("Coverage snapshot skipped: no usable profile data");
            return;
        };

        let payload = SnapshotPayload {
            scheduler_name: self.scheduler_name.clone(),
            fuzzer_engine: FUZZER_ENGINE,
            build_fingerprint: self.fingerprint,
            num_fuzzers: self.num_fuzzers,
            metrics: summary.metrics,
            files: summary.files,
        };

        post_to_backend(
            &self.backend_url,
            "coverage_snapshots",
            &payload,
            Duration::from_secs(30),
            &self.scheduler_name,
        );
    }
}

/// POST a payload to a backend endpoint, logging the outcome.
///
/// Telemetry must never disturb fuzzing, so failures are only logged.
pub(crate) fn post_to_backend<T: Serialize>(
    backend_url: &str,
    endpoint: &str,
    payload: &T,
    timeout: Duration,
    scheduler_name: &str,
) {
    let url = format!("{backend_url}/{endpoint}");
    match http_client()
        .post(&url)
        .json(payload)
        .timeout(timeout)
        .send()
    {
        Ok(resp) if resp.status().is_success() => {
            debug!("Sent {endpoint} for scheduler={scheduler_name}");
        }
        Ok(resp) => {
            debug!("{endpoint} rejected by backend: {}", resp.status());
        }
        Err(e) => {
            debug!("Failed to send {endpoint}: {e}");
        }
    }
}

/// Shared HTTP client, so the periodic heartbeats reuse connections instead of
/// building a new TLS stack on every scheduler tick.
pub(crate) fn http_client() -> &'static reqwest::blocking::Client {
    static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::blocking::Client::new)
}
