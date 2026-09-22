// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

//! The container options coming from the configuration are appended after the
//! flags that build the sandbox, so the container runtime lets them override
//! that hardening. These tests pin down the allowlist that keeps them from
//! doing so.

use scheduler::validation::validate_container_options;

fn options(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

fn accepts(args: &[&str]) -> bool {
    validate_container_options(&options(args)).is_ok()
}

#[test]
fn resource_limits_are_accepted() {
    assert!(accepts(&["--platform=linux/arm64"]));
    assert!(accepts(&["--memory", "4g"]));
    assert!(accepts(&["--memory=4g"]));
    assert!(accepts(&["--cpus=2", "--memory-swap", "8g"]));
    assert!(accepts(&["--ulimit", "nofile=1024:2048"]));
    assert!(accepts(&["--init"]));
    assert!(accepts(&["-e", "FUZZ_MODE=fast"]));
    assert!(accepts(&[]));
    assert!(accepts(&[""]));
}

#[test]
fn namespace_and_privilege_flags_are_rejected() {
    for args in [
        vec!["--privileged"],
        vec!["--cap-add=SYS_ADMIN"],
        vec!["--cap-add", "SYS_ADMIN"],
        vec!["--device=/dev/kvm"],
        vec!["--security-opt", "seccomp=unconfined"],
        vec!["-v", "/:/host"],
        vec!["--volume=/:/host"],
        vec!["--mount", "type=bind,src=/,dst=/host"],
    ] {
        assert!(!accepts(&args), "should have been rejected: {args:?}");
    }
}

/// The previous denylist matched each option in isolation with `starts_with`,
/// so splitting a flag from its value walked straight past it.
#[test]
fn space_separated_host_namespaces_are_rejected() {
    for args in [
        vec!["--pid", "host"],
        vec!["--network", "host"],
        vec!["--net", "host"],
        vec!["--ipc", "host"],
        vec!["--userns", "host"],
    ] {
        assert!(!accepts(&args), "should have been rejected: {args:?}");
    }
}

/// Flags that were never on the denylist but undo the hardening all the same,
/// since the runtime honours the last occurrence.
#[test]
fn flags_overriding_the_sandbox_are_rejected() {
    for args in [
        vec!["--read-only=false"],
        vec!["--uts=host"],
        vec!["--cgroupns=host"],
        vec!["--user=0"],
        vec!["--entrypoint", "/bin/sh"],
        vec!["--tmpfs", "/tmp:exec"],
    ] {
        assert!(!accepts(&args), "should have been rejected: {args:?}");
    }
}

#[test]
fn a_flag_cannot_swallow_the_next_flag_as_its_value() {
    assert!(!accepts(&["--memory", "--privileged"]));
    assert!(!accepts(&["--memory"]));
}

#[test]
fn limits_cannot_be_turned_negative() {
    assert!(!accepts(&["--pids-limit=-1"]));
    assert!(!accepts(&["--pids-limit", "-1"]));
}

#[test]
fn values_cannot_carry_shell_metacharacters() {
    assert!(!accepts(&["--label", "a`id`"]));
    assert!(!accepts(&["--env=NAME=$(id)"]));
    assert!(!accepts(&["--hostname", "a;b"]));
}

#[test]
fn valueless_flags_reject_a_value() {
    assert!(!accepts(&["--init=false"]));
}
