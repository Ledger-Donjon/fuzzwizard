-- SPDX-FileCopyrightText: © 2026 LEDGER SAS
-- SPDX-License-Identifier: MIT

-- Your SQL goes here

CREATE TABLE admin_events (
    id SERIAL PRIMARY KEY,
    event_type TEXT NOT NULL,
    message TEXT NOT NULL,
    metadata TEXT,
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),
    processed BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE INDEX idx_admin_events_processed ON admin_events(processed, created_at);
