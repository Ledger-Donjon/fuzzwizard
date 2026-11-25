<!-- SPDX-FileCopyrightText: © 2026 LEDGER SAS -->
<!-- SPDX-License-Identifier: MIT -->

# Backend Documentation

## Overview

The backend is a REST API that manages crash storage and retrieval in a PostgreSQL database. It provides endpoints for:
- Storing crashes reported by schedulers
- Retrieving crashes (all, last N, by ID)
- Managing administrative events
- Storing coverage snapshots (aggregated metrics per build fingerprint)
- Storing the coverage time series of each fuzzer binary
- Tracking live fuzzer status through scheduler heartbeats
- Tracking the total runtime of each fuzzer binary
- Reporting (aggregated status, coverage and crash counts, as JSON or HTML)
- Maintenance (preview and delete old records)


## API Endpoints

### Crashes

- **`GET /crashes?limit={n}`**: List crash metadata, newest first (`limit` defaults to 100, capped at 1000)
- **`GET /crashes/last`**: Get last crash, with its `report`, `input` and `program` blobs
- **`GET /crashes/last/{number}`**: List the metadata of the last N crashes (`number` clamped to 1..1000)
- **`GET /crashes/{id}`**: Get crash by ID, with its `report`, `input` and `program` blobs
- **`GET /crashes/count`**: Get total crash count
- **`GET /crashes/ids?after={date}`**: Get crash IDs since date
- **`GET /crashes/metadata?after={date}`**: Get id, filename, scheduler and date of the crashes since date, oldest first, without their blobs (at most 1000 per call)
- **`POST /crashes`**: Upload new crash
- **`POST /collect_crashes`**: Write crash artifacts into a private temp directory (mode `0700`). The JSON response is the directory path.

Listing endpoints return metadata only (`id`, `filename`, `scheduler_name`,
`created_at`). Fetch `/crashes/{id}` to get the artifacts of a given crash.

### Admin Events

- **`POST /admin_events`**: Create admin event
- **`GET /admin_events/unprocessed`**: Get unprocessed events
- **`POST /admin_events/{id}/mark_processed`**: Mark event as processed

### Coverage Snapshots

- **`POST /coverage_snapshots`**: Create a coverage snapshot (deduplicated by fingerprint)
- **`GET /coverage_snapshots?scheduler=&after=&before=&limit=`**: List snapshots (time series). `limit` defaults to 100 and is capped at 1000.
- **`GET /coverage_snapshots/latest?scheduler=`**: Get latest snapshot for a scheduler
- **`GET /coverage_snapshots/fuzzers?scheduler=&sha=&after=&before=&limit=`**: List the coverage points of the fuzzer binaries (time series), newest first. `limit` defaults to 100 and is capped at 1000.

### Fuzzer Status

- **`POST /fuzzer_status`**: Scheduler heartbeat declaring the status of all its fuzzers

### Reporting

- **`GET /report?scheduler=`**: Aggregated report as JSON
- **`GET /report/html?scheduler=`**: Same report rendered as a standalone HTML page

### Maintenance

- **`POST /maintenance/preview`**: Preview records that would be deleted
- **`POST /maintenance/delete`**: Delete old records in batches

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
- **Filename**: 4096 characters
- **Scheduler name**: 256 characters


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

The backend listens on `127.0.0.1:3000`, so it is only reachable from the machine (or container) it runs on. To open the HTML report from another host, tunnel the port over SSH rather than exposing it.

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
  ],
  "fuzzers": [
    {
      "sha": "abcdef1234...",
      "metrics": [
        {"metric_name": "lines", "covered": 300, "total": 800},
        {"metric_name": "functions", "covered": 20, "total": 40}
      ]
    }
  ]
}
```

### Coverage of Each Fuzzer

Each snapshot also carries the coverage of each fuzzer binary alone, in the `fuzzers` array. The scheduler measures each binary with its own profile only. Thus the numbers show what one fuzzer reached, and not the merged result of all fuzzers.

The backend stores one row for each binary and each snapshot in the `fuzzer_coverage_points` table. These rows form the coverage time series of the binary. A row has these fields:

- `scheduler_name` and `fuzzer_sha` identify the series. `fuzzer_sha` is the hex hash of the binary, the same value as in the heartbeat. A rebuild gives a new hash, so a rebuild starts a new series.
- `captured_at` is the time of the snapshot that carried the point. The project point and the fuzzer points of one snapshot have the same time.
- `runtime_seconds` is the total runtime of the binary at `captured_at` (see [Total Runtime](#total-runtime)). The value is `null` if the backend has no total for the binary. The value can be late by one heartbeat interval (60 seconds).
- `lines_*`, `functions_*`, `regions_*`, and `branches_*` are the covered and total counts. A metric that the scheduler did not send is stored as `0 / 0`.

A scheduler adds a point for each binary at each snapshot: every `coverage_snapshot_interval_minutes` (see [config.md](config.md)) and at each build transition. The rule that ignores a second snapshot of the same build within five minutes also applies to the points. A snapshot without a `fuzzers` array is valid. Schedulers that are older than this field send no points.

Run the migration before you deploy the backend. The backend writes the points in the snapshot transaction, so a missing table fails every snapshot.

`GET /coverage_snapshots/fuzzers` returns the points. Use `sha` to get the series of one binary, and `after` and `before` (format `%Y-%m-%d %H:%M:%S`) to select a time range.

```json
[
  {
    "scheduler_name": "my-scheduler",
    "fuzzer_sha": "abcdef1234...",
    "captured_at": "2026-11-25T14:00:00",
    "runtime_seconds": 86400,
    "lines_covered": 300, "lines_total": 800,
    "functions_covered": 20, "functions_total": 40,
    "regions_covered": 0, "regions_total": 0,
    "branches_covered": 0, "branches_total": 0
  }
]
```


## Fuzzer Status (Heartbeat)

The backend has no visibility on running processes, so each scheduler pushes the state of its fuzzers to `POST /fuzzer_status` on every loop iteration (every 60 seconds). This is what makes a live status and a meaningful "last update" date possible.

The payload declares the **complete** set of fuzzers known to the scheduler. Rows are upserted on `(scheduler_name, fuzzer_sha)`, and any fuzzer missing from the payload is deleted, so a binary that disappears from the targets directory also disappears from the report.

`started_at` is wall-clock time: the scheduler tracks uptime with a monotonic clock internally and converts it before sending. Fuzzers that were discovered but never spawned are reported as `stopped` with no `started_at`.

```json
{
  "scheduler_name": "my-scheduler",
  "scheduler_instance": "9f2c41d07a6b3e58",
  "fuzzers": [
    {
      "sha": "abcdef1234...",
      "path": "/targets/fuzz_parser",
      "status": "running",
      "uptime_seconds": 3600,
      "runtime_seconds": 86400
    }
  ]
}
```

Accepted statuses are `running`, `crashed` and `stopped`. The endpoint returns `204 No Content`, and a heartbeat failure never interrupts fuzzing: the scheduler only logs it at debug level.

### Total Runtime

The total runtime is the time during which a fuzzer binary ran, over all its runs. The backend keeps one total for each binary, identified by its hash. The total stays correct when the fuzzer restarts and when the scheduler restarts.

The scheduler sends two fields for the total runtime:

- `scheduler_instance` is a random value that the scheduler process sets when it starts. A new value tells the backend that the scheduler restarted.
- `runtime_seconds` is the time that the binary ran since the scheduler process started. The counter adds up all runs of the binary. Time in the `crashed` or `stopped` status does not count. A fuzzer with `-jobs=4` counts once, because the counter measures wall-clock time and not CPU time.

The backend stores the total in the `fuzzer_runtime` table. For each heartbeat, it adds the growth of the counter since the previous heartbeat. If `scheduler_instance` changed, or if the counter is smaller than before, the scheduler restarted. The backend then adds the full counter. Thus a scheduler restart loses at most the last heartbeat interval (60 seconds).

If a heartbeat has no `scheduler_instance`, the backend does not change the total. Schedulers that are older than this field send no total.


## Reporting

`GET /report` aggregates, per scheduler: fuzzer counts by status with per-fuzzer detail, a coverage synthesis, and crash counters (total, last 24h, date of the last crash). Pass `?scheduler=<name>` to restrict the report to a single scheduler.

A scheduler appears in the report as soon as it left a trace in any table, even if it never sent a heartbeat — its health is then `unknown`.

### Coverage Synthesis

The coverage block does not just echo the last snapshot: it summarises the whole history stored for the scheduler.

| Field | Meaning |
|-------|---------|
| `captured_at`, `fuzzer_engine`, `build_fingerprint`, `num_fuzzers` | The latest snapshot |
| `previous_captured_at` | Snapshot the deltas are measured against |
| `snapshots_total`, `distinct_builds`, `first_captured_at` | Counted over every snapshot ever stored |
| `history_truncated` | `true` when older snapshots fall outside the scanned window |

And per metric (`lines`, `functions`, `regions`, `branches`):

| Field | Meaning |
|-------|---------|
| `covered`, `total`, `percent` | Current value |
| `previous_covered`, `previous_percent` | Value in the previous snapshot |
| `delta_covered`, `delta_percent` | Progress since then, in percentage points |
| `peak_percent`, `peak_captured_at` | Best ratio ever reached, and when |
| `history` | Chronological series, oldest first, used to draw the trend |

Trends are computed over the 500 most recent snapshots, and each `history` series is capped to its last 40 points; the counters above always reflect the full table.

Schedulers snapshot on a timer (hourly by default, see `coverage_snapshot_interval_minutes` in [config.md](config.md)) as well as on every build transition, so the series tracks coverage growth *within* a campaign and not only across rebuilds. Two snapshots of the same build closer than five minutes apart are treated as a duplicate send and ignored, which keeps a retry from creating a spurious point.

`totals.coverage` sums the latest snapshot of every scheduler per metric, giving a project-wide ratio.

### Coverage of Each Fuzzer in the Report

Each entry of `fuzzer_details` has a `coverage` list. The list holds one item for each metric that has a total above 0, with the same fields as the metrics of the coverage block (`covered`, `percent`, `delta_percent`, `peak_percent`, `history`). Each point of the `history` also has `runtime_seconds`. The backend builds the list from the points of the 500 most recent snapshots of the scheduler, and caps each `history` to its last 40 points. If no snapshot measured the binary, the list is empty.

### Per-File Detail

`llvm-cov export` already breaks its totals down per source file, so each snapshot also stores that breakdown. `files_total` counts the instrumented files, and `files` lists the 100 with the most uncovered lines first — the ranking that points at where fuzzing has the most left to gain.

| Field | Meaning |
|-------|---------|
| `path` | Source file, as reported by `llvm-cov` |
| `lines_covered`, `lines_total`, `lines_percent` | Line coverage of the file |
| `functions_*`, `regions_*`, `branches_*` | Same counters for the other metrics |
| `uncovered_lines` | Lines left to reach; the sort key |
| `delta_lines_percent` | Movement since the previous snapshot, in percentage points |
| `is_new` | The file was absent from the previous snapshot |

A scheduler records at most 5000 files per snapshot, keeping the least covered ones beyond that. Source paths are interned in `coverage_paths` so that repeating them across snapshots costs an integer rather than a string.

Because snapshots are now periodic, only the **newest snapshot of each build** keeps its per-file rows: sending a fresh detail for a build discards the one it supersedes. The totals therefore form a full time series while the file rows stay proportional to the number of builds rather than to the number of snapshots. The trade-off is that `delta_lines_percent` compares against the last snapshot that still carries detail — in practice the previous build — rather than against the previous hour.

A payload without a `files` array never erases existing detail, so an older scheduler degrades gracefully instead of wiping the table.

A snapshot sent by an older scheduler simply carries no file list, and the report then omits the section.

### Health

Health is derived from the age of the most recent heartbeat, relative to the expected 60 second interval:

| Health | Last heartbeat |
|--------|----------------|
| `healthy` | 3 minutes or less |
| `stale` | 3 to 15 minutes |
| `down` | more than 15 minutes |
| `unknown` | never received |

### HTML Report

`GET /report/html` serves the same data as a self-contained page (inline CSS, no external asset) with status pills, coverage progress bars annotated with their delta, an inline SVG trend line per metric, and a per-fuzzer table. Hovering a coverage bar shows the peak ever reached. A global coverage card is added when several schedulers are reported. The weakest 15 files are tabulated under the coverage block, the rest folding into an expandable section; hovering a truncated path shows it in full. The "Coverage" column of the fuzzer table shows the line coverage of each binary, its change since the previous point, and a trend line. If you hover the cell, the page shows the other metrics and the date of the first point of the trend. The page refreshes itself every 30 seconds.

```bash
# JSON, scriptable
curl -s http://127.0.0.1:3000/report | jq '.totals'

# HTML, open in a browser
xdg-open http://127.0.0.1:3000/report/html
```

The `fuzzer_status` table needs no cleanup: it holds at most one row per live fuzzer and is pruned by each heartbeat.

The `fuzzer_runtime` table keeps one row for each fuzzer binary that a scheduler reported, also after the binary is gone. A heartbeat does not delete these rows, and the maintenance tool does not delete them either. Each row is small, and the report shows the total of each binary in the "Temps total" column of the fuzzer table and in the `total_runtime_seconds` field of the JSON report.


## Maintenance / Cleanup

The `fuzzwizard-cleanup` CLI tool provides interactive, safe cleanup of old backend data without stopping the TUI. It communicates through the backend's maintenance API.

### Default Retention

| Category | Default |
|----------|---------|
| Crashes | 6 months |
| Admin events (processed) | 30 days |
| Coverage snapshots | Kept indefinitely (override with `--coverage-before`) |
| Coverage per-file detail | 30 days (`--coverage-files-days`, `0` disables the purge) |

Dropping the per-file detail leaves the snapshots and their aggregated metrics untouched, so long-term trends survive the purge while the bulky part is reclaimed.

The coverage points of each fuzzer belong to their snapshot. When the tool deletes a snapshot (`--coverage-before`), the database also deletes its points. The per-file purge does not change the points.

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

# Keep the per-file coverage detail for a full year instead of 30 days
fuzzwizard-cleanup --coverage-files-days 365

# Never purge the per-file detail
fuzzwizard-cleanup --coverage-files-days 0

# Specify a non-default backend URL
fuzzwizard-cleanup --backend-url http://my-host:3000
```

The tool always shows a preview of what will be deleted and requires explicit confirmation before proceeding. Deletions are performed in small batches (500 records at a time) to avoid blocking concurrent TUI operations.


## See Also

- [install.md](install.md): Installation instructions
- [config.md](config.md): Configuration documentation
- [notificator.md](notificator.md): Notificator documentation
