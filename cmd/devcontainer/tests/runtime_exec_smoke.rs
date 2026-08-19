//! Runtime smoke tests for exec command behavior.

mod support;

use serde_json::json;
use std::fs;

use support::runtime_harness::{write_devcontainer_config, RuntimeHarness};

#[test]
fn exec_separator_preserves_payload_options() {
    let harness = RuntimeHarness::new();
    let fake_podman = harness.fake_podman.to_string_lossy().to_string();

    let output = harness.run(
        &[
            "exec",
            "--docker-path",
            fake_podman.as_str(),
            "--container-id",
            "fake-container-id",
            "--",
            "/bin/echo",
            "--workspace-mount-consistency",
            "payload-choice",
        ],
        &[],
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).expect("utf8 stdout"),
        "--workspace-mount-consistency payload-choice\n"
    );
    assert!(
        harness
            .read_exec_argv_log()
            .contains("[/bin/echo]\n[--workspace-mount-consistency]\n[payload-choice]"),
        "{}",
        harness.read_exec_argv_log()
    );
}

#[test]
fn interactive_exec_attaches_stdin() {
    let harness = RuntimeHarness::new();
    let fake_podman = harness.fake_podman.to_string_lossy().to_string();

    let output = harness.run_with_input(
        &[
            "exec",
            "--docker-path",
            fake_podman.as_str(),
            "--container-id",
            "fake-container-id",
            "/bin/cat",
        ],
        &[("FAKE_PODMAN_REQUIRE_INTERACTIVE", "1")],
        "hello-from-stdin\n",
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).expect("utf8 stdout"),
        "hello-from-stdin\n"
    );

    let invocations = harness.read_invocations();
    assert!(invocations.contains("exec -i "));
}

#[test]
fn exec_with_container_id_uses_metadata_for_context() {
    let harness = RuntimeHarness::new();
    let inspect_path = harness.root.join("inspect.json");
    let metadata = serde_json::to_string(&json!({
        "remoteUser": "vscode",
        "remoteEnv": {
            "TEST_REMOTE_ENV": "from-metadata"
        }
    }))
    .expect("metadata");
    fs::write(
        &inspect_path,
        json!([{
            "Config": {
                "User": "container-user",
                "Labels": {
                    "devcontainer.metadata": metadata,
                    "devcontainer.local_folder": "/host/project"
                }
            },
            "Mounts": [{
                "Source": "/host/project",
                "Destination": "/container/project"
            }]
        }])
        .to_string(),
    )
    .expect("inspect json");

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "exec",
            "--docker-path",
            fake_podman.as_str(),
            "--container-id",
            "fake-container-id",
            "/bin/echo",
            "hello-from-metadata",
        ],
        &[(
            "FAKE_PODMAN_INSPECT_FILE",
            inspect_path.to_string_lossy().as_ref(),
        )],
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).expect("utf8 stdout"),
        "hello-from-metadata\n"
    );
    let invocations = harness.read_invocations();
    assert!(invocations.contains("inspect fake-container-id"));
    assert!(invocations.contains("exec -i --workdir /container/project --user vscode"));
    assert!(invocations.contains("-e TEST_REMOTE_ENV=from-metadata"));
    assert!(invocations.contains("-e HOME=/home/vscode"));
    assert!(invocations.contains("fake-container-id /bin/echo hello-from-metadata"));
}

#[test]
fn up_persists_metadata_for_followup_exec_with_container_id() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(&workspace).expect("workspace dir");
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"alpine:3.20\",\n  \"workspaceMount\": \"type=bind,source=/host/project,target=/persisted-workspace\",\n  \"remoteUser\": \"vscode\",\n  \"remoteEnv\": {\n    \"TEST_REMOTE_ENV\": \"from-config\"\n  }\n}\n",
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
        &[("FAKE_PODMAN_PS_DISABLE_DEFAULT", "1")],
    );

    assert!(up_output.status.success(), "{up_output:?}");
    let exec_output = harness.run(
        &[
            "exec",
            "--docker-path",
            fake_podman.as_str(),
            "--container-id",
            "fake-container-id",
            "/bin/echo",
            "hello-from-persisted-metadata",
        ],
        &[],
    );

    assert!(exec_output.status.success(), "{exec_output:?}");
    assert_eq!(
        String::from_utf8(exec_output.stdout).expect("utf8 stdout"),
        "hello-from-persisted-metadata\n"
    );
    let invocations = harness.read_invocations();
    assert!(invocations.contains("--label devcontainer.metadata="));
    assert!(invocations.contains("inspect fake-container-id"));
    assert!(invocations.contains("exec -i --workdir /persisted-workspace --user vscode"));
    assert!(invocations.contains("-e TEST_REMOTE_ENV=from-config"));
    assert!(invocations.contains("-e HOME=/home/vscode"));
    assert!(invocations.contains("fake-container-id /bin/echo hello-from-persisted-metadata"));
}

#[test]
fn compose_up_persists_metadata_for_followup_exec_with_container_id() {
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
        "{\n  \"dockerComposeFile\": \"docker-compose.yml\",\n  \"service\": \"app\",\n  \"workspaceFolder\": \"/persisted-compose-workspace\",\n  \"remoteUser\": \"vscode\",\n  \"remoteEnv\": {\n    \"TEST_REMOTE_ENV\": \"from-compose-config\"\n  }\n}\n",
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
    let exec_output = harness.run(
        &[
            "exec",
            "--docker-path",
            fake_podman.as_str(),
            "--container-id",
            "fake-compose-container-id",
            "/bin/echo",
            "hello-from-compose-metadata",
        ],
        &[],
    );

    assert!(exec_output.status.success(), "{exec_output:?}");
    assert_eq!(
        String::from_utf8(exec_output.stdout).expect("utf8 stdout"),
        "hello-from-compose-metadata\n"
    );

    let invocations = harness.read_invocations();
    assert!(invocations.contains("inspect fake-compose-container-id"));
    assert!(invocations.contains("compose --project-name workspace_devcontainer -f "));
    assert!(invocations.contains("exec -i --workdir /persisted-compose-workspace --user vscode"));
    assert!(invocations.contains("-e TEST_REMOTE_ENV=from-compose-config"));
    assert!(invocations.contains("-e HOME=/home/vscode"));
    assert!(invocations.contains("fake-compose-container-id /bin/echo hello-from-compose-metadata"));
}

#[test]
fn up_can_omit_config_remote_env_from_persisted_metadata() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(&workspace).expect("workspace dir");
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"alpine:3.20\",\n  \"workspaceMount\": \"type=bind,source=/host/project,target=/persisted-workspace\",\n  \"remoteUser\": \"vscode\",\n  \"remoteEnv\": {\n    \"TEST_REMOTE_ENV\": \"from-config\"\n  }\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let up_output = harness.run(
        &[
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--omit-config-remote-env-from-metadata",
        ],
        &[("FAKE_PODMAN_PS_DISABLE_DEFAULT", "1")],
    );

    assert!(up_output.status.success(), "{up_output:?}");
    let exec_output = harness.run(
        &[
            "exec",
            "--docker-path",
            fake_podman.as_str(),
            "--container-id",
            "fake-container-id",
            "/bin/sh",
            "-lc",
            "printf %s \"${TEST_REMOTE_ENV-}\"",
        ],
        &[],
    );

    assert!(exec_output.status.success(), "{exec_output:?}");
    assert_eq!(
        String::from_utf8(exec_output.stdout).expect("utf8 stdout"),
        ""
    );
    let invocations = harness.read_invocations();
    assert!(invocations.contains("--label devcontainer.metadata="));
    assert!(!invocations.contains("TEST_REMOTE_ENV=from-config"));
    assert!(invocations.contains("exec -i --workdir /persisted-workspace --user vscode"));
    assert!(invocations.contains("-e HOME=/home/vscode"));
    assert!(invocations.contains("fake-container-id /bin/sh -lc printf %s \"${TEST_REMOTE_ENV-}\""));
}

#[test]
fn exec_injects_secrets_file_environment() {
    let harness = RuntimeHarness::new();
    let inspect_path = harness.root.join("inspect.json");
    let secrets_path = harness.root.join("secrets.json");
    fs::write(
        &secrets_path,
        "{\n  \"SECRET_TOKEN\": \"from-secret-file\"\n}\n",
    )
    .expect("secrets file");
    fs::write(
        &inspect_path,
        json!([{
            "Config": {
                "Labels": {
                    "devcontainer.metadata": "{}",
                    "devcontainer.local_folder": "/host/project"
                }
            },
            "Mounts": [{
                "Source": "/host/project",
                "Destination": "/container/project"
            }]
        }])
        .to_string(),
    )
    .expect("inspect json");

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "exec",
            "--docker-path",
            fake_podman.as_str(),
            "--container-id",
            "fake-container-id",
            "--secrets-file",
            secrets_path.to_string_lossy().as_ref(),
            "/bin/sh",
            "-lc",
            "printf %s \"$SECRET_TOKEN\"",
        ],
        &[(
            "FAKE_PODMAN_INSPECT_FILE",
            inspect_path.to_string_lossy().as_ref(),
        )],
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).expect("utf8 stdout"),
        "from-secret-file"
    );
    let invocations = harness.read_invocations();
    assert!(invocations.contains("-e SECRET_TOKEN=from-secret-file"));
}

#[test]
fn exec_derives_home_from_passwd_when_container_home_is_unwritable() {
    let harness = RuntimeHarness::new();
    let inspect_path = harness.root.join("inspect.json");
    let metadata = serde_json::to_string(&json!({
        "remoteUser": "vscode",
        "remoteEnv": {
            "TEST_REMOTE_ENV": "from-metadata"
        }
    }))
    .expect("metadata");
    fs::write(
        &inspect_path,
        json!([{
            "Config": {
                "Env": ["HOME=/root"],
                "Labels": {
                    "devcontainer.metadata": metadata,
                    "devcontainer.local_folder": "/host/project"
                }
            },
            "Mounts": [{
                "Source": "/host/project",
                "Destination": "/container/project"
            }]
        }])
        .to_string(),
    )
    .expect("inspect json");

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "exec",
            "--docker-path",
            fake_podman.as_str(),
            "--container-id",
            "fake-container-id",
            "/bin/sh",
            "-lc",
            "printf %s \"$HOME\"",
        ],
        &[(
            "FAKE_PODMAN_INSPECT_FILE",
            inspect_path.to_string_lossy().as_ref(),
        )],
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).expect("utf8 stdout"),
        "/home/vscode"
    );
    let invocations = harness.read_invocations();
    assert!(invocations.contains("-e HOME=/home/vscode"));
}

#[test]
fn exec_derives_home_from_container_user_when_devcontainer_user_is_unset() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(&workspace).expect("workspace dir");
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"alpine:3.20\",\n  \"workspaceFolder\": \"/container/project\"\n}\n",
    );
    let inspect_path = harness.root.join("inspect.json");
    fs::write(
        &inspect_path,
        json!([{
            "Config": {
                "User": "vscode",
                "Env": ["HOME=/root"]
            },
            "Mounts": [{
                "Source": "/host/project",
                "Destination": "/container/project"
            }]
        }])
        .to_string(),
    )
    .expect("inspect json");

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "exec",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--container-id",
            "fake-container-id",
            "/bin/sh",
            "-lc",
            "printf %s \"$HOME\"",
        ],
        &[(
            "FAKE_PODMAN_INSPECT_FILE",
            inspect_path.to_string_lossy().as_ref(),
        )],
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).expect("utf8 stdout"),
        "/home/vscode"
    );
    let invocations = harness.read_invocations();
    assert!(invocations.contains("exec -i --workdir /container/project -e HOME=/home/vscode"));
    assert!(!invocations.contains("--user vscode"));
}

#[test]
fn exec_keeps_explicit_remote_env_home() {
    let harness = RuntimeHarness::new();
    let inspect_path = harness.root.join("inspect.json");
    let metadata = serde_json::to_string(&json!({
        "remoteUser": "vscode",
        "remoteEnv": {
            "HOME": "/custom-home"
        }
    }))
    .expect("metadata");
    fs::write(
        &inspect_path,
        json!([{
            "Config": {
                "Env": ["HOME=/root"],
                "Labels": {
                    "devcontainer.metadata": metadata,
                    "devcontainer.local_folder": "/host/project"
                }
            },
            "Mounts": [{
                "Source": "/host/project",
                "Destination": "/container/project"
            }]
        }])
        .to_string(),
    )
    .expect("inspect json");

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "exec",
            "--docker-path",
            fake_podman.as_str(),
            "--container-id",
            "fake-container-id",
            "/bin/sh",
            "-lc",
            "printf %s \"$HOME\"",
        ],
        &[(
            "FAKE_PODMAN_INSPECT_FILE",
            inspect_path.to_string_lossy().as_ref(),
        )],
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).expect("utf8 stdout"),
        "/custom-home"
    );
    let invocations = harness.read_invocations();
    assert!(invocations.contains("-e HOME=/custom-home"));
    assert!(!invocations.contains("-e HOME=/home/vscode"));
}
