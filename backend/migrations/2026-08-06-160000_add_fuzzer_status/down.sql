-- SPDX-FileCopyrightText: © 2026 LEDGER SAS
-- SPDX-License-Identifier: MIT

-- idx_crashes_scheduler_time is kept: up.sql creates it only if missing, so it
-- may predate this migration, and an extra index on crashes is harmless.
DROP TABLE IF EXISTS fuzzer_status;
