//! CLI smoke tests for global OCI authentication options.

use std::fs;

use devcontainer::VERSION;
use serde_json::Value;

use crate::support::test_support::{devcontainer_command, unique_temp_dir};

fn utf8_stdout(output: &std::process::Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("utf8 stdout")
}

fn utf8_stderr(output: &std::process::Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("utf8 stderr")
}

#[test]
fn global_options_preserve_root_command_and_nested_help() {
    for (args, expected) in [
        (
            vec!["--oci-auth-hardening", "--help"],
            "devcontainer <command>",
        ),
        (
            vec!["up", "--oci-auth-hardening", "--help"],
            "devcontainer up",
        ),
        (
            vec!["templates", "--oci-auth-hardening", "apply", "--help"],
            "devcontainer templates apply",
        ),
    ] {
        let output = devcontainer_command(None)
            .args(args)
            .output()
            .expect("help command should run");

        assert!(output.status.success(), "{output:?}");
        assert!(utf8_stdout(&output).contains(expected), "{output:?}");
        assert_eq!(utf8_stderr(&output), "");
    }
}

#[test]
fn global_options_preserve_root_command_and_nested_version() {
    for args in [
        vec!["--oci-auth-hardening=false", "--version"],
        vec!["up", "--oci-auth-hardening=false", "--version"],
        vec![
            "templates",
            "--oci-auth-hardening=false",
            "apply",
            "--version",
        ],
    ] {
        let output = devcontainer_command(None)
            .args(args)
            .output()
            .expect("version command should run");

        assert!(output.status.success(), "{output:?}");
        assert_eq!(utf8_stdout(&output), format!("{VERSION}\n"));
        assert_eq!(utf8_stderr(&output), "");
    }
}

#[test]
fn read_configuration_accepts_typed_globals_after_the_command() {
    let root = unique_temp_dir("devcontainer-global-options");
    let config_dir = root.join(".devcontainer");
    fs::create_dir_all(&config_dir).expect("config dir");
    fs::write(
        config_dir.join("devcontainer.json"),
        r#"{ "image": "alpine:3.20" }"#,
    )
    .expect("config");

    let output = devcontainer_command(None)
        .args([
            "read-configuration",
            "--allow-cross-origin-auth-host",
            "registry.example=auth.example",
            "--workspace-folder",
            root.to_string_lossy().as_ref(),
            "--oci-auth-hardening",
        ])
        .output()
        .expect("read-configuration should run");

    assert!(output.status.success(), "{output:?}");
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json stdout");
    assert_eq!(payload["configuration"]["image"], "alpine:3.20");
    assert_eq!(utf8_stderr(&output), "");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn conflicting_duplicate_boolean_globals_are_rejected() {
    let output = devcontainer_command(None)
        .args([
            "--oci-auth-hardening",
            "--oci-auth-hardening=false",
            "--version",
        ])
        .output()
        .expect("conflicting globals should be rejected");

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert_eq!(utf8_stdout(&output), "");
    assert!(
        utf8_stderr(&output)
            .contains("Option --oci-auth-hardening may not be repeated with conflicting values"),
        "{output:?}"
    );
}

#[test]
fn equivalent_duplicate_boolean_globals_are_accepted() {
    let output = devcontainer_command(None)
        .args([
            "--oci-auth-hardening=true",
            "--oci-auth-hardening",
            "--version",
        ])
        .output()
        .expect("equivalent globals should be accepted");

    assert!(output.status.success(), "{output:?}");
    assert_eq!(utf8_stdout(&output), format!("{VERSION}\n"));
    assert_eq!(utf8_stderr(&output), "");
}
