//! Public CLI coverage for literal Docker argument forwarding.

mod support;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use support::runtime_harness::{write_devcontainer_config, RuntimeHarness};

/// Quote a single shell argument used by the checking engine wrapper.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Reject any engine invocation that loses or changes the required prefix.
fn checking_engine(harness: &RuntimeHarness, prefix: &[&str]) -> PathBuf {
    let wrapper = harness.root.join("checking-engine");
    let mut script = String::from("#!/bin/sh\nset -eu\n");
    for expected in prefix {
        script.push_str(&format!(
            "[ \"$#\" -gt 0 ] && [ \"$1\" = {} ] || exit 91\nshift\n",
            shell_quote(expected)
        ));
    }
    script.push_str(&format!(
        "exec {} \"$@\"\n",
        shell_quote(harness.fake_podman.to_str().expect("engine path"))
    ));
    fs::write(&wrapper, script).expect("checking engine");
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).expect("executable wrapper");
    wrapper
}

/// Assemble both repeated inline values and a spaced Docker argument.
fn command_args(command: &str, engine: &Path, prefix: &[&str], tail: &[&str]) -> Vec<String> {
    let mut args = vec![
        command.into(),
        "--docker-path".into(),
        engine.display().to_string(),
    ];
    for (index, value) in prefix.iter().enumerate() {
        if index == 1 {
            args.extend(["--docker-arg".into(), (*value).into()]);
        } else {
            args.push(format!("--docker-arg={value}"));
        }
    }
    args.extend(tail.iter().map(|value| (*value).to_string()));
    args
}

/// Every public runtime command must keep literal prefixes out of CLI and child parsing.
#[test]
fn docker_arguments_reach_build_up_lifecycle_inspection_and_exec() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    let config = write_devcontainer_config(
        &workspace,
        r#"{"build":{"dockerfile":"Dockerfile"},"updateRemoteUserUID":false,"onCreateCommand":"echo created"}"#,
    );
    fs::write(workspace.join("Dockerfile"), "FROM alpine:3.20\n").expect("Dockerfile");
    let prefix = [
        "--session",
        "a b",
        "001",
        "a=b",
        "\"quoted\"",
        "'quoted'",
        "",
        "--",
        "a\\b",
        "--oci-auth-hardening",
        "--pull-always",
    ];
    let engine = checking_engine(&harness, &prefix);
    let workspace = workspace.to_str().expect("workspace path");
    let config = config.to_str().expect("config path");
    let commands: &[(&str, &[&str])] = &[
        (
            "build",
            &[
                "--workspace-folder",
                workspace,
                "--image-name",
                "test/docker-arg",
            ],
        ),
        (
            "up",
            &[
                "--workspace-folder",
                workspace,
                "--remove-existing-container",
            ],
        ),
        (
            "set-up",
            &["--container-id", "fake-container-id", "--config", config],
        ),
        (
            "run-user-commands",
            &["--container-id", "fake-container-id", "--config", config],
        ),
        (
            "read-configuration",
            &[
                "--container-id",
                "fake-container-id",
                "--workspace-folder",
                workspace,
            ],
        ),
        (
            "exec",
            &[
                "--container-id",
                "fake-container-id",
                "/bin/echo",
                "--docker-arg=child-value",
                "",
            ],
        ),
        ("upgrade", &["--workspace-folder", workspace]),
    ];
    for (command, tail) in commands {
        let args = command_args(command, &engine, &prefix, tail);
        let output = harness.run(&args.iter().map(String::as_str).collect::<Vec<_>>(), &[]);
        assert!(output.status.success(), "{command}: {output:?}");
        if *command == "exec" {
            assert_eq!(
                String::from_utf8(output.stdout).expect("exec output"),
                "--docker-arg=child-value \n"
            );
        }
    }
    let calls = harness.read_invocations();
    assert!(calls.contains("build "), "{calls}");
    assert!(calls.contains("exec "), "{calls}");
    assert!(calls.contains("inspect "), "{calls}");
}

/// The Compose plugin receives global arguments while a separate executable does not.
#[test]
fn compose_plugin_receives_docker_arguments() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    write_devcontainer_config(
        &workspace,
        r#"{"dockerComposeFile":"compose.yml","service":"app","updateRemoteUserUID":false}"#,
    );
    fs::write(
        workspace.join(".devcontainer/compose.yml"),
        "services:\n  app:\n    image: alpine:3.20\n",
    )
    .expect("Compose file");
    let prefix = ["-H", "tcp://example.invalid:2375"];
    let engine = checking_engine(&harness, &prefix);
    let args = command_args(
        "up",
        &engine,
        &prefix,
        &[
            "--workspace-folder",
            workspace.to_str().expect("workspace path"),
            "--remove-existing-container",
        ],
    );
    let output = harness.run(&args.iter().map(String::as_str).collect::<Vec<_>>(), &[]);
    assert!(output.status.success(), "{output:?}");
    let calls = harness.read_invocations();
    assert!(calls.contains("compose version --short"), "{calls}");
    assert!(
        calls
            .lines()
            .any(|line| line.starts_with("compose ") && line.contains(" up ")),
        "{calls}"
    );
}
