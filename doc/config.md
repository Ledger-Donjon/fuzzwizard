<!-- SPDX-FileCopyrightText: © 2026 LEDGER SAS -->
<!-- SPDX-License-Identifier: MIT -->

# Configuration Guide

This document explains the configuration files used by Fuzzwizard: `config.json` (main configuration) and `scheduler_config.json` (scheduler-specific configuration).

## Main Configuration: config.json

The main configuration file is used by FuzzTUI to launch and manage all components.

### Structure

```json
{
  "schedulers": [...],
  "database_url": "...",
  "logdir": "...",
  "container_binary": "...",
  "backend_binary": "...",
  "notificator_binary": "...",
  "notifications": {...}
}
```

### Fields

#### `schedulers` (array, required)

List of schedulers to manage. Each scheduler is an object with:

- **`name`** (string, required): Unique name for the scheduler (displayed in the UI)
- **`config`** (string, required): Absolute path to the scheduler's configuration file

**Example**:
```json
{
  "schedulers": [
    {
      "name": "project-a-scheduler",
      "config": "/etc/fuzzwizard/project-a.json"
    },
    {
      "name": "project-b-scheduler",
      "config": "/etc/fuzzwizard/project-b.json"
    }
  ]
}
```


#### `database_url` (string, required)

PostgreSQL connection string for the backend.

- **Format**: `postgres://user:password@host:port/database`
- **Example**: `postgres://postgres:postgres@127.0.0.1:5432/fuzzwizard`
- **Note**: This URL is passed to the backend via the `--database-url` argument

#### `logdir` (string, required)

Directory where logs are written.

- Can be a relative or absolute path
- Directory is created automatically if it doesn't exist
- **Example**: `"./logs"` or `"/var/log/fuzzwizard"`
- Logs are written to:
  - `{logdir}/fuzztui.log`: Main launcher logs
  - `{logdir}/notificator.log`: Notification service logs
  - `{logdir}/running_tasks.log`: Scheduler task logs

#### `container_binary` (string, optional)

Path to the Docker/Podman binary for containerized fuzzer execution.

- **Example**: `"/usr/bin/docker"` or `"/usr/bin/podman"`
- If `null`, schedulers cannot use containers
- Must be an absolute path
- Binary must be executable and accessible

#### `backend_binary` (string, optional)

Path to the backend release binary.

- **Example**: `"/path/to/fuzzwizard/target/release/backend"`
- If not set, FuzzTUI will use `cargo run -p backend` to launch the backend
- If set, FuzzTUI will launch the binary directly (recommended for production)
- Must be an absolute path
- Binary must be executable and accessible
- To build the binary: `cd /path/to/fuzzwizard && cargo build --release`

**Note**: Using release binaries is recommended for better performance and stability.

#### `notificator_binary` (string, optional)

Path to the notificator release binary.

- **Example**: `"/path/to/fuzzwizard/target/release/notificator"`
- If not set, FuzzTUI will use `cargo run -p notificator` to launch the notificator
- If set, FuzzTUI will launch the binary directly (recommended for production)
- Must be an absolute path
- Binary must be executable and accessible
- To build the binary: `cd /path/to/fuzzwizard && cargo build --release`

**Note**: Using release binaries is recommended for better performance and stability. The binaries will be in `target/release/backend` and `target/release/notificator` after building.

#### `notifications` (object, required)

Notification configuration.

**Structure**:
```json
{
  "notificator_state_file": "/tmp/notificator_state.txt",
  "providers": {
    "slack": {...},
    "file": {...}
  }
}
```

**Fields**:
- **`notificator_state_file`** (string, required): Path to the state file for tracking notified crashes
- **`providers`** (object, required): Notification provider configurations

**Slack Provider**:
```json
{
  "slack": {
    "enabled": true,
    "token": "xoxb-your-token-here",
    "channel": "#alerts"
  }
}
```

- **`enabled`** (boolean): Enable/disable Slack notifications
- **`token`** (string): Slack bot token (required if enabled)
- **`channel`** (string): Slack channel ID or name (required if enabled)

**File Provider**:
```json
{
  "file": {
    "enabled": true,
    "output_file": "./logs/crashes.log"
  }
}
```

- **`enabled`** (boolean): Enable/disable file notifications
- **`output_file`** (string): Path to the output file (required if enabled)

### Complete Example

```json
{
  "schedulers": [
    {
      "name": "main-scheduler",
      "config": "/etc/fuzzwizard/main.json"
    },
    {
      "name": "test-scheduler",
      "config": "/etc/fuzzwizard/test.json"
    }
  ],
  "database_url": "postgres://fuzzwizard:password@localhost:5432/fuzzwizard",
  "logdir": "/var/log/fuzzwizard",
  "container_binary": "/usr/bin/podman",
  "backend_binary": "/opt/fuzzwizard/target/release/backend",
  "notificator_binary": "/opt/fuzzwizard/target/release/notificator",
  "notifications": {
    "notificator_state_file": "/tmp/notificator_state.txt",
    "providers": {
      "slack": {
        "enabled": true,
        "token": "xoxb-1234567890-1234567890123-abcdefghijklmnopqrstuvwx",
        "channel": "#alerts"
      },
      "file": {
        "enabled": true,
        "output_file": "/tmp/fuzzwizard/crashes.log"
      }
    }
  }
}
```

## Scheduler Configuration: scheduler_config.json

Each scheduler has its own configuration file that defines what to fuzz and how.

### Structure

```json
{
  "scheduler_name": "...",
  "targets_directory": [...],
  "build_script": "...",
  "build_script_interval_hours": null,
  "backend_url": "...",
  "corpus_directory": "...",
  "crashes_directory": "...",
  "coverage_directory": "...",
  "has_coverage": true,
  "llvm_bin_directory": null,
  "container_image": null,
  "container_options": null,
  "state_file": null
}
```

### Fields

#### `scheduler_name` (string, required)

Unique name for the scheduler.

- Used to identify the scheduler in logs
- Used to name the default state file: `/tmp/{scheduler_name}_state.json`
- Used to identify crashes in the backend

**Example**: `"my_scheduler"`, `"project-a-scheduler"`

#### `targets_directory` (array of strings, required)

List of directories where binaries to fuzz are located.

- Each directory is scanned recursively
- Binaries are identified by their SHA-256 hash
- New binaries are automatically launched
- Removed binaries are automatically stopped

**Example**:
```json
{
  "targets_directory": [
    "/opt/fuzzwizard/targets/project-a",
    "/opt/fuzzwizard/targets/project-b"
  ]
}
```

**Note**: Paths must be absolute.

#### `build_script` (string, required)

Path to the script that builds the fuzzer targets.

- Script is executed in the background when the scheduler is created
- Script must compile binaries into the `targets_directory` directories
- Script is executed with `sh -c`

**Example**: `"/opt/fuzzwizard/scripts/build.sh"`

**Note**: Path must be absolute and the script must be executable.

#### `build_script_interval_hours` (number, optional)

Minimum time between two runs of `build_script`, in whole hours.

- If omitted or `null`, the build script runs on **every** scheduler refresh (FuzzTUI periodic update) or on **every** loop iteration (standalone `scheduler` binary, every 60 seconds).
- If set to a **positive** integer (e.g. `6`), `build_script` runs at most once per that many hours; intermediate updates still refresh targets and tasks, but skip rebuilding until the interval has elapsed.
- `0` is treated like omitted: no hourly throttling.

**Example**: `"build_script_interval_hours": 24` for at most one build per day.

#### `backend_url` (string, required)

URL of the backend for sending crashes.

- **Format**: `http://host:port` or `https://host:port`
- Scheduler sends crashes via `POST {backend_url}/crashes`
- Must be accessible from the scheduler

**Example**: `"http://127.0.0.1:3000"`, `"https://backend.example.com"`

#### `corpus_directory` (string, required)

Base directory for fuzzer corpus.

- Each fuzzer has its own subdirectory: `{corpus_directory}/{fuzzer_name}/`
- Corpus is reused if the binary is restarted
- A random initialization file is created if the corpus is empty

**Example**: `"/opt/fuzzwizard/corpus"`

**Note**: Directory is created automatically if it doesn't exist.

#### `crashes_directory` (string, required)

Base directory for fuzzer crashes.

- Each fuzzer has its own subdirectory: `{crashes_directory}/{fuzzer_name}/`
- Crash files are read and sent to the backend

**Example**: `"/opt/fuzzwizard/crashes"`

**Note**: Directory is created automatically if it doesn't exist.

#### `coverage_directory` (string, required)

Base directory for coverage files.

- Each fuzzer has its own subdirectory: `{coverage_directory}/{fuzzer_name}/`
- Used only if `has_coverage: true`
- `.profraw` files are stored here

**Example**: `"/opt/fuzzwizard/coverage"`

**Note**: Directory is created automatically if it doesn't exist.

#### `has_coverage` (boolean, required)

Enable or disable coverage collection.

- `true`: Coverage files are generated and stored
- `false`: No coverage collection

**Note**: Requires binaries to be compiled with coverage instrumentation (LLVM).

#### `llvm_bin_directory` (string, optional)

Directory that contains the LLVM tools `llvm-cov` and `llvm-profdata`, used when the scheduler analyzes coverage (FuzzTUI / API).

- If omitted or `null`, the scheduler tries the environment variable `FUZZWIZARD_LLVM_BIN` (same semantics: directory path), then falls back to resolving those tool names from `PATH`.
- Set this when LLVM is not on the default `PATH` (for example Homebrew: `/opt/homebrew/opt/llvm/bin`).

**Example**: `"/opt/homebrew/opt/llvm/bin"`

#### `container_image` (string, optional)

Docker/Podman image to use for executing fuzzers.

- If set, fuzzers are executed in containers
- Image must contain necessary tools (libFuzzer, etc.)
- Volumes are automatically mounted:
  - `/fuzzer/binary`: Fuzzer binary
  - `/fuzzer/crash_dir/`: Crash directory
  - `/fuzzer/corpus_dir/`: Corpus directory
  - `/fuzzer/coverage/`: Coverage directory (if enabled)

**Example**: `"fuzzing-image:latest"`, `"myregistry/fuzzer:v1.0"`

**Note**: Requires `container_binary` to be set in the launcher configuration.

#### `container_options` (array of strings, optional)

Additional options to pass to the container.

- Added after the mounted volumes
- Useful for options like `--privileged`, `--network`, etc.

**Example**:
```json
{
  "container_options": [
    "--privileged",
    "--network=host",
    "--cap-add=SYS_PTRACE"
  ]
}
```

#### `state_file` (string, optional)

Path to the shared state file.

- If not set, uses `/tmp/{scheduler_name}_state.json` by default
- Allows manual control of fuzzers (START/STOP/RESTART)
- See [state_file.md](state_file.md) for more details

**Example**: `"/var/lib/fuzzwizard/scheduler_state.json"`

**Note**: File is created automatically if it doesn't exist.

### Complete Example

```json
{
  "scheduler_name": "my_scheduler",
  "targets_directory": [
    "/opt/fuzzwizard/targets/project-a",
    "/opt/fuzzwizard/targets/project-b"
  ],
  "build_script": "/opt/fuzzwizard/scripts/build.sh",
  "build_script_interval_hours": null,
  "backend_url": "http://127.0.0.1:3000",
  "corpus_directory": "/opt/fuzzwizard/corpus",
  "crashes_directory": "/opt/fuzzwizard/crashes",
  "coverage_directory": "/opt/fuzzwizard/coverage",
  "has_coverage": true,
  "llvm_bin_directory": null,
  "container_image": "fuzzing-image:latest",
  "container_options": [
    "--privileged"
  ],
  "state_file": "/var/lib/fuzzwizard/scheduler_state.json"
}
```


## See Also

- [scheduler.md](scheduler.md): Detailed scheduler documentation
- [state_file.md](state_file.md): State file format and usage
- [install.md](install.md): Installation instructions
