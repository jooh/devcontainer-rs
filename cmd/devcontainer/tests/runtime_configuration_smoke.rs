//! Runtime smoke tests for configuration reporting behavior.

mod support;

use std::fs;
use std::path::Path;

use support::runtime_harness::{write_devcontainer_config, RuntimeHarness};

#[test]
fn read_configuration_uses_environment_config_path() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    let default_config = workspace.join(".devcontainer").join("devcontainer.json");
    let env_config = workspace
        .join(".devcontainer")
        .join("podman")
        .join("devcontainer.json");
    fs::create_dir_all(default_config.parent().expect("default config parent"))
        .expect("default config dir");
    fs::create_dir_all(env_config.parent().expect("env config parent")).expect("env config dir");
    fs::write(&default_config, r#"{"image":"default"}"#).expect("default config");
    fs::write(&env_config, r#"{"image":"environment"}"#).expect("environment config");

    let output = harness.run(
        &[
            "read-configuration",
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
        ],
        &[(
            "DEVCONTAINER_CONFIG",
            ".devcontainer/podman/devcontainer.json",
        )],
    );

    assert!(output.status.success(), "{output:?}");
    let payload = harness.parse_stdout_json(&output);
    assert_eq!(payload["configuration"]["image"], "environment");
    assert_eq!(
        payload["configuration"]["configFilePath"],
        fs::canonicalize(&env_config)
            .expect("canonical environment config")
            .display()
            .to_string()
    );
}

#[test]
fn read_configuration_with_container_id_merges_config_and_container_metadata() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(&workspace).expect("workspace dir");
    let config_path = write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"alpine:3.20\",\n  \"postAttachCommand\": \"touch /postAttachCommand.txt\",\n  \"remoteEnv\": {\n    \"TEST_RE\": \"${containerEnv:TEST_CE}\"\n  }\n}\n",
    );

    let inspect_path = harness.root.join("inspect.json");
    fs::write(
        &inspect_path,
        r#"[{
  "Config": {
    "Labels": {
      "devcontainer.metadata": "{ \"postCreateCommand\": \"touch /postCreateCommand.txt\", \"remoteEnv\": { \"FROM_METADATA\": \"yes\" } }"
    },
    "Env": [
      "PATH=/usr/local/bin:/usr/bin",
      "TEST_CE=from-container"
    ]
  },
  "Mounts": [{
    "Source": "/workspace",
    "Destination": "/workspaces/workspace"
  }]
}]"#,
    )
    .expect("inspect file");

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "read-configuration",
            "--docker-path",
            fake_podman.as_str(),
            "--container-id",
            "fake-existing-container",
            "--config",
            config_path.to_string_lossy().as_ref(),
            "--include-merged-configuration",
        ],
        &[(
            "FAKE_PODMAN_INSPECT_FILE",
            inspect_path.to_string_lossy().as_ref(),
        )],
    );

    assert!(output.status.success(), "{output:?}");
    let payload = harness.parse_stdout_json(&output);
    assert_eq!(
        payload["configuration"]["remoteEnv"]["TEST_RE"],
        "from-container"
    );
    assert_eq!(
        payload["mergedConfiguration"]["postAttachCommands"]
            .as_array()
            .expect("post attach commands")
            .len(),
        1
    );
    assert_eq!(
        payload["mergedConfiguration"]["postCreateCommands"]
            .as_array()
            .expect("post create commands")
            .len(),
        1
    );
    assert_eq!(
        payload["mergedConfiguration"]["remoteEnv"]["FROM_METADATA"],
        "yes"
    );
}

#[test]
fn read_configuration_with_container_id_uses_container_metadata_without_config() {
    let harness = RuntimeHarness::new();
    let inspect_path = harness.root.join("inspect.json");
    fs::write(
        &inspect_path,
        r#"[{
  "Config": {
    "Labels": {
      "devcontainer.local_folder": "/tmp/workspace",
      "devcontainer.metadata": "{ \"postCreateCommand\": \"touch /postCreateCommand.txt\", \"workspaceFolder\": \"/workspace/from-metadata\" }"
    },
    "Env": [
      "PATH=/usr/local/bin:/usr/bin"
    ]
  },
  "Mounts": [{
    "Source": "/tmp/workspace",
    "Destination": "/workspace/from-metadata"
  }]
}]"#,
    )
    .expect("inspect file");

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "read-configuration",
            "--docker-path",
            fake_podman.as_str(),
            "--container-id",
            "fake-existing-container",
            "--include-merged-configuration",
        ],
        &[(
            "FAKE_PODMAN_INSPECT_FILE",
            inspect_path.to_string_lossy().as_ref(),
        )],
    );

    assert!(output.status.success(), "{output:?}");
    let payload = harness.parse_stdout_json(&output);
    assert_eq!(payload["configuration"], serde_json::json!({}));
    assert_eq!(
        payload["mergedConfiguration"]["postCreateCommands"]
            .as_array()
            .expect("post create commands")
            .len(),
        1
    );
    assert!(payload.get("workspace").is_none());
    assert_eq!(
        payload["mergedConfiguration"]["workspaceFolder"],
        "/workspace/from-metadata"
    );
}

#[test]
fn read_configuration_with_container_id_reports_inspect_failures() {
    let harness = RuntimeHarness::new();
    let failing_engine = harness.root.join("failing-podman");
    write_executable_script(
        &failing_engine,
        "#!/bin/sh\nprintf 'inspect failed for %s\\n' \"$*\" >&2\nexit 7\n",
    );

    let fake_podman = failing_engine.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "read-configuration",
            "--docker-path",
            fake_podman.as_str(),
            "--container-id",
            "fake-existing-container",
        ],
        &[],
    );

    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(stderr.contains("inspect failed"), "{stderr}");
}

#[test]
fn read_configuration_with_container_id_keeps_metadata_without_local_folder() {
    let harness = RuntimeHarness::new();
    let inspect_path = harness.root.join("inspect.json");
    fs::write(
        &inspect_path,
        r#"[{
  "Config": {
    "Labels": {
      "devcontainer.metadata": "{ \"remoteEnv\": { \"LOCAL_TOKEN\": \"${localWorkspaceFolder}\" }, \"postCreateCommand\": \"echo metadata\" }"
    },
    "Env": []
  },
  "Mounts": []
}]"#,
    )
    .expect("inspect file");

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "read-configuration",
            "--docker-path",
            fake_podman.as_str(),
            "--container-id",
            "fake-existing-container",
            "--include-merged-configuration",
        ],
        &[(
            "FAKE_PODMAN_INSPECT_FILE",
            inspect_path.to_string_lossy().as_ref(),
        )],
    );

    assert!(output.status.success(), "{output:?}");
    let payload = harness.parse_stdout_json(&output);
    assert_eq!(
        payload["mergedConfiguration"]["remoteEnv"]["LOCAL_TOKEN"],
        "${localWorkspaceFolder}"
    );
    assert_eq!(
        payload["mergedConfiguration"]["postCreateCommands"]
            .as_array()
            .expect("post create commands")
            .len(),
        1
    );
    assert!(payload.get("workspace").is_none());
}

fn write_executable_script(path: &Path, body: &str) {
    fs::write(path, body).expect("script");
    let mut permissions = fs::metadata(path).expect("script metadata").permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o755);
    }
    fs::set_permissions(path, permissions).expect("script permissions");
}
