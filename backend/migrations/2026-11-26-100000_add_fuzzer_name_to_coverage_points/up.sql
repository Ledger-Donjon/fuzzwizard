-- SPDX-FileCopyrightText: © 2026 LEDGER SAS
-- SPDX-License-Identifier: MIT

-- File name of the fuzzer binary, as sent by the scheduler with each point.
-- NULL for points that a scheduler older than this column sent.
ALTER TABLE fuzzer_coverage_points ADD COLUMN fuzzer_name TEXT;

-- Points of the binaries that are still reported take the name from their
-- path. Points of binaries that are gone stay NULL.
UPDATE fuzzer_coverage_points AS p
SET fuzzer_name = regexp_replace(s.fuzzer_path, '^.*/', '')
FROM fuzzer_status AS s
WHERE s.scheduler_name = p.scheduler_name
  AND s.fuzzer_sha = p.fuzzer_sha;
