<!-- SPDX-FileCopyrightText: © 2026 LEDGER SAS -->
<!-- SPDX-License-Identifier: MIT -->

# Backend Documentation

## Overview

The backend is a REST API that manages crash storage and retrieval in a PostgreSQL database. It provides endpoints for:
- Storing crashes reported by schedulers
- Retrieving crashes (all, last N, by ID)
- Managing administrative events


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


## See Also

- [install.md](install.md): Installation instructions
- [config.md](config.md): Configuration documentation
- [notificator.md](notificator.md): Notificator documentation
