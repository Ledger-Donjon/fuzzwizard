-- SPDX-FileCopyrightText: © 2026 LEDGER SAS
-- SPDX-License-Identifier: MIT

-- Source paths repeat identically in every snapshot, so they are interned here
-- instead of being duplicated on each coverage_files row.
CREATE TABLE coverage_paths (
    id SERIAL PRIMARY KEY,
    path TEXT NOT NULL,
    UNIQUE (path)
);

CREATE TABLE coverage_files (
    id SERIAL PRIMARY KEY,
    snapshot_id INT NOT NULL REFERENCES coverage_snapshots(id) ON DELETE CASCADE,
    path_id INT NOT NULL REFERENCES coverage_paths(id),
    lines_covered INT NOT NULL,
    lines_total INT NOT NULL,
    functions_covered INT NOT NULL,
    functions_total INT NOT NULL,
    regions_covered INT NOT NULL,
    regions_total INT NOT NULL,
    branches_covered INT NOT NULL,
    branches_total INT NOT NULL,
    UNIQUE (snapshot_id, path_id)
);

CREATE INDEX idx_coverage_files_snapshot ON coverage_files (snapshot_id);
CREATE INDEX idx_coverage_files_path ON coverage_files (path_id);
