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
  - Launcher processes (backend, notificator)
  - Scheduler processes with status and uptime
  - Recent crashes (last 10)
- **Scheduler tabs**: One tab per scheduler showing:
  - Scheduler information (name, status, info, state file path)
  - Task table with columns: SHA, FileName, Status, StatusTime
  - Task management menu (when scheduler is running)

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

### Global Controls
- **Q**: Quit the application
- **Left/Right**: Navigate between tabs
- **C**: Reload configuration (with confirmation prompt)

### Processes Tab
- **Up/Down**: Scroll through the content

### Scheduler Tabs
- **Up/Down**: Navigate through tasks in the table
- **M**: Open task management menu (only when scheduler is running)
  - **Up/Down**: Navigate menu options
  - **Enter**: Execute selected action
  - **Esc** or **M**: Close menu

### Task Management Menu

When a task is selected and the scheduler is running, pressing **M** opens a context menu with available actions:

- **If task is RUNNING**:
  - **STOP**: Stop the running task
  - **RESTART**: Restart the task

- **If task is STOPPED or CRASHED**:
  - **START**: Start the task

**Note**: If the scheduler is stopped, the interface is dimmed and task management is disabled. Navigation (Up/Down) still works to view tasks, but no interactions are possible.

## See Also

- [config.md](config.md): Configuration details
- [backend.md](backend.md): Backend API
- [scheduler.md](scheduler.md): Scheduler documentation
- [notificator.md](notificator.md): Notificator documentation
