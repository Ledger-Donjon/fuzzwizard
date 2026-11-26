-- SPDX-FileCopyrightText: © 2026 LEDGER SAS
-- SPDX-License-Identifier: MIT

-- Total running time of each fuzzer binary, over all its runs and over the
-- restarts of its scheduler. Unlike fuzzer_status, a row stays when the
-- binary is no longer reported.
CREATE TABLE fuzzer_runtime (
    id SERIAL PRIMARY KEY,
    scheduler_name TEXT NOT NULL,
    -- Hex-encoded SHA3-256 of the fuzzer binary, as used by the scheduler
    fuzzer_sha TEXT NOT NULL,
    total_runtime_seconds BIGINT NOT NULL DEFAULT 0,
    -- Last runtime counter of the scheduler process that reported it. The
    -- counter starts at 0 when the scheduler restarts.
    last_reported_seconds BIGINT NOT NULL,
    scheduler_instance TEXT NOT NULL,
    updated_at TIMESTAMP NOT NULL DEFAULT NOW(),
    UNIQUE (scheduler_name, fuzzer_sha)
);
