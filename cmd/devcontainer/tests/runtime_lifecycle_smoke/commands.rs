//! Smoke tests for lifecycle command execution across up, set-up, and run-user-commands flows.

use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

use devcontainer::config::{substitute_local_context, ConfigContext};

use crate::support::runtime_harness::{write_devcontainer_config, RuntimeHarness};

const DEVCONTAINER_LOCAL_FOLDER_LABEL: &str = "devcontainer.local_folder";

fn devcontainer_id_for_labels(workspace: &Path, labels: HashMap<String, String>) -> String {
    substitute_local_context(
        &json!("${devcontainerId}"),
        &ConfigContext {
            workspace_folder: workspace.to_path_buf(),
            env: HashMap::new(),
            container_workspace_folder: None,
            id_labels: labels,
        },
    )
    .as_str()
    .expect("devcontainer id")
    .to_string()
}

#[test]
fn run_user_commands_resolves_container_ids_from_headered_ps_output() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(&workspace).expect("workspace dir");
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"alpine:3.20\",\n  \"postCreateCommand\": \"echo post-create\"\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "run-user-commands",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
        ],
        &[
            ("FAKE_PODMAN_PS_OUTPUT", "fake-container-id"),
            ("FAKE_PODMAN_PS_WITH_HEADER", "1"),
        ],
    );

    assert!(output.status.success(), "{output:?}");
    let invocations = harness.read_invocations();
    assert!(invocations.contains("ps -q "));
    let exec_log = harness.read_exec_log();
    assert!(exec_log.contains("/bin/sh -lc echo post-create"));
}

#[test]
fn run_user_commands_with_container_id_loads_metadata_lifecycle_hooks() {
    let harness = RuntimeHarness::new();
    let inspect_path = harness.root.join("inspect.json");
    let metadata = serde_json::to_string(&json!({
        "postCreateCommand": "echo post-create-from-metadata",
        "postAttachCommand": "echo post-attach-from-metadata",
        "workspaceFolder": "/metadata-workspace"
    }))
    .expect("metadata");
    fs::write(
        &inspect_path,
        json!([{
            "Config": {
                "Labels": {
                    "devcontainer.metadata": metadata
                }
            },
            "Mounts": []
        }])
        .to_string(),
    )
    .expect("inspect json");

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "run-user-commands",
            "--docker-path",
            fake_podman.as_str(),
            "--container-id",
            "fake-container-id",
        ],
        &[(
            "FAKE_PODMAN_INSPECT_FILE",
            inspect_path.to_string_lossy().as_ref(),
        )],
    );

    assert!(output.status.success(), "{output:?}");
    let payload = harness.parse_stdout_json(&output);
    assert_eq!(payload["remoteWorkspaceFolder"], "/metadata-workspace");
    let invocations = harness.read_invocations();
    assert!(invocations.contains("inspect fake-container-id"));
    let exec_log = harness.read_exec_log();
    assert!(exec_log.contains("/bin/sh -lc echo post-create-from-metadata"));
    assert!(exec_log.contains("/bin/sh -lc echo post-attach-from-metadata"));
}

#[test]
fn lifecycle_commands_run_as_the_configured_remote_user() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(&workspace).expect("workspace dir");
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"alpine:3.20\",\n  \"remoteUser\": \"vscode\",\n  \"postCreateCommand\": \"echo ready\"\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
        ],
        &[("FAKE_PODMAN_PS_DISABLE_DEFAULT", "1")],
    );

    assert!(output.status.success(), "{output:?}");
    let invocations = harness.read_invocations();
    assert!(invocations.contains("exec --workdir /workspaces/workspace --user vscode"));
}

#[test]
fn set_up_and_run_user_commands_target_existing_containers() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(&workspace).expect("workspace dir");
    let config_path = write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"alpine:3.20\",\n  \"postCreateCommand\": \"echo post-create\",\n  \"postAttachCommand\": \"echo post-attach\"\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let set_up_output = harness.run(
        &[
            "set-up",
            "--docker-path",
            fake_podman.as_str(),
            "--container-id",
            "fake-container-id",
            "--config",
            config_path.to_string_lossy().as_ref(),
            "--include-configuration",
        ],
        &[],
    );

    assert!(set_up_output.status.success(), "{set_up_output:?}");
    let payload = harness.parse_stdout_json(&set_up_output);
    assert_eq!(payload["containerId"], "fake-container-id");
    assert_eq!(payload["configuration"]["image"], "alpine:3.20");

    let run_user_commands_output = harness.run(
        &[
            "run-user-commands",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
        ],
        &[("FAKE_PODMAN_PS_OUTPUT", "fake-container-id")],
    );

    assert!(
        run_user_commands_output.status.success(),
        "{run_user_commands_output:?}"
    );
    let exec_log = harness.read_exec_log();
    assert!(exec_log.contains("/bin/sh -lc echo post-create"));
    assert!(exec_log.contains("/bin/sh -lc echo post-attach"));
}

#[test]
fn compose_lifecycle_commands_honor_explicit_container_id() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(workspace.join(".devcontainer")).expect("workspace config dir");
    fs::write(
        workspace.join(".devcontainer").join("docker-compose.yml"),
        "services:\n  app:\n    image: alpine:3.20\n",
    )
    .expect("compose");
    write_devcontainer_config(
        &workspace,
        "{\n  \"dockerComposeFile\": \"docker-compose.yml\",\n  \"service\": \"app\",\n  \"workspaceFolder\": \"/workspace\",\n  \"postCreateCommand\": \"echo post-create\",\n  \"postAttachCommand\": \"echo post-attach\"\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let set_up_output = harness.run(
        &[
            "set-up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--container-id",
            "fake-compose-container-id",
            "--include-configuration",
        ],
        &[],
    );

    assert!(set_up_output.status.success(), "{set_up_output:?}");
    let payload = harness.parse_stdout_json(&set_up_output);
    assert_eq!(payload["containerId"], "fake-compose-container-id");
    assert_eq!(payload["configuration"]["service"], "app");

    let run_user_commands_output = harness.run(
        &[
            "run-user-commands",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--container-id",
            "fake-compose-container-id",
        ],
        &[],
    );

    assert!(
        run_user_commands_output.status.success(),
        "{run_user_commands_output:?}"
    );
    let invocations = harness.read_invocations();
    assert!(!invocations.contains("compose --project-name"));
    assert!(invocations.contains("exec --workdir /workspace"));
    assert!(invocations.contains("-e HOME=/root"));
    assert!(invocations.contains("fake-compose-container-id /bin/sh -lc echo post-create"));
    assert!(invocations.contains("fake-compose-container-id /bin/sh -lc echo post-attach"));
    let exec_log = harness.read_exec_log();
    assert!(exec_log.contains("/bin/sh -lc echo post-create"));
    assert!(exec_log.contains("/bin/sh -lc echo post-attach"));
}

#[test]
fn lifecycle_commands_receive_secrets_from_file() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    let secrets_path = harness.root.join("secrets.json");
    fs::create_dir_all(&workspace).expect("workspace dir");
    fs::write(
        &secrets_path,
        "{\n  \"SECRET_TOKEN\": \"from-secret-file\"\n}\n",
    )
    .expect("secrets file");
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"alpine:3.20\",\n  \"postCreateCommand\": \"printf %s \\\"$SECRET_TOKEN\\\" > /workspaces/workspace/secret.txt\"\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--secrets-file",
            secrets_path.to_string_lossy().as_ref(),
        ],
        &[("FAKE_PODMAN_PS_DISABLE_DEFAULT", "1")],
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(workspace.join("secret.txt")).expect("secret file"),
        "from-secret-file"
    );
    let invocations = harness.read_invocations();
    assert!(invocations.contains("-e SECRET_TOKEN=from-secret-file"));
}

#[test]
fn up_lifecycle_commands_receive_derived_home() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(&workspace).expect("workspace dir");
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"alpine:3.20\",\n  \"remoteUser\": \"vscode\",\n  \"postCreateCommand\": \"printf %s \\\"$HOME\\\" > /workspaces/workspace/home.txt\"\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
        ],
        &[("FAKE_PODMAN_PS_DISABLE_DEFAULT", "1")],
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(workspace.join("home.txt")).expect("home file"),
        "/home/vscode"
    );
    let invocations = harness.read_invocations();
    assert!(invocations.contains("-e HOME=/home/vscode"));
}

#[test]
fn up_lifecycle_commands_derive_home_from_container_user_when_devcontainer_user_is_unset() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(&workspace).expect("workspace dir");
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"alpine:3.20\",\n  \"postCreateCommand\": \"printf %s \\\"$HOME\\\" > /workspaces/workspace/home.txt\"\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
        ],
        &[
            ("FAKE_PODMAN_PS_DISABLE_DEFAULT", "1"),
            ("FAKE_PODMAN_CONTAINER_USER", "vscode"),
            ("FAKE_PODMAN_CONTAINER_HOME", "/root"),
        ],
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(workspace.join("home.txt")).expect("home file"),
        "/home/vscode"
    );
    let invocations = harness.read_invocations();
    assert!(invocations.contains("exec --workdir /workspaces/workspace -e HOME=/home/vscode"));
    assert!(!invocations.contains("--user vscode"));
}

#[test]
fn lifecycle_array_commands_preserve_argument_boundaries() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(&workspace).expect("workspace dir");
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"alpine:3.20\",\n  \"postCreateCommand\": [\"printf\", \"%s\", \"foo='bar baz'\"]\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
        ],
        &[("FAKE_PODMAN_PS_DISABLE_DEFAULT", "1")],
    );

    assert!(output.status.success(), "{output:?}");
    let exec_argv = harness.read_exec_argv_log();
    assert!(exec_argv.contains("[printf]\n[%s]\n[foo='bar baz']"));
    assert!(!exec_argv.contains("[sh]\n[-lc]\n[printf %s foo='bar baz']"));
}

#[test]
fn object_lifecycle_commands_are_executed() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(&workspace).expect("workspace dir");
    let config_path = write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"alpine:3.20\",\n  \"postCreateCommand\": {\n    \"alpha\": \"echo first\",\n    \"beta\": [\"printf\", \"%s\", \"second value\"]\n  }\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "set-up",
            "--docker-path",
            fake_podman.as_str(),
            "--container-id",
            "fake-container-id",
            "--config",
            config_path.to_string_lossy().as_ref(),
        ],
        &[],
    );

    assert!(output.status.success(), "{output:?}");
    let invocations = harness.read_invocations();
    assert!(invocations.contains("exec --workdir /workspaces/workspace"));
    assert!(invocations.contains("-e HOME=/root"));
    assert!(invocations.contains("fake-container-id /bin/sh -lc echo first"));
    assert!(invocations.contains("fake-container-id printf %s second value"));
}

#[test]
fn run_user_commands_with_container_id_preserves_legacy_devcontainer_id_labels() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(&workspace).expect("workspace dir");
    let config_path = write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"alpine:3.20\",\n  \"postAttachCommand\": \"echo ${devcontainerId}\"\n}\n",
    );
    let workspace = workspace.canonicalize().expect("workspace path");
    let legacy_id = devcontainer_id_for_labels(
        &workspace,
        HashMap::from([(
            DEVCONTAINER_LOCAL_FOLDER_LABEL.to_string(),
            workspace.display().to_string(),
        )]),
    );
    let inspect_path = harness.root.join("inspect.json");
    fs::write(
        &inspect_path,
        json!([{
            "Config": {
                "Labels": {
                    DEVCONTAINER_LOCAL_FOLDER_LABEL: workspace.display().to_string()
                }
            },
            "Mounts": []
        }])
        .to_string(),
    )
    .expect("inspect json");

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "run-user-commands",
            "--docker-path",
            fake_podman.as_str(),
            "--container-id",
            "fake-container-id",
            "--config",
            config_path.to_string_lossy().as_ref(),
        ],
        &[(
            "FAKE_PODMAN_INSPECT_FILE",
            inspect_path.to_string_lossy().as_ref(),
        )],
    );

    assert!(output.status.success(), "{output:?}");
    let exec_log = harness.read_exec_log();
    assert!(exec_log.contains(&format!("/bin/sh -lc echo {legacy_id}")));
}
