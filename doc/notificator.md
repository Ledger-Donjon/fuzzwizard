<!-- SPDX-FileCopyrightText: © 2026 LEDGER SAS -->
<!-- SPDX-License-Identifier: MIT -->

# Notificator Documentation

## Overview

The notificator is a service that monitors the backend and sends notifications when new crashes are detected or administrative events occur. It:
- Polls the backend regularly for new crashes
- Sends notifications via configured providers (Slack, file)
- Maintains persistent state of last processed crash
- Handles administrative events

## Architecture

```
┌─────────────┐
│  Main Loop  │ ──► Polls every 10 seconds
└──────┬──────┘
       │
       ├──► ┌─────────────┐
       │    │ Backend API │ ──► GET /crashes/ids, GET /crashes/{id}
       │    └─────────────┘     GET /admin_events, POST /admin_events
       │
       ├──► ┌─────────────┐
       │    │State Manager│ ──► Tracks last processed crash date
       │    └─────────────┘
       │
       └──► ┌───────────────────────┐
            │ Notification Providers│
            └──────┬────────────────┘
                   │
                   ├──► ┌───────┐
                   │    │ Slack │ ──► Sends to Slack channel
                   │    └───────┘
                   │
                   └──► ┌───────┐
                        │ File  │ ──► Writes to log file
                        └───────┘
```

## Main Loop

1. **Fetch crash IDs**: Get crash IDs since last processed date
2. **Process crashes**: For each crash:
   - Fetch full crash data
   - Send notifications via all enabled providers
   - Update last processed date
3. **Fetch admin events**: Get unprocessed admin events
4. **Process events**: For each event:
   - Send notifications
   - Mark event as processed
5. **Sleep**: Wait 10 seconds before next iteration

## Notification Providers

### Slack Provider

Sends notifications to a Slack channel.

**Configuration**:
```json
{
  "slack": {
    "enabled": true,
    "token": "xoxb-your-token-here",
    "channel": "#alerts"
  }
}
```

**Message Format**:
```
New crash for fuzzer_name (id: 123)

To retrieve artifacts locally:
$ curl -X POST http://127.0.0.1:3000/collect_crashes -H 'Content-Type: application/json' -d '123'
```

### File Provider

Writes notifications to a file.

**Configuration**:
```json
{
  "file": {
    "enabled": true,
    "output_file": "./logs/crashes.log"
  }
}
```

## Configuration

```json
{
  "notifications": {
    "notificator_state_file": "/tmp/notificator_state.txt",
    "providers": {
      "slack": {
        "enabled": true,
        "token": "xoxb-...",
        "channel": "#alerts"
      },
      "file": {
        "enabled": true,
        "output_file": "./logs/crashes.log"
      }
    }
  }
}
```
See [config.md](config.md) for configuration details.

## State Management

The notificator uses a state file to track the last processed crash:
- **Location**: Specified in `notificator_state_file`
- **Format**: ISO datetime string: `YYYY-MM-DD HH:MM:SS.ffffff`
- **Initialization**: If missing, fetches last crash from database
- **Locking**: Uses exclusive file locking to prevent race conditions


## Usage

### Standalone

```bash
cargo run -p notificator -- config.json
```

Or using the release binary:

```bash
./target/release/notificator config.json
```

### Via FuzzTUI

The notificator is typically launched automatically by FuzzTUI. You can configure FuzzTUI to use a release binary by setting `notificator_binary` in your `config.json` (see [config.md](config.md) for details). If not set, FuzzTUI will use `cargo run -p notificator` to launch it.

## Limitations

- **Fixed polling**: 10 seconds (not configurable)
- **No retry**: Failed notifications are not retried
- **No rate limiting**: Slack rate limits not handled

## See Also

- [backend.md](backend.md): Backend API documentation
- [config.md](config.md): Configuration file documentation
