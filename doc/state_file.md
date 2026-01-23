<!-- SPDX-FileCopyrightText: © 2026 LEDGER SAS -->
<!-- SPDX-License-Identifier: MIT -->

# State File Documentation

The scheduler uses a JSON state file to log the current state of all fuzzing tasks. The state file is automatically updated by the scheduler when task status changes.

## Overview

The state file provides a way to:
- Monitor fuzzer states (RUNNING, CRASHED, STOPPED)
- Access container names for debugging
- Configure environment variables (`var_env`) and command options (`cmd_options`) for each fuzzer
- View environment variables and command options used for each fuzzer

**Note**: 
- Task control (START/STOP/RESTART) is handled directly through the TUI interface. See [fuzztui.md](fuzztui.md) for details.
- The `state` field is read-only and automatically updated by the scheduler.
- You can manually edit `var_env` and `cmd_options` fields in the state file - these will be used when starting/restarting tasks via the TUI.

## Configuration

The `state_file` field is **optional** in your scheduler configuration, if `state_file` is not defined, the scheduler will automatically use `/tmp/{scheduler_name}_state.json` as the default path.

**On startup**: The scheduler always creates an empty state file, overwriting any existing file. This ensures a clean state on each launch.

## State File Format

The state file is a JSON with the following structure:

```json
{
  "fuzzers": [
    {
      "path": "/path/to/fuzzer_binary",
      "sha": "a1b2c3d4e5f6...",
      "state": "RUNNING",
      "container_name": "docker-fw-abc123-fuzzer_name",
      "var_env": {
        "ASAN_OPTIONS": "detect_leaks=1",
        "MSAN_OPTIONS": "exit_code=86"
      },
      "cmd_options": [
        "-max_total_time=3600",
        "-jobs=1"
      ]
    },
    {
      "path": "/path/to/another_fuzzer",
      "sha": "f6e5d4c3b2a1...",
      "state": "CRASHED",
      "container_name": null,
      "var_env": null,
      "cmd_options": null
    }
  ]
}
```

### Fields

- **`path`**: Path to the fuzzer binary
- **`sha`**: SHA3-256 hash of the binary (hexadecimal)
- **`state`**: Current state of the fuzzer (see "States" section)
- **`container_name`**: Docker/Podman container name if fuzzer runs in a container (optional, `null` if not applicable)
- **`var_env`**: Environment variables to set when launching the fuzzer (optional, `null` if not defined)
- **`cmd_options`**: Additional command-line options to pass to the fuzzer (optional, `null` if not defined)

### Notes

- The `container_name` field is automatically filled by the scheduler for fuzzers running in containers. This allows easy attachment with `docker attach <container_name>` or `docker exec -it <container_name> bash`.
- The `var_env` and `cmd_options` fields are initialized with default values on first launch, but can be modified in the state file to control fuzzer behavior.
- Values of `var_env` and `cmd_options` are preserved during automatic state file updates.

## States

All states are automatically updated by the scheduler when task status changes. The `state` field is read-only from a user perspective (do not edit it manually).

- **`RUNNING`**: The fuzzer is currently running
- **`CRASHED`**: The fuzzer has crashed
- **`STOPPED`**: The fuzzer has been stopped (either manually via TUI or automatically when target is deleted)

## How It Works

1. **Creation**: The state file is created at scheduler initialization with an empty state
2. **Update on Changes**: The state file is updated only when task status changes:
   - When a task is added (new binary detected)
   - When a task is removed (binary deleted)
   - When a task status changes (RUNNING → CRASHED, RUNNING → STOPPED, etc.)
   - When a task is started/stopped/restarted via the TUI
3. **Logging**: The state file serves as a persistent log of all fuzzing tasks and their current status
4. **Preservation**: Environment variables (`var_env`) and command options (`cmd_options`) are preserved across updates
5. **Configuration**: When starting or restarting a task via the TUI, the scheduler reads `var_env` and `cmd_options` from the state file if they exist

## Configuring Task Options

You can edit the state file to add or modify environment variables and command options for fuzzers. These options will be used the next time the task is started or restarted via the TUI.

### Adding Environment Variables

Edit the state file and add a `var_env` object to the fuzzer entry:

```json
{
  "fuzzers": [
    {
      "path": "/path/to/fuzzer",
      "sha": "a1b2c3d4e5f6...",
      "state": "STOPPED",
      "var_env": {
        "ASAN_OPTIONS": "detect_leaks=1",
        "UBSAN_OPTIONS": "halt_on_error=1"
      },
      "cmd_options": null
    }
  ]
}
```

### Adding Command Options

Edit the state file and add a `cmd_options` array to the fuzzer entry:

```json
{
  "fuzzers": [
    {
      "path": "/path/to/fuzzer",
      "sha": "a1b2c3d4e5f6...",
      "state": "STOPPED",
      "var_env": null,
      "cmd_options": [
        "-max_total_time=7200",
        "-jobs=2",
        "-dict=my_dict.txt"
      ]
    }
  ]
}
```

### Combined Configuration

You can combine both environment variables and command options:

```json
{
  "fuzzers": [
    {
      "path": "/path/to/fuzzer",
      "sha": "a1b2c3d4e5f6...",
      "state": "STOPPED",
      "var_env": {
        "ASAN_OPTIONS": "detect_leaks=1"
      },
      "cmd_options": [
        "-max_total_time=3600",
        "-jobs=1"
      ]
    }
  ]
}
```

**Important**: After editing the state file, the options will be used the next time you start or restart the task via the TUI. The scheduler will preserve these options in future updates.

## Usage Examples

### Viewing Current State

The state file can be read to see the current status of all fuzzing tasks:

```bash
cat /tmp/my_scheduler_state.json | jq
```

This will show all running, crashed, and stopped fuzzers with their current configuration.

### Monitoring Container Names

The `container_name` field allows you to easily attach to running fuzzer containers:

```bash
# Get container name from state file
CONTAINER=$(cat /tmp/my_scheduler_state.json | jq -r '.fuzzers[0].container_name')

# Attach to container
docker attach $CONTAINER
# or
docker exec -it $CONTAINER bash
```

### Task Management

To start, stop, or restart tasks, use the TUI interface. See [fuzztui.md](fuzztui.md) for details on using the task management menu.




