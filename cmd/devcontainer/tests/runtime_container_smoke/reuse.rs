//! Runtime container smoke tests for container reuse and restart behavior.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use devcontainer::config::{substitute_local_context, ConfigContext};
use serde_json::json;

use crate::support::runtime_harness::{write_devcontainer_config, RuntimeHarness};

const DEVCONTAINER_LOCAL_FOLDER_LABEL: &str = "devcontainer.local_folder";
const DEVCONTAINER_CONFIG_FILE_LABEL: &str = "devcontainer.config_file";

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
fn up_preserves_custom_id_labels_for_followup_exec() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(&workspace).expect("workspace dir");
    write_devcontainer_config(&workspace, "{\n  \"image\": \"alpine:3.20\"\n}\n");

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let up_output = harness.run(
        &[
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--id-label",
            "example.label=from-user",
        ],
        &[("FAKE_PODMAN_PS_DISABLE_DEFAULT", "1")],
    );

    assert!(up_output.status.success(), "{up_output:?}");

    let exec_output = harness.run(
        &[
            "exec",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--id-label",
            "example.label=from-user",
            "/bin/echo",
            "hello-via-label",
        ],
        &[("FAKE_PODMAN_PS_REQUIRE_LABEL", "example.label=from-user")],
    );

    assert!(exec_output.status.success(), "{exec_output:?}");
    assert_eq!(
        String::from_utf8(exec_output.stdout).expect("utf8 stdout"),
        "hello-via-label\n"
    );

    let invocations = harness.read_invocations();
    assert!(invocations.contains("--label example.label=from-user"));
    assert!(invocations.contains("ps -q --filter label=example.label=from-user"));
}

#[test]
fn up_reuses_existing_container_when_labels_match() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(&workspace).expect("workspace dir");
    write_devcontainer_config(&workspace, "{\n  \"image\": \"alpine:3.20\"\n}\n");

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
        ],
        &[("FAKE_PODMAN_PS_OUTPUT", "existing-container-id")],
    );

    assert!(output.status.success(), "{output:?}");
    let payload = harness.parse_stdout_json(&output);
    assert_eq!(payload["containerId"], "existing-container-id");
    let invocations = harness.read_invocations();
    assert!(invocations.contains("ps -q "));
    assert!(!invocations.contains("run "));
}

#[test]
fn up_reusing_running_container_skips_create_only_lifecycle_hooks() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(&workspace).expect("workspace dir");
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"alpine:3.20\",\n  \"onCreateCommand\": \"echo on-create\",\n  \"updateContentCommand\": \"echo update-content\",\n  \"postCreateCommand\": \"echo post-create\",\n  \"postStartCommand\": \"echo post-start\",\n  \"postAttachCommand\": \"echo post-attach\"\n}\n",
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
        &[("FAKE_PODMAN_PS_OUTPUT", "existing-container-id")],
    );

    assert!(output.status.success(), "{output:?}");
    let exec_log = harness.read_exec_log();
    assert!(!exec_log.contains("/bin/sh -lc echo on-create"));
    assert!(!exec_log.contains("/bin/sh -lc echo update-content"));
    assert!(!exec_log.contains("/bin/sh -lc echo post-create"));
    assert!(!exec_log.contains("/bin/sh -lc echo post-start"));
    assert!(exec_log.contains("/bin/sh -lc echo post-attach"));
}

#[test]
fn up_resumes_stopped_containers_instead_of_creating_new_ones() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(&workspace).expect("workspace dir");
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"alpine:3.20\",\n  \"onCreateCommand\": \"echo on-create\",\n  \"updateContentCommand\": \"echo update-content\",\n  \"postCreateCommand\": \"echo post-create\",\n  \"postStartCommand\": \"echo post-start\",\n  \"postAttachCommand\": \"echo post-attach\"\n}\n",
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
            ("FAKE_PODMAN_PS_OUTPUT", "stopped-container-id"),
            ("FAKE_PODMAN_PS_REQUIRE_ALL", "1"),
        ],
    );

    assert!(output.status.success(), "{output:?}");
    let payload = harness.parse_stdout_json(&output);
    assert_eq!(payload["containerId"], "stopped-container-id");
    let invocations = harness.read_invocations();
    assert!(invocations.contains("ps -q -a "));
    assert!(invocations.contains("start stopped-container-id"));
    assert!(!invocations.contains("run "));
    let exec_log = harness.read_exec_log();
    assert!(!exec_log.contains("/bin/sh -lc echo on-create"));
    assert!(!exec_log.contains("/bin/sh -lc echo update-content"));
    assert!(!exec_log.contains("/bin/sh -lc echo post-create"));
    assert!(exec_log.contains("/bin/sh -lc echo post-start"));
    assert!(exec_log.contains("/bin/sh -lc echo post-attach"));
}

#[test]
fn up_remove_existing_container_recreates_the_container() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(&workspace).expect("workspace dir");
    write_devcontainer_config(&workspace, "{\n  \"image\": \"alpine:3.20\"\n}\n");

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--remove-existing-container",
        ],
        &[("FAKE_PODMAN_PS_OUTPUT", "existing-container-id")],
    );

    assert!(output.status.success(), "{output:?}");
    let payload = harness.parse_stdout_json(&output);
    assert_eq!(payload["containerId"], "fake-container-id");
    let invocations = harness.read_invocations();
    assert!(invocations.contains("rm -f existing-container-id"));
    assert!(invocations.contains("run "));
}

#[test]
fn up_expect_existing_container_fails_when_missing() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(&workspace).expect("workspace dir");
    let config_file = write_devcontainer_config(&workspace, "{\n  \"image\": \"alpine:3.20\"\n}\n");
    let expected_workspace = fs::canonicalize(&workspace).expect("canonical workspace");
    let expected_config = fs::canonicalize(&config_file).expect("canonical config");

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--expect-existing-container",
        ],
        &[("FAKE_PODMAN_PS_DISABLE_DEFAULT", "1")],
    );

    assert!(!output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stderr)
            .expect("utf8 stderr")
            .trim(),
        format!(
            "Dev container not found for workspace folder '{}' and config file '{}'. If the container was created with a different config file, pass --config <path> or set DEVCONTAINER_CONFIG.",
            expected_workspace.display(),
            expected_config.display()
        )
    );
}

#[test]
fn up_reuse_applies_legacy_devcontainer_id_before_initialize_command() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(&workspace).expect("workspace dir");
    let init_id_path = workspace.join("initialize-id.txt");
    let config_path = write_devcontainer_config(
        &workspace,
        &format!(
            "{{\n  \"image\": \"alpine:3.20\",\n  \"initializeCommand\": \"printf %s ${{devcontainerId}} > {}\",\n  \"postAttachCommand\": \"echo ${{devcontainerId}}\"\n}}\n",
            init_id_path.display()
        ),
    );
    let workspace = workspace.canonicalize().expect("workspace path");
    let config_path = config_path.canonicalize().expect("config path");
    let legacy_id = devcontainer_id_for_labels(
        &workspace,
        HashMap::from([(
            DEVCONTAINER_LOCAL_FOLDER_LABEL.to_string(),
            workspace.display().to_string(),
        )]),
    );
    let current_id = devcontainer_id_for_labels(
        &workspace,
        HashMap::from([
            (
                DEVCONTAINER_LOCAL_FOLDER_LABEL.to_string(),
                workspace.display().to_string(),
            ),
            (
                DEVCONTAINER_CONFIG_FILE_LABEL.to_string(),
                config_path.display().to_string(),
            ),
        ]),
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
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
        ],
        &[
            ("FAKE_PODMAN_PS_OUTPUT", "existing-container-id"),
            (
                "FAKE_PODMAN_INSPECT_FILE",
                inspect_path.to_string_lossy().as_ref(),
            ),
        ],
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(&init_id_path).expect("initialize id"),
        legacy_id
    );
    let exec_log = harness.read_exec_log();
    assert!(exec_log.contains(&format!("/bin/sh -lc echo {legacy_id}")));
    assert!(!exec_log.contains(&format!("/bin/sh -lc echo {current_id}")));
}
