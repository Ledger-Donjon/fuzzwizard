-- SPDX-FileCopyrightText: © 2026 LEDGER SAS
-- SPDX-License-Identifier: MIT

CREATE TABLE fuzzer_status (
    id SERIAL PRIMARY KEY,
    scheduler_name TEXT NOT NULL,
    -- Hex-encoded SHA3-256 of the fuzzer binary, as used by the scheduler
    fuzzer_sha TEXT NOT NULL,
    fuzzer_path TEXT NOT NULL,
    -- One of: running, crashed, stopped
    status TEXT NOT NULL,
    -- Wall-clock start time, derived by the scheduler from its monotonic clock
    started_at TIMESTAMP,
    last_seen_at TIMESTAMP NOT NULL DEFAULT NOW(),
    UNIQUE (scheduler_name, fuzzer_sha)
);

CREATE INDEX idx_fuzzer_status_scheduler
    ON fuzzer_status (scheduler_name, last_seen_at DESC);

-- The report aggregates crash counts per scheduler, which is a sequential scan otherwise
CREATE INDEX IF NOT EXISTS idx_crashes_scheduler_time
    ON crashes (scheduler_name, created_at DESC);
