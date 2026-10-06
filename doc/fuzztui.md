<!-- SPDX-FileCopyrightText: © 2026 LEDGER SAS -->
<!-- SPDX-License-Identifier: MIT -->

# FuzzTUI Documentation

## Overview

FuzzTUI is a terminal user interface (TUI) for monitoring and managing the Fuzzwizard system. It provides an interactive interface to:
- Monitor process status (backend, notificator, schedulers)
- View recent crashes
- Navigate between schedulers
- View running fuzzing tasks
- Display coverage reports per task (when coverage is enabled)

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
  - **Coverage view**: Per-task coverage report (summary table and line-by-line file view) when coverage is enabled for the scheduler

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
- **D**: Display coverage for the selected task (only when scheduler is running and task has coverage)
- **P**: Display project-wide coverage (merges all tasks in this scheduler)
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

### Task Management Menu – Coverage

When a task is selected and the scheduler is running, the menu includes:

- **DISPLAY COVERAGE**: Open the coverage view for the selected task (requires coverage enabled and LLVM tools: `llvm-profdata`, `llvm-cov`).
- **DISPLAY PROJECT COVERAGE**: Open the project-wide coverage view, merging `.profraw` files from **all** tasks in this scheduler into a single `project.profdata` and running `llvm-cov` with every instrumented binary. All fuzzers must share a compatible LLVM instrumentation version.

### Coverage View

When coverage is displayed (key **D** / **P** or menu **DISPLAY COVERAGE** / **DISPLAY PROJECT COVERAGE**), a full-screen overlay shows:

1. **Summary table**: Header row plus one row per source file with coverage (Regions, Functions, Lines, Branches). The first column is the file path.
2. **Navigation**:
   - **Up/Down**: Move selection in the summary table
   - **Enter**: Open line-by-line coverage for the selected file
   - **Esc** or **d**: Close coverage view and return to the scheduler tab

3. **File view**: Line-by-line coverage for one source file. **Up/Down** scroll the content; **B** or **Esc** returns to the summary table.

Coverage data is produced by the scheduler using `llvm-profdata` (merge of `.profraw` files) and `llvm-cov` (report/show). The scheduler must have `has_coverage: true` and a valid `coverage_directory` in its config.

When sources are not available on disk, the coverage view falls back to region-level data from `llvm-cov export` (functions, regions, uncovered line ranges). The overlay title shows "source unavailable" and the border turns yellow.

#### Project coverage

Project coverage (`P` / **DISPLAY PROJECT COVERAGE**) merges all `.profraw` files across tasks into `{coverage_directory}/project.profdata` and passes every binary as `-object` to `llvm-cov`. The merge and analysis run in a background thread so the TUI remains responsive. If `llvm-profdata merge` fails (e.g. incompatible LLVM versions across fuzzers), the error is displayed in the overlay.

## See Also

- [config.md](config.md): Configuration details
- [backend.md](backend.md): Backend API
- [scheduler.md](scheduler.md): Scheduler documentation
- [notificator.md](notificator.md): Notificator documentation
