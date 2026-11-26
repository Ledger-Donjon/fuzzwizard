-- SPDX-FileCopyrightText: © 2026 LEDGER SAS
-- SPDX-License-Identifier: MIT

-- One row for each fuzzer binary at each snapshot tick: the coverage time
-- series of the binary.
CREATE TABLE fuzzer_coverage_points (
    id SERIAL PRIMARY KEY,
    snapshot_id INT NOT NULL REFERENCES coverage_snapshots(id) ON DELETE CASCADE,
    scheduler_name TEXT NOT NULL,
    -- Hex-encoded SHA3-256 of the fuzzer binary, as used by the scheduler
    fuzzer_sha TEXT NOT NULL,
    -- Same time as the snapshot that carried the point
    captured_at TIMESTAMP NOT NULL,
    -- Total runtime of the binary at captured_at, NULL if fuzzer_runtime has no row
    runtime_seconds BIGINT,
    lines_covered INT NOT NULL,
    lines_total INT NOT NULL,
    functions_covered INT NOT NULL,
    functions_total INT NOT NULL,
    regions_covered INT NOT NULL,
    regions_total INT NOT NULL,
    branches_covered INT NOT NULL,
    branches_total INT NOT NULL,
    -- Also serves lookups by snapshot_id, its leading column.
    UNIQUE (snapshot_id, fuzzer_sha)
);

CREATE INDEX idx_fuzzer_coverage_points_series
    ON fuzzer_coverage_points (scheduler_name, fuzzer_sha, captured_at DESC);
