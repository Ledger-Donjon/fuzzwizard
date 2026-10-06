// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

//! Security validation helpers for every untrusted input the scheduler reads:
//! the state file, the fuzzer command line and environment, container options
//! and the backend URL.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::{Result, SchedulerError};

/// Maximum length for environment variable keys and values
const MAX_ENV_VAR_LENGTH: usize = 4096;
/// Maximum length for command options
const MAX_CMD_OPTION_LENGTH: usize = 8192;
/// Maximum size of state file (10 MB)
const MAX_STATE_FILE_SIZE: usize = 10 * 1024 * 1024;
/// Maximum length of a container name
const MAX_CONTAINER_NAME_LENGTH: usize = 256;
/// Maximum length of a build script path
const MAX_BUILD_SCRIPT_LENGTH: usize = 65536;

/// Dangerous characters that could be used for shell injection
/// Used for validating environment variables and command options
const DANGEROUS_CHARS: &[char] = &[
    '\0', // null byte
    '\n', '\r', // newlines
    ';',  // command separator
    '&',  // background/AND
    '|',  // pipe
    '`',  // command substitution
    '$',  // variable expansion
    '(', ')', // subshell
    '<', '>', // redirection
];

/// Subset of [`DANGEROUS_CHARS`] rejected in command options: `<`, `>`, `(` and
/// `)` are legitimate in some fuzzer flags and are never passed to a shell.
const CMD_OPTION_DANGEROUS_CHARS: &[char] = &[
    '\0', // null byte
    '\n', '\r', // newlines
    ';',  // command separator
    '&',  // background/AND
    '|',  // pipe
    '`',  // command substitution
    '$',  // variable expansion (can be used in paths but risky)
];

/// Container flags accepted from the configuration, with a flag telling whether
/// the option consumes the argument that follows it as its value.
///
/// This is deliberately an allowlist. A denylist cannot work here: the options
/// are appended *after* the hardening flags the scheduler builds, so the
/// container runtime lets the last occurrence win, and any flag that was not
/// thought of (`--pid host`, `--read-only=false`, `--entrypoint`, ...) silently
/// undoes the sandbox. Only resource limits and cosmetic flags are allowed;
/// anything touching namespaces, capabilities, devices, mounts or the
/// entrypoint is rejected by construction.
const ALLOWED_CONTAINER_OPTIONS: &[(&str, bool)] = &[
    ("--cpu-period", true),
    ("--cpu-quota", true),
    ("--cpu-shares", true),
    ("--cpus", true),
    ("--cpuset-cpus", true),
    ("--cpuset-mems", true),
    ("-e", true),
    ("--env", true),
    ("--hostname", true),
    ("--init", false),
    ("--label", true),
    ("--memory", true),
    ("--memory-reservation", true),
    ("--memory-swap", true),
    ("--pids-limit", true),
    ("--platform", true),
    ("--shm-size", true),
    ("--ulimit", true),
    ("-w", true),
    ("--workdir", true),
];

/// Shell constructs that reveal an inline command rather than a script path.
const DANGEROUS_BUILD_SCRIPT_PATTERNS: &[&str] = &["&&", "||", ";", "`", "$(", ">>", "<<"];

/// Validates an environment variable key and value
///
/// # Security
///
/// - Checks for null bytes
/// - Checks for newlines (could break shell escaping)
/// - Validates length limits
/// - Checks for dangerous shell metacharacters that could enable command injection
///
/// # Errors
///
/// Returns `SchedulerError::InvalidInput` if the key or value is invalid
pub fn validate_env_var(key: &str, value: &str) -> Result<()> {
    // Check key is not empty
    if key.is_empty() {
        return Err(SchedulerError::InvalidInput(
            "Environment variable key cannot be empty".to_string(),
        ));
    }

    // Check length limits
    if key.len() > MAX_ENV_VAR_LENGTH {
        return Err(SchedulerError::InvalidInput(format!(
            "Environment variable key too long: {} bytes (max: {})",
            key.len(),
            MAX_ENV_VAR_LENGTH
        )));
    }
    if value.len() > MAX_ENV_VAR_LENGTH {
        return Err(SchedulerError::InvalidInput(format!(
            "Environment variable value too long: {} bytes (max: {})",
            value.len(),
            MAX_ENV_VAR_LENGTH
        )));
    }

    // Check key for dangerous characters
    if key.chars().any(|c| DANGEROUS_CHARS.contains(&c)) {
        return Err(SchedulerError::InvalidInput(format!(
            "Environment variable key contains dangerous characters: {key}"
        )));
    }

    // Key should be alphanumeric + underscore + dash
    if !key
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return Err(SchedulerError::InvalidInput(format!(
            "Invalid characters in environment variable key: {key}"
        )));
    }

    // Check value for dangerous characters
    if value.chars().any(|c| DANGEROUS_CHARS.contains(&c)) {
        return Err(SchedulerError::InvalidInput(
            "Environment variable value contains dangerous characters".to_string(),
        ));
    }

    Ok(())
}

/// Validates a command option
///
/// # Security
///
/// - Checks for null bytes
/// - Validates length limits
/// - Checks for shell metacharacters that could enable command injection
///
/// # Errors
///
/// Returns `SchedulerError::InvalidInput` if the option is invalid
pub fn validate_cmd_option(option: &str) -> Result<()> {
    // Check option is not empty
    if option.is_empty() {
        return Err(SchedulerError::InvalidInput(
            "Command option cannot be empty".to_string(),
        ));
    }

    if option.len() > MAX_CMD_OPTION_LENGTH {
        return Err(SchedulerError::InvalidInput(format!(
            "Command option too long: {} bytes (max: {})",
            option.len(),
            MAX_CMD_OPTION_LENGTH
        )));
    }

    if option
        .chars()
        .any(|c| CMD_OPTION_DANGEROUS_CHARS.contains(&c))
    {
        return Err(SchedulerError::InvalidInput(
            "Command option contains dangerous characters".to_string(),
        ));
    }

    Ok(())
}

/// Validates a SHA-256 hex string
///
/// # Security
///
/// - Must be exactly 64 hexadecimal characters
/// - No other characters allowed
pub fn validate_sha256_hex(sha: &str) -> Result<()> {
    if sha.len() != 64 {
        return Err(SchedulerError::InvalidInput(format!(
            "Invalid SHA-256 length: {} (expected 64)",
            sha.len()
        )));
    }

    if !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(SchedulerError::InvalidInput(
            "SHA-256 must contain only hexadecimal characters".to_string(),
        ));
    }

    Ok(())
}

/// Validates the size of state file content before parsing
pub fn validate_state_file_size(content: &str) -> Result<()> {
    if content.len() > MAX_STATE_FILE_SIZE {
        return Err(SchedulerError::InvalidInput(format!(
            "State file too large: {} bytes (max: {})",
            content.len(),
            MAX_STATE_FILE_SIZE
        )));
    }
    Ok(())
}

/// Validates a container name to ensure it doesn't contain dangerous characters
pub fn validate_container_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(SchedulerError::InvalidInput(
            "Container name cannot be empty".to_string(),
        ));
    }

    if name.len() > MAX_CONTAINER_NAME_LENGTH {
        return Err(SchedulerError::InvalidInput(format!(
            "Container name too long: {} bytes (max: {MAX_CONTAINER_NAME_LENGTH})",
            name.len()
        )));
    }

    // Container names should only contain alphanumeric, dash, underscore
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(SchedulerError::InvalidInput(format!(
            "Invalid characters in container name: {name}"
        )));
    }

    Ok(())
}

/// Look up a flag in [`ALLOWED_CONTAINER_OPTIONS`], returning whether it takes
/// a value.
fn allowed_container_flag(flag: &str) -> Option<bool> {
    ALLOWED_CONTAINER_OPTIONS
        .iter()
        .find(|(allowed, _)| *allowed == flag)
        .map(|(_, takes_value)| *takes_value)
}

/// Render the allowlist for error messages.
fn allowed_container_flags() -> String {
    ALLOWED_CONTAINER_OPTIONS
        .iter()
        .map(|(flag, _)| *flag)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Validates the value carried by a container flag.
///
/// A value starting with `-` would either be a flag smuggled past the
/// allowlist (`--memory --privileged`) or a negative limit that removes the
/// bound it is supposed to set (`--pids-limit=-1`).
fn validate_container_value(flag: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(SchedulerError::InvalidInput(format!(
            "Container option '{flag}' has an empty value"
        )));
    }
    if value.len() > MAX_CMD_OPTION_LENGTH {
        return Err(SchedulerError::InvalidInput(format!(
            "Container option '{flag}' value too long: {} bytes (max: {MAX_CMD_OPTION_LENGTH})",
            value.len()
        )));
    }
    if value.starts_with('-') {
        return Err(SchedulerError::InvalidInput(format!(
            "Container option '{flag}' has a suspicious value starting with '-': {value}"
        )));
    }
    if value.chars().any(|c| DANGEROUS_CHARS.contains(&c)) {
        return Err(SchedulerError::InvalidInput(format!(
            "Container option '{flag}' value contains dangerous characters"
        )));
    }
    Ok(())
}

/// Validates the container options as a whole against [`ALLOWED_CONTAINER_OPTIONS`].
///
/// The list is checked as a sequence rather than option by option, because a
/// flag and its value may be two separate entries (`["--memory", "1g"]`) and
/// validating them in isolation is what let `["--pid", "host"]` through.
///
/// # Errors
///
/// Returns `SchedulerError::InvalidInput` if any option is not allowlisted, if
/// a flag that needs a value has none, or if a value looks dangerous.
pub fn validate_container_options(options: &[String]) -> Result<()> {
    let mut iter = options.iter().filter(|o| !o.is_empty());

    while let Some(option) = iter.next() {
        if option.len() > MAX_CMD_OPTION_LENGTH {
            return Err(SchedulerError::InvalidInput(format!(
                "Container option too long: {} bytes (max: {MAX_CMD_OPTION_LENGTH})",
                option.len()
            )));
        }

        // `--flag=value` carries its own value; `--flag` may take the next entry.
        let (flag, inline_value) = match option.split_once('=') {
            Some((flag, value)) => (flag, Some(value)),
            None => (option.as_str(), None),
        };

        let Some(takes_value) = allowed_container_flag(flag) else {
            return Err(SchedulerError::InvalidInput(format!(
                "Container option '{option}' is not allowed. \
                 Allowed options: {}",
                allowed_container_flags()
            )));
        };

        match (takes_value, inline_value) {
            (true, Some(value)) => validate_container_value(flag, value)?,
            (true, None) => {
                let value = iter.next().ok_or_else(|| {
                    SchedulerError::InvalidInput(format!(
                        "Container option '{flag}' requires a value"
                    ))
                })?;
                validate_container_value(flag, value)?;
            }
            (false, None) => {}
            (false, Some(_)) => {
                return Err(SchedulerError::InvalidInput(format!(
                    "Container option '{flag}' does not take a value: {option}"
                )));
            }
        }
    }

    Ok(())
}

/// Validates that a backend URL points to localhost only (SSRF hardening).
///
/// Parses the authority/host explicitly so prefixes like `http://127.0.0.1.evil.test`
/// are not mistaken for loopback.
pub fn validate_backend_url(url: &str) -> Result<()> {
    let url = url.trim();
    let scheme_len = if url.len() >= 7 && url.as_bytes()[..7].eq_ignore_ascii_case(b"http://") {
        7
    } else if url.len() >= 8 && url.as_bytes()[..8].eq_ignore_ascii_case(b"https://") {
        8
    } else {
        return Err(SchedulerError::InvalidInput(
            "backend_url must start with http:// or https://".to_string(),
        ));
    };

    let rest = &url[scheme_len..];
    let authority_end = rest
        .find(|c| ['/', '?', '#'].contains(&c))
        .unwrap_or(rest.len());
    let authority = &rest[..authority_end];

    let host_port = authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority);

    if host_port.starts_with('[') {
        return Err(SchedulerError::InvalidInput(
            "backend_url must use 127.0.0.1 or localhost (IPv6 bracket host is not supported)"
                .to_string(),
        ));
    }

    let host_end = host_port.find(':').unwrap_or(host_port.len());
    let host = &host_port[..host_end];
    if host != "127.0.0.1" && !host.eq_ignore_ascii_case("localhost") {
        return Err(SchedulerError::InvalidInput(format!(
            "backend_url must use 127.0.0.1 or localhost as host, got: {host}"
        )));
    }

    if host_end < host_port.len() {
        let port_part = &host_port[host_end + 1..];
        if port_part.is_empty() || !port_part.chars().all(|c| c.is_ascii_digit()) {
            return Err(SchedulerError::InvalidInput(
                "backend_url: invalid port".to_string(),
            ));
        }
    }

    Ok(())
}

/// Validates a build script path: must be an absolute path to an existing executable file.
/// Rejects arbitrary shell commands to prevent code injection.
pub fn validate_build_script(script: &str) -> Result<()> {
    if script.trim().is_empty() {
        return Ok(());
    }

    let s = script.trim();
    if s.chars().any(|c| c.is_control()) {
        return Err(SchedulerError::InvalidInput(
            "Build script must not contain control characters".to_string(),
        ));
    }

    if script.contains('\0') {
        return Err(SchedulerError::InvalidInput(
            "Build script contains null bytes".to_string(),
        ));
    }

    if script.len() > MAX_BUILD_SCRIPT_LENGTH {
        return Err(SchedulerError::InvalidInput(format!(
            "Build script too long: {} bytes (max: {MAX_BUILD_SCRIPT_LENGTH})",
            script.len()
        )));
    }

    for pattern in DANGEROUS_BUILD_SCRIPT_PATTERNS {
        if script.contains(pattern) {
            return Err(SchedulerError::InvalidInput(format!(
                "Build script contains dangerous shell pattern '{pattern}'. \
                 Use an executable script file instead of inline shell commands."
            )));
        }
    }

    let path = Path::new(s);
    if !path.is_absolute() {
        return Err(SchedulerError::InvalidInput(
            "Build script must be an absolute path to an executable file (e.g. /opt/fuzzwizard/scripts/build.sh)".to_string(),
        ));
    }

    let meta = fs::metadata(path).map_err(|_| {
        SchedulerError::InvalidInput(format!(
            "Build script path does not exist or is not accessible: {s}"
        ))
    })?;
    if !meta.is_file() {
        return Err(SchedulerError::InvalidInput(format!(
            "Build script must be a regular file: {s}"
        )));
    }
    let mode = meta.permissions().mode();
    if mode & 0o111 == 0 {
        return Err(SchedulerError::InvalidInput(format!(
            "Build script is not executable: {s}"
        )));
    }

    Ok(())
}
