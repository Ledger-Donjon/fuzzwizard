-- SPDX-FileCopyrightText: © 2026 LEDGER SAS
-- SPDX-License-Identifier: MIT

CREATE TABLE coverage_snapshots (
    id SERIAL PRIMARY KEY,
    scheduler_name TEXT NOT NULL,
    fuzzer_engine TEXT NOT NULL,
    -- Canonical fingerprint: sorted, concatenated SHA3-256 hashes of all binaries
    build_fingerprint TEXT NOT NULL,
    num_fuzzers INT NOT NULL,
    captured_at TIMESTAMP NOT NULL DEFAULT NOW(),
    schema_version INT NOT NULL DEFAULT 1
);

CREATE INDEX idx_coverage_snapshots_scheduler_time
    ON coverage_snapshots (scheduler_name, captured_at DESC);

CREATE INDEX idx_coverage_snapshots_fingerprint
    ON coverage_snapshots (build_fingerprint, captured_at DESC);

CREATE TABLE coverage_metrics (
    id SERIAL PRIMARY KEY,
    snapshot_id INT NOT NULL REFERENCES coverage_snapshots(id) ON DELETE CASCADE,
    metric_name TEXT NOT NULL,
    covered INT NOT NULL,
    total INT NOT NULL,
    UNIQUE (snapshot_id, metric_name)
);

CREATE INDEX idx_coverage_metrics_snapshot
    ON coverage_metrics (snapshot_id);
