-- SPDX-FileCopyrightText: © 2026 LEDGER SAS
-- SPDX-License-Identifier: MIT

-- This file should undo anything in `up.sql`

DROP INDEX IF EXISTS idx_admin_events_processed;
DROP TABLE IF EXISTS admin_events;
