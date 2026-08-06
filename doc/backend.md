<!-- SPDX-FileCopyrightText: © 2026 LEDGER SAS -->
<!-- SPDX-License-Identifier: MIT -->

# Backend Documentation

## Overview

The backend is a REST API that manages crash storage and retrieval in a PostgreSQL database. It provides endpoints for:
- Storing crashes reported by schedulers
- Retrieving crashes (all, last N, by ID)
- Managing administrative events
- Storing coverage snapshots (aggregated metrics per build fingerprint)
- Maintenance (preview and delete old records)


## API Endpoints

### Crashes

- **`GET /crashes`**: Get all crashes
- **`GET /crashes/last`**: Get last crash
- **`GET /crashes/last/{number}`**: Get last N crashes
- **`GET /crashes/{id}`**: Get crash by ID
- **`GET /crashes/count`**: Get total crash count
- **`GET /crashes/ids?after={date}`**: Get crash IDs since date
- **`POST /crashes`**: Upload new crash
- **`POST /collect_crashes`**: Collect crash artifacts to temp directory

### Admin Events

- **`POST /admin_events`**: Create admin event
- **`GET /admin_events/unprocessed`**: Get unprocessed events
- **`POST /admin_events/{id}/mark_processed`**: Mark event as processed

### Coverage Snapshots

- **`POST /coverage_snapshots`**: Create a coverage snapshot (deduplicated by fingerprint)
- **`GET /coverage_snapshots?scheduler=&after=&before=&limit=`**: List snapshots (time series)
- **`GET /coverage_snapshots/latest?scheduler=`**: Get latest snapshot for a scheduler

### Maintenance

- **`POST /maintenance/preview`**: Preview records that would be deleted (requires admin token)
- **`POST /maintenance/delete`**: Delete old records in batches (requires admin token)

## Crash Data Structure

```json
{
  "id": 1,
  "report": [/* bytes */],
  "input": [/* bytes */],
  "filename": "fuzzer_name",
  "program": [/* bytes */],
  "scheduler_name": "scheduler1",
  "created_at": "2025-01-15T14:30:45.123"
}
```

## Size Limits

- **HTTP body**: 500 MB (configurable)
- **Program**: 100 MB
- **Input**: 10 MB
- **Report**: 1 MB


## Usage

### Standalone

```bash
export DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/fuzzwizard
cargo run -p backend -- --database-url $DATABASE_URL
```

Or using the release binary:

```bash
./target/release/backend --database-url postgres://postgres:postgres@127.0.0.1:5432/fuzzwizard
```

### Via FuzzTUI

The backend is typically launched automatically by FuzzTUI. You can configure FuzzTUI to use a release binary by setting `backend_binary` in your `config.json` (see [config.md](config.md) for details). If not set, FuzzTUI will use `cargo run -p backend` to launch it.

The backend listens on `0.0.0.0:3000`.

## Connection Pool

- **Max connections**: 10
- **Min idle**: 2
- **Connection timeout**: 5 seconds


## Coverage Snapshots

Coverage snapshots are automatically captured by the scheduler just before binary transitions (when the set of instrumented binaries changes). Each snapshot contains aggregated metrics (lines, functions, regions, branches) in a universal format compatible with libFuzzer/LLVM today and AFL in the future.

Snapshots are deduplicated by `build_fingerprint` (the canonical sorted SHA3-256 hashes of all binaries). A snapshot with an identical fingerprint for the same scheduler is stored only once.

### Coverage Snapshot Structure

```json
{
  "scheduler_name": "my-scheduler",
  "fuzzer_engine": "libfuzzer",
  "build_fingerprint": "abcdef1234...:fedcba4321...",
  "num_fuzzers": 3,
  "metrics": [
    {"metric_name": "lines", "covered": 450, "total": 1000},
    {"metric_name": "functions", "covered": 30, "total": 50},
    {"metric_name": "regions", "covered": 200, "total": 500}
  ]
}
```


## Maintenance / Cleanup

The `fuzzwizard-cleanup` CLI tool provides interactive, safe cleanup of old backend data without stopping the TUI. It communicates through the backend's maintenance API.

### Default Retention

| Category | Default |
|----------|---------|
| Crashes | 6 months |
| Admin events (processed) | 30 days |
| Coverage snapshots | Kept indefinitely (override with `--coverage-before`) |

### Usage

```bash
# Preview what would be deleted (dry-run)
fuzzwizard-cleanup --dry-run

# Interactive cleanup with defaults
fuzzwizard-cleanup

# Custom retention: crashes older than 3 months, events older than 7 days
fuzzwizard-cleanup --crashes-months 3 --admin-events-days 7

# Also purge old coverage snapshots
fuzzwizard-cleanup --coverage-before "2025-06-01"

# Specify a non-default backend URL
fuzzwizard-cleanup --backend-url http://my-host:3000
```

The tool always shows a preview of what will be deleted and requires explicit confirmation before proceeding. Deletions are performed in small batches (500 records at a time) to avoid blocking concurrent TUI operations.


## See Also

- [install.md](install.md): Installation instructions
- [config.md](config.md): Configuration documentation
- [notificator.md](notificator.md): Notificator documentation
