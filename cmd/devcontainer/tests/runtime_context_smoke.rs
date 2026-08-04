//! Runtime smoke tests for context resolution and inspected-container behavior.

mod support;

use std::fs;

use support::runtime_harness::{write_devcontainer_config, RuntimeHarness};

#[test]
fn exec_with_config_uses_the_config_workspace_for_lookup_and_workdir() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    let caller_dir = harness.root.join("caller");
    fs::create_dir_all(&workspace).expect("workspace dir");
    fs::create_dir_all(&caller_dir).expect("caller dir");
    let config_path = write_devcontainer_config(&workspace, "{\n  \"image\": \"alpine:3.20\"\n}\n");
    let expected_workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.clone());

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let required_label = format!("devcontainer.local_folder={}", expected_workspace.display());
    let output = harness.run_in_dir(
        &[
            "exec",
            "--docker-path",
            fake_podman.as_str(),
            "--config",
            config_path.to_string_lossy().as_ref(),
            "/bin/echo",
            "hello-from-config",
        ],
        &[("FAKE_PODMAN_PS_REQUIRE_LABEL", required_label.as_str())],
        Some(&caller_dir),
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).expect("utf8 stdout"),
        "hello-from-config\n"
    );

    let invocations = harness.read_invocations();
    assert!(invocations.contains(&format!(
        "ps -q --filter label=devcontainer.local_folder={}",
        expected_workspace.display()
    )));
    assert!(invocations.contains("exec -i --workdir /workspaces/workspace"));
    assert!(invocations.contains("-e HOME=/root"));
    assert!(invocations.contains("fake-container-id /bin/echo hello-from-config"));
}

#[test]
fn nested_config_exec_uses_workspace_root_and_config_label() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    let caller_dir = harness.root.join("caller");
    let nested_config_dir = workspace.join(".devcontainer").join("python");
    fs::create_dir_all(&nested_config_dir).expect("nested config dir");
    fs::create_dir_all(&caller_dir).expect("caller dir");
    let config_path = nested_config_dir.join("devcontainer.json");
    fs::write(&config_path, "{\n  \"image\": \"alpine:3.20\"\n}\n").expect("config write");
    let expected_workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.clone());
    let expected_config = config_path
        .canonicalize()
        .unwrap_or_else(|_| config_path.clone());

    let required_labels = format!(
        "devcontainer.local_folder={}\ndevcontainer.config_file={}",
        expected_workspace.display(),
        expected_config.display()
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run_in_dir(
        &[
            "exec",
            "--docker-path",
            fake_podman.as_str(),
            "--config",
            config_path.to_string_lossy().as_ref(),
            "/bin/echo",
            "hello-from-nested-config",
        ],
        &[("FAKE_PODMAN_PS_REQUIRE_LABELS", required_labels.as_str())],
        Some(&caller_dir),
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).expect("utf8 stdout"),
        "hello-from-nested-config\n"
    );

    let invocations = harness.read_invocations();
    assert!(invocations.contains(&format!(
        "ps -q --filter label=devcontainer.local_folder={}",
        expected_workspace.display()
    )));
    assert!(invocations.contains(&format!(
        "--filter label=devcontainer.config_file={}",
        expected_config.display()
    )));
    assert!(invocations.contains("exec -i --workdir /workspaces/workspace"));
    assert!(invocations.contains("-e HOME=/root"));
    assert!(invocations.contains("fake-container-id /bin/echo hello-from-nested-config"));
}

#[test]
fn environment_config_exec_uses_nested_config_identity() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    let nested_config_dir = workspace.join(".devcontainer").join("podman");
    fs::create_dir_all(&nested_config_dir).expect("nested config dir");
    let config_path = nested_config_dir.join("devcontainer.json");
    fs::write(
        &config_path,
        "{\n  \"image\": \"alpine:3.20\",\n  \"workspaceFolder\": \"/environment-workspace\"\n}\n",
    )
    .expect("config write");
    let expected_workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.clone());
    let expected_config = config_path
        .canonicalize()
        .unwrap_or_else(|_| config_path.clone());
    let required_labels = format!(
        "devcontainer.local_folder={}\ndevcontainer.config_file={}",
        expected_workspace.display(),
        expected_config.display()
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run_in_dir(
        &[
            "exec",
            "--docker-path",
            fake_podman.as_str(),
            "/bin/echo",
            "hello-from-environment-config",
        ],
        &[
            (
                "DEVCONTAINER_CONFIG",
                ".devcontainer/podman/devcontainer.json",
            ),
            ("FAKE_PODMAN_PS_REQUIRE_LABELS", required_labels.as_str()),
        ],
        Some(&workspace),
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).expect("utf8 stdout"),
        "hello-from-environment-config\n"
    );
    let invocations = harness.read_invocations();
    assert!(invocations.contains(&format!(
        "--filter label=devcontainer.config_file={}",
        expected_config.display()
    )));
    assert!(invocations.contains("exec -i --workdir /environment-workspace"));
}

#[test]
fn exec_from_workspace_directory_loads_local_config() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(&workspace).expect("workspace dir");
    let config_path = write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"alpine:3.20\",\n  \"workspaceFolder\": \"/configured-workspace\",\n  \"remoteUser\": \"vscode\",\n  \"remoteEnv\": {\n    \"TEST_REMOTE_ENV\": \"from-config\"\n  }\n}\n",
    );
    let expected_workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.clone());
    let expected_config = config_path
        .canonicalize()
        .unwrap_or_else(|_| config_path.clone());

    let required_labels = format!(
        "devcontainer.local_folder={}\ndevcontainer.config_file={}",
        expected_workspace.display(),
        expected_config.display()
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run_in_dir(
        &[
            "exec",
            "--docker-path",
            fake_podman.as_str(),
            "/bin/echo",
            "hello-from-workspace",
        ],
        &[("FAKE_PODMAN_PS_REQUIRE_LABELS", required_labels.as_str())],
        Some(&workspace),
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).expect("utf8 stdout"),
        "hello-from-workspace\n"
    );

    let invocations = harness.read_invocations();
    assert!(invocations.contains(&format!(
        "ps -q --filter label=devcontainer.local_folder={}",
        expected_workspace.display()
    )));
    assert!(invocations.contains(&format!(
        "--filter label=devcontainer.config_file={}",
        expected_config.display()
    )));
    assert!(invocations.contains("exec -i --workdir /configured-workspace --user vscode"));
    assert!(invocations.contains("-e TEST_REMOTE_ENV=from-config"));
    assert!(invocations.contains("-e HOME=/home/vscode"));
    assert!(invocations.contains("fake-container-id /bin/echo hello-from-workspace"));
}

#[test]
fn exec_with_override_config_uses_override_contents_and_workspace_config_labels() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    let caller_dir = harness.root.join("caller");
    let override_dir = workspace.join(".devcontainer");
    let override_path = override_dir.join("override.json");
    fs::create_dir_all(&override_dir).expect("override dir");
    fs::create_dir_all(&caller_dir).expect("caller dir");
    fs::write(
        &override_path,
        "{\n  \"image\": \"alpine:3.20\",\n  \"workspaceFolder\": \"/override-workspace\",\n  \"remoteUser\": \"vscode\"\n}\n",
    )
    .expect("override write");
    let expected_workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.clone());
    let expected_config = expected_workspace
        .join(".devcontainer")
        .join("devcontainer.json");

    let required_labels = format!(
        "devcontainer.local_folder={}\ndevcontainer.config_file={}",
        expected_workspace.display(),
        expected_config.display()
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run_in_dir(
        &[
            "exec",
            "--docker-path",
            fake_podman.as_str(),
            "--override-config",
            override_path.to_string_lossy().as_ref(),
            "/bin/echo",
            "hello-from-override",
        ],
        &[("FAKE_PODMAN_PS_REQUIRE_LABELS", required_labels.as_str())],
        Some(&caller_dir),
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).expect("utf8 stdout"),
        "hello-from-override\n"
    );

    let invocations = harness.read_invocations();
    assert!(invocations.contains(&format!(
        "--filter label=devcontainer.config_file={}",
        expected_config.display()
    )));
    assert!(invocations.contains("exec -i --workdir /override-workspace --user vscode"));
    assert!(invocations.contains("-e HOME=/home/vscode"));
    assert!(invocations.contains("fake-container-id /bin/echo hello-from-override"));
}
