<!-- SPDX-FileCopyrightText: © 2026 LEDGER SAS -->
<!-- SPDX-License-Identifier: MIT -->

# FuzzTUI Documentation

## Overview

FuzzTUI is a terminal user interface (TUI) for monitoring and managing the Fuzzwizard system. It provides an interactive interface to:
- Monitor process status (backend, notificator, schedulers)
- View recent crashes
- Navigate between schedulers
- View running fuzzing tasks

## Architecture

```
┌──────────────┐
│ Main Thread  │ ──► TUI Rendering
└──────┬───────┘
       │
       ├──► ┌──────────────┐
       │    │ Crash Thread │ ──► Fetches crashes (60s)
       │    └──────┬───────┘
       │           │
       │           ▼
       │    ┌──────────────┐
       │    │ Backend API  │
       │    └──────┬───────┘
       │           │
       │           ▼
       │    ┌──────────────┐
       │    │  PostgreSQL  │
       │    └──────────────┘
       │
       ├──► ┌───────────────┐
       │    │Launcher Thread│ ──► Updates status (60s)
       │    └──────┬────────┘
       │           │
       │           ▼
       │    ┌──────────────┐
       │    │   Launcher   │ ──► Process Manager
       │    └──────┬───────┘
       │           │
       │           ├──► Backend
       │           ├──► Notificator ──► Slack API
       │           ├──► Scheduler 1
       │           └──► Scheduler N
       │
       └──► reads Launcher state
```

## Components

### Launcher

The launcher manages the lifecycle of all processes:
- Starts and monitors backend, notificator, and schedulers
- Updates configuration from JSON file
- Sends Slack alerts on errors

### TUI Interface

The user interface displays:
- **Processes tab**: Status of all managed processes
- **Scheduler tabs**: One tab per scheduler showing:
  - Scheduler status and uptime
  - Running fuzzing tasks with binary hash and status
- **Recent crashes**: Last 10 crashes with ID, filename, scheduler, and date

### Threads

- **Main thread**: TUI rendering loop (non-blocking)
- **Crash thread**: Fetches crashes from backend every 60 seconds
- **Launcher thread**: Updates launcher status every 60 seconds

### Communication

- **mpsc channels**: Asynchronous communication between threads
- **Arc<RwLock<Launcher>>**: Thread-safe shared state
- **Cache**: Non-blocking display cache to avoid UI freezing

## Configuration

See [config.md](config.md) for configuration details.

## Usage

```bash
cargo run -p fuzztui -- config.json
```

## Keyboard Controls

- **Q**: Quit
- **Left/Right**: Navigate tabs
- **Up/Down**: Scroll content

## See Also

- [config.md](config.md): Configuration details
- [backend.md](backend.md): Backend API
- [scheduler.md](scheduler.md): Scheduler documentation
- [notificator.md](notificator.md): Notificator documentation
