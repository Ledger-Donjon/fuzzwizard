<!-- SPDX-FileCopyrightText: © 2026 LEDGER SAS -->
<!-- SPDX-License-Identifier: MIT -->

# State File Documentation

The scheduler can use a shared JSON state file to allow manual control of fuzzers without modifying the target directories.

## Overview

The state file provides a way to:
- Manually start, stop, or restart fuzzers
- Control fuzzer behavior with custom environment variables and command options
- Monitor fuzzer states (RUNNING, CRASHED, STOPPED)
- Access container names for debugging

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

### User Commands (modifiable by user)

- **`START`**: Start a fuzzer that is `STOPPED` or `CRASHED`. If the fuzzer is already `RUNNING`, this command is ignored.
- **`STOP`**: Stop the fuzzer if it is running
- **`RESTART`**: Restart a fuzzer that is already `RUNNING`. If the fuzzer is `STOPPED` or `CRASHED`, you must first change its state to `START` in the file.

### Automatic States (updated by scheduler)

- **`RUNNING`**: The fuzzer is running
- **`CRASHED`**: The fuzzer has crashed
- **`STOPPED`**: The fuzzer has been stopped (automatic state after a `STOP` or target deletion)

## How It Works

1. **Read**: On each `update()` call, the scheduler reads the state file
2. **Apply**: User commands (`START`, `STOP`, `RESTART`) are applied
3. **Update**: The scheduler updates the file with actual states (`RUNNING`, `CRASHED`, `STOPPED`)

## Usage Examples

### Restart a crashed fuzzer

1. Edit the state file and change the state to `START`:
```json
{
  "fuzzers": [
    {
      "path": "/path/to/fuzzer",
      "sha": "a1b2c3d4...",
      "state": "START"
    }
  ]
}
```

2. The scheduler will detect the `START` command on the next `update()` (every 60 seconds by default) and restart the fuzzer

**Note**: To restart a fuzzer that is already `RUNNING`, use `RESTART` instead of `START`.

### Stop a specific fuzzer

```json
{
  "fuzzers": [
    {
      "path": "/path/to/fuzzer",
      "sha": "a1b2c3d4...",
      "state": "STOP"
    }
  ]
}
```

**Note**: After stopping, the state will be automatically set to `STOPPED` by the scheduler.

### Start a new fuzzer with custom options

```json
{
  "fuzzers": [
    {
      "path": "/path/to/new_fuzzer",
      "sha": "f6e5d4c3...",
      "state": "START",
      "var_env": {
        "ASAN_OPTIONS": "detect_leaks=1",
        "UBSAN_OPTIONS": "halt_on_error=1"
      },
      "cmd_options": [
        "-max_total_time=7200",
        "-jobs=2",
        "-dict=my_dict.txt"
      ]
    }
  ]
}
```



## Important Notes

- The file is updated after each `update()` and after each `report_crashes()`
- User modifications are applied on the next `update()` cycle (60 seconds by default)
- The SHA must be in hexadecimal (64 characters)
- The path must exactly match the binary path in the targets
- The `container_name` field is automatically filled by the scheduler and allows easy container attachment:
  ```bash
  docker attach <container_name>
  # or
  docker exec -it <container_name> bash
  ```
- The `var_env` and `cmd_options` fields are preserved during automatic state file updates. You can modify them in the file to control fuzzer behavior on the next startup.
- On first launch of a fuzzer, `var_env` and `cmd_options` are `null` and default scheduler values are used. After the first launch, these values are preserved in the state file.
