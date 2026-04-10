-- SPDX-FileCopyrightText: © 2026 LEDGER SAS
-- SPDX-License-Identifier: MIT

-- Your SQL goes here

CREATE TABLE crashes (
	id SERIAL PRIMARY KEY, 
	input BYTEA NOT NULL,
	report BYTEA NOT NULL,
	filename TEXT NOT NULL,
	program BYTEA NOT NULL,
	scheduler_name TEXT NOT NULL,
	created_at TIMESTAMP NOT NULL default current_timestamp
);
