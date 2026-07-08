//! Runtime container smoke tests for compose-backed up flows.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use devcontainer::config::{substitute_local_context, ConfigContext};
use serde_json::json;

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

fn generated_override_contents(harness: &RuntimeHarness) -> String {
    let log = harness.read_compose_file_log();
    let mut capture = false;
    let mut content = String::new();
    for line in log.lines() {
        if let Some(path) = line.strip_prefix("BEGIN ") {
            capture = path.contains("devcontainer-compose-override");
            continue;
        }
        if line.starts_with("END ") {
            if capture {
                break;
            }
            capture = false;
            continue;
        }
        if capture {
            content.push_str(line);
            content.push('\n');
        }
    }
    content
}

fn write_executable(path: &Path, contents: String) {
    fs::write(path, contents).expect("executable wrapper");
    let mut permissions = fs::metadata(path)
        .expect("executable wrapper metadata")
        .permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o755);
    }
    fs::set_permissions(path, permissions).expect("executable wrapper permissions");
}

fn compose_label_lookup_args(project_name: &str, service: &str, include_stopped: bool) -> String {
    let all_arg = if include_stopped { " -a" } else { "" };
    format!("ps -q{all_arg} --filter label=com.docker.compose.project={project_name} --filter label=com.docker.compose.service={service}")
}

#[test]
fn up_starts_compose_services_and_exec_uses_compose_container_lookup() {
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
        "{\n  \"dockerComposeFile\": \"docker-compose.yml\",\n  \"service\": \"app\",\n  \"workspaceFolder\": \"/workspace\",\n  \"remoteUser\": \"vscode\",\n  \"postCreateCommand\": \"echo ready\"\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let up_output = harness.run(
        &[
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
        ],
        &[],
    );

    assert!(up_output.status.success(), "{up_output:?}");
    let up_payload = harness.parse_stdout_json(&up_output);
    assert_eq!(up_payload["containerId"], "fake-compose-container-id");
    assert_eq!(up_payload["composeProjectName"], "workspace_devcontainer");
    assert_eq!(up_payload["remoteWorkspaceFolder"], "/workspace");

    let exec_output = harness.run(
        &[
            "exec",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "/bin/echo",
            "hello-from-compose",
        ],
        &[],
    );

    assert!(exec_output.status.success(), "{exec_output:?}");
    assert_eq!(
        String::from_utf8(exec_output.stdout).expect("utf8 stdout"),
        "hello-from-compose\n"
    );

    let invocations = harness.read_invocations();
    assert!(invocations.contains("compose --project-name workspace_devcontainer -f "));
    assert!(invocations.contains(" up -d"));
    assert!(!invocations.contains(" up -d app"));
    assert!(invocations.contains(&compose_label_lookup_args(
        "workspace_devcontainer",
        "app",
        false
    )));
    assert!(invocations.contains("exec -i --workdir /workspace --user vscode"));
    assert!(invocations.contains("-e HOME=/home/vscode"));
    assert!(invocations.contains("fake-compose-container-id /bin/echo hello-from-compose"));

    let exec_log = harness.read_exec_log();
    assert!(exec_log.contains("/bin/sh -lc echo ready"));
}

#[test]
fn up_generated_override_preserves_compose_version_prefix() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(workspace.join(".devcontainer")).expect("workspace config dir");
    fs::write(
        workspace.join(".devcontainer").join("docker-compose.yml"),
        "version: '3.8'\nservices:\n  app:\n    image: alpine:3.20\n",
    )
    .expect("compose");
    write_devcontainer_config(
        &workspace,
        "{\n  \"dockerComposeFile\": \"docker-compose.yml\",\n  \"service\": \"app\",\n  \"workspaceFolder\": \"/workspace\"\n}\n",
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
        &[],
    );

    assert!(output.status.success(), "{output:?}");
    let override_content = generated_override_contents(&harness);
    assert!(
        override_content.starts_with("version: '3.8'\n"),
        "{override_content}"
    );
}

#[test]
fn up_uses_root_remote_workspace_folder_when_compose_workspace_folder_is_omitted() {
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
        "{\n  \"dockerComposeFile\": \"docker-compose.yml\",\n  \"service\": \"app\"\n}\n",
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
        &[],
    );

    assert!(output.status.success(), "{output:?}");
    let payload = harness.parse_stdout_json(&output);
    assert_eq!(payload["remoteWorkspaceFolder"], "/");
}

#[test]
fn up_honors_run_services_and_includes_the_primary_service() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(workspace.join(".devcontainer")).expect("workspace config dir");
    fs::write(
        workspace.join(".devcontainer").join("docker-compose.yml"),
        "services:\n  app:\n    image: alpine:3.20\n  worker:\n    image: alpine:3.20\n",
    )
    .expect("compose");
    write_devcontainer_config(
        &workspace,
        "{\n  \"dockerComposeFile\": \"docker-compose.yml\",\n  \"service\": \"app\",\n  \"workspaceFolder\": \"/workspace\",\n  \"runServices\": [\"worker\"]\n}\n",
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
        &[],
    );

    assert!(output.status.success(), "{output:?}");
    let invocations = harness.read_invocations();
    assert!(invocations.contains(" up -d worker app"));
}

#[test]
fn up_re_resolves_recreated_compose_container_ids() {
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
        "{\n  \"dockerComposeFile\": \"docker-compose.yml\",\n  \"service\": \"app\",\n  \"workspaceFolder\": \"/workspace\",\n  \"postCreateCommand\": \"echo recreated-post-create\",\n  \"postAttachCommand\": \"echo recreated-post-attach\"\n}\n",
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
            ("FAKE_PODMAN_PS_OUTPUT_BEFORE_UP", "old-compose-id"),
            ("FAKE_PODMAN_PS_OUTPUT_AFTER_UP", "new-compose-id"),
        ],
    );

    assert!(output.status.success(), "{output:?}");
    let payload = harness.parse_stdout_json(&output);
    assert_eq!(payload["containerId"], "new-compose-id");

    let invocations = harness.read_invocations();
    assert!(invocations.contains(&compose_label_lookup_args(
        "workspace_devcontainer",
        "app",
        false
    )));
    assert!(invocations.contains("exec --workdir /workspace"));
    assert!(invocations.contains("-e HOME=/root"));
    assert!(invocations.contains("new-compose-id /bin/sh -lc echo recreated-post-create"));

    let exec_log = harness.read_exec_log();
    assert!(exec_log.contains("/bin/sh -lc echo recreated-post-create"));
    assert!(exec_log.contains("/bin/sh -lc echo recreated-post-attach"));
}

#[test]
fn exec_accepts_custom_compose_binary_for_compose_workspaces() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    let compose_wrapper = harness.root.join("podman-compose");
    fs::create_dir_all(workspace.join(".devcontainer")).expect("workspace config dir");
    fs::write(
        workspace.join(".devcontainer").join("docker-compose.yml"),
        "services:\n  app:\n    image: alpine:3.20\n",
    )
    .expect("compose");
    write_devcontainer_config(
        &workspace,
        "{\n  \"dockerComposeFile\": \"docker-compose.yml\",\n  \"service\": \"app\",\n  \"workspaceFolder\": \"/workspace\",\n  \"remoteUser\": \"vscode\"\n}\n",
    );
    fs::write(
        &compose_wrapper,
        format!(
            "#!/bin/sh\nexec \"{}\" compose \"$@\"\n",
            harness.fake_podman.display()
        ),
    )
    .expect("compose wrapper");
    let mut permissions = fs::metadata(&compose_wrapper)
        .expect("compose wrapper metadata")
        .permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o755);
    }
    fs::set_permissions(&compose_wrapper, permissions).expect("compose wrapper permissions");

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let up_output = harness.run(
        &[
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--docker-compose-path",
            compose_wrapper.to_string_lossy().as_ref(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
        ],
        &[],
    );

    assert!(up_output.status.success(), "{up_output:?}");

    let output = harness.run(
        &[
            "exec",
            "--docker-path",
            fake_podman.as_str(),
            "--docker-compose-path",
            compose_wrapper.to_string_lossy().as_ref(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "/bin/echo",
            "hello-from-custom-compose",
        ],
        &[],
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).expect("utf8 stdout"),
        "hello-from-custom-compose\n"
    );

    let invocations = harness.read_invocations();
    assert!(invocations.contains("compose --project-name workspace_devcontainer -f "));
    assert!(invocations.contains(" up -d"));
    assert!(invocations.contains(&compose_label_lookup_args(
        "workspace_devcontainer",
        "app",
        false
    )));
    assert!(invocations.contains("exec -i --workdir /workspace --user vscode"));
    assert!(invocations.contains("-e HOME=/home/vscode"));
    assert!(invocations.contains("fake-compose-container-id /bin/echo hello-from-custom-compose"));
}

#[test]
fn up_uses_env_backed_engine_and_compose_paths_for_compose_workspaces() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    let compose_wrapper = harness.root.join("podman-compose");
    fs::create_dir_all(workspace.join(".devcontainer")).expect("workspace config dir");
    fs::write(
        workspace.join(".devcontainer").join("docker-compose.yml"),
        "services:\n  app:\n    image: alpine:3.20\n",
    )
    .expect("compose");
    write_devcontainer_config(
        &workspace,
        "{\n  \"dockerComposeFile\": \"docker-compose.yml\",\n  \"service\": \"app\",\n  \"workspaceFolder\": \"/workspace\"\n}\n",
    );
    write_executable(
        &compose_wrapper,
        format!(
            "#!/bin/sh\nexec \"{}\" compose \"$@\"\n",
            harness.fake_podman.display()
        ),
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let compose_path = compose_wrapper.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "up",
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
        ],
        &[
            ("DEVCONTAINER_DOCKER_PATH", fake_podman.as_str()),
            ("DEVCONTAINER_DOCKER_COMPOSE_PATH", compose_path.as_str()),
        ],
    );

    assert!(output.status.success(), "{output:?}");
    let payload = harness.parse_stdout_json(&output);
    assert_eq!(payload["containerId"], "fake-compose-container-id");
    let invocations = harness.read_invocations();
    assert!(invocations.contains("compose --project-name workspace_devcontainer -f "));
    assert!(invocations.contains(" up -d"));
}

#[test]
fn exec_with_standalone_podman_compose_uses_engine_label_lookup() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    let compose_wrapper = harness.root.join("podman-compose");
    fs::create_dir_all(workspace.join(".devcontainer")).expect("workspace config dir");
    fs::write(
        workspace.join(".devcontainer").join("docker-compose.yml"),
        "services:\n  app:\n    image: alpine:3.20\n",
    )
    .expect("compose");
    write_devcontainer_config(
        &workspace,
        "{\n  \"dockerComposeFile\": \"docker-compose.yml\",\n  \"service\": \"app\",\n  \"workspaceFolder\": \"/workspace\",\n  \"remoteUser\": \"vscode\"\n}\n",
    );
    write_executable(
        &compose_wrapper,
        format!(
            "#!/bin/sh\nexec \"{}\" compose \"$@\"\n",
            harness.fake_podman.display()
        ),
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "exec",
            "--docker-path",
            fake_podman.as_str(),
            "--docker-compose-path",
            compose_wrapper.to_string_lossy().as_ref(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "/bin/echo",
            "hello-from-podman-compose",
        ],
        &[
            ("FAKE_PODMAN_COMPOSE_PS_REJECT_SERVICE_ARGUMENT", "1"),
            (
                "FAKE_PODMAN_PS_REQUIRE_LABELS",
                "com.docker.compose.project=workspace_devcontainer\ncom.docker.compose.service=app",
            ),
            ("FAKE_PODMAN_PS_OUTPUT", "fake-compose-container-id"),
        ],
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).expect("utf8 stdout"),
        "hello-from-podman-compose\n"
    );

    let invocations = harness.read_invocations();
    assert!(!invocations.contains(" ps -q app"));
    assert!(invocations.contains(&compose_label_lookup_args(
        "workspace_devcontainer",
        "app",
        false
    )));
    assert!(invocations.contains("fake-compose-container-id /bin/echo hello-from-podman-compose"));
}

#[test]
fn up_reused_compose_service_preserves_legacy_devcontainer_id() {
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
        "{\n  \"dockerComposeFile\": \"docker-compose.yml\",\n  \"service\": \"app\",\n  \"workspaceFolder\": \"/workspace\",\n  \"postAttachCommand\": \"echo ${devcontainerId}\"\n}\n",
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
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
        ],
        &[
            ("FAKE_PODMAN_PS_OUTPUT", "existing-compose-id"),
            (
                "FAKE_PODMAN_INSPECT_FILE",
                inspect_path.to_string_lossy().as_ref(),
            ),
        ],
    );

    assert!(output.status.success(), "{output:?}");
    let payload = harness.parse_stdout_json(&output);
    assert_eq!(payload["containerId"], "existing-compose-id");
    let exec_log = harness.read_exec_log();
    assert!(exec_log.contains(&format!("/bin/sh -lc echo {legacy_id}")));
}

#[test]
fn up_expect_existing_compose_container_fails_when_missing() {
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
        "{\n  \"dockerComposeFile\": \"docker-compose.yml\",\n  \"service\": \"app\",\n  \"workspaceFolder\": \"/workspace\"\n}\n",
    );

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
        &[("FAKE_PODMAN_PS_OUTPUT", "")],
    );

    assert!(!output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stderr)
            .expect("utf8 stderr")
            .trim(),
        "Dev container not found."
    );
}

#[test]
fn up_remove_existing_compose_container_uses_engine_rm() {
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
        "{\n  \"dockerComposeFile\": \"docker-compose.yml\",\n  \"service\": \"app\",\n  \"workspaceFolder\": \"/workspace\"\n}\n",
    );

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
        &[
            ("FAKE_PODMAN_PS_OUTPUT_BEFORE_UP", "existing-compose-id"),
            (
                "FAKE_PODMAN_PS_OUTPUT_AFTER_UP",
                "fake-compose-container-id",
            ),
        ],
    );

    assert!(output.status.success(), "{output:?}");
    let payload = harness.parse_stdout_json(&output);
    assert_eq!(payload["containerId"], "fake-compose-container-id");

    let invocations = harness.read_invocations();
    assert!(invocations.contains("rm -f existing-compose-id"));
    assert!(!invocations.contains(" rm -s -f app"));
}

#[test]
fn up_resumes_stopped_compose_services_without_rerunning_create_hooks() {
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
        "{\n  \"dockerComposeFile\": \"docker-compose.yml\",\n  \"service\": \"app\",\n  \"workspaceFolder\": \"/workspace\",\n  \"onCreateCommand\": \"echo on-create\",\n  \"updateContentCommand\": \"echo update-content\",\n  \"postCreateCommand\": \"echo post-create\",\n  \"postStartCommand\": \"echo post-start\",\n  \"postAttachCommand\": \"echo post-attach\"\n}\n",
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
            ("FAKE_PODMAN_PS_OUTPUT", "stopped-compose-container-id"),
            ("FAKE_PODMAN_PS_REQUIRE_ALL", "1"),
        ],
    );

    assert!(output.status.success(), "{output:?}");
    let payload = harness.parse_stdout_json(&output);
    assert_eq!(payload["containerId"], "stopped-compose-container-id");

    let invocations = harness.read_invocations();
    assert!(invocations.contains(&compose_label_lookup_args(
        "workspace_devcontainer",
        "app",
        false
    )));
    assert!(invocations.contains(&compose_label_lookup_args(
        "workspace_devcontainer",
        "app",
        true
    )));
    assert!(invocations.contains(" up -d --no-recreate"));
    assert!(!invocations.contains(" up -d app"));

    let exec_log = harness.read_exec_log();
    assert!(!exec_log.contains("/bin/sh -lc echo on-create"));
    assert!(!exec_log.contains("/bin/sh -lc echo update-content"));
    assert!(!exec_log.contains("/bin/sh -lc echo post-create"));
    assert!(exec_log.contains("/bin/sh -lc echo post-start"));
    assert!(exec_log.contains("/bin/sh -lc echo post-attach"));
}

#[test]
fn up_reuses_existing_compose_container_with_no_recreate() {
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
        "{\n  \"dockerComposeFile\": \"docker-compose.yml\",\n  \"service\": \"app\",\n  \"workspaceFolder\": \"/workspace\"\n}\n",
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
        &[("FAKE_PODMAN_PS_OUTPUT", "fake-compose-container-id")],
    );

    assert!(output.status.success(), "{output:?}");
    let invocations = harness.read_invocations();
    assert!(invocations.contains(" up -d --no-recreate"));
}

#[test]
fn up_expect_existing_compose_container_uses_no_recreate() {
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
        "{\n  \"dockerComposeFile\": \"docker-compose.yml\",\n  \"service\": \"app\",\n  \"workspaceFolder\": \"/workspace\"\n}\n",
    );

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
        &[("FAKE_PODMAN_PS_OUTPUT", "fake-compose-container-id")],
    );

    assert!(output.status.success(), "{output:?}");
    let invocations = harness.read_invocations();
    assert!(invocations.contains(" up -d --no-recreate"));
}
