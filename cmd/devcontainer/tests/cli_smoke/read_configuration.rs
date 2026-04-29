//! CLI smoke tests for read-configuration flows.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::support::test_support::{devcontainer_command, repo_root, unique_temp_dir};

fn run_read_configuration(
    args: &[&str],
    current_dir: Option<&std::path::Path>,
) -> (std::process::Output, Value) {
    let mut command = devcontainer_command(current_dir);
    command.arg("read-configuration").args(args);

    let output = command.output().expect("read-configuration should run");
    let stdout = String::from_utf8(output.stdout.clone()).expect("utf8 stdout");
    let parsed = serde_json::from_str(&stdout).expect("json stdout");
    (output, parsed)
}

fn default_user_data_folder(root: &Path) -> PathBuf {
    if cfg!(target_os = "linux") {
        let username = env::var("USER").unwrap_or_else(|_| "unknown".to_string());
        root.join(format!("devcontainercli-{username}"))
    } else {
        root.join("devcontainercli")
    }
}

#[test]
fn top_level_help_lists_supported_commands() {
    let output = devcontainer_command(None)
        .arg("--help")
        .output()
        .expect("help command should run");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(stdout.contains("read-configuration"));
    assert!(stdout.contains("templates"));
    assert!(stdout.contains("Create and run dev container"));
    assert!(stdout.contains("devcontainer <command>"));
    assert!(!stdout.contains("docs/upstream/command-reference.md"));
}

#[test]
fn read_configuration_command_returns_configuration_payload() {
    let root = unique_temp_dir("devcontainer-cli-smoke");
    let config_dir = root.join(".devcontainer");
    fs::create_dir_all(&config_dir).expect("config dir");
    fs::write(
        config_dir.join("devcontainer.json"),
        "{\n  \"image\": \"debian:bookworm\"\n}\n",
    )
    .expect("config write");

    let output = devcontainer_command(None)
        .args([
            "read-configuration",
            "--workspace-folder",
            root.to_string_lossy().as_ref(),
        ])
        .output()
        .expect("read-configuration should run");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(stdout.contains("\"configuration\""));
    assert!(stdout.contains("\"workspace\""));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn read_configuration_loads_default_control_manifest_without_user_data_flag() {
    let root = unique_temp_dir("devcontainer-cli-smoke");
    let temp_root = unique_temp_dir("devcontainer-cli-smoke");
    let config_dir = root.join(".devcontainer");
    let user_data = default_user_data_folder(&temp_root);
    fs::create_dir_all(&config_dir).expect("config dir");
    fs::create_dir_all(&user_data).expect("user data dir");
    fs::write(
        config_dir.join("devcontainer.json"),
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"ghcr.io/devcontainers/features/problematic-feature:1\": {}\n  }\n}\n",
    )
    .expect("config write");
    fs::write(
        user_data.join("control-manifest.json"),
        serde_json::json!({
            "disallowedFeatures": [{
                "featureIdPrefix": "ghcr.io/devcontainers/features/problematic-feature",
                "documentationURL": "https://containers.dev/"
            }],
            "featureAdvisories": []
        })
        .to_string(),
    )
    .expect("control manifest");

    let output = devcontainer_command(None)
        .env("TMPDIR", &temp_root)
        .args([
            "read-configuration",
            "--workspace-folder",
            root.to_string_lossy().as_ref(),
            "--include-features-configuration",
        ])
        .output()
        .expect("read-configuration should run");

    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(stderr.contains("problematic-feature:1"), "{stderr}");

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(temp_root);
}

#[test]
fn read_configuration_supports_upstream_subfolder_config() {
    let root = repo_root();
    let workspace = root
        .join("upstream")
        .join("src")
        .join("test")
        .join("configs")
        .join("dockerfile-without-features");
    let config = workspace
        .join(".devcontainer")
        .join("subfolder")
        .join("devcontainer.json");

    let (output, payload) = run_read_configuration(
        &[
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--config",
            config.to_string_lossy().as_ref(),
        ],
        None,
    );

    assert!(output.status.success());
    assert_eq!(
        payload["configuration"]["remoteEnv"]["SUBFOLDER_CONFIG_REMOTE_ENV"],
        "true"
    );
    assert_eq!(
        payload["configuration"]["configFilePath"],
        config
            .canonicalize()
            .expect("canonical config")
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(
        payload["workspace"]["workspaceFolder"],
        "/workspaces/upstream/src/test/configs/dockerfile-without-features"
    );
}

#[test]
fn read_configuration_uses_current_directory_with_upstream_fixture() {
    let root = repo_root();
    let workspace = root
        .join("upstream")
        .join("src")
        .join("test")
        .join("configs")
        .join("image");

    let (output, payload) = run_read_configuration(&[], Some(&workspace));

    assert!(output.status.success());
    assert_eq!(payload["configuration"]["image"], "ubuntu:latest");
    assert_eq!(
        payload["configuration"]["remoteEnv"]["CONTAINER_PATH"],
        "${containerEnv:PATH}"
    );
    let local_path = payload["configuration"]["remoteEnv"]["LOCAL_PATH"]
        .as_str()
        .expect("local path");
    let expected_local_path = std::env::var_os("PATH").expect("PATH env set");
    assert_eq!(local_path, expected_local_path.to_string_lossy().as_ref());
}

#[test]
fn read_configuration_applies_upstream_style_local_substitution_defaults() {
    let root = repo_root();
    let workspace = root
        .join("src")
        .join("test")
        .join("parity")
        .join("fixtures")
        .join("config")
        .join("upstream-style-substitution");
    let config = workspace.join("devcontainer.json");

    let (output, payload) = run_read_configuration(
        &[
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--config",
            config.to_string_lossy().as_ref(),
        ],
        None,
    );

    assert!(output.status.success());
    assert_eq!(
        payload["configuration"]["containerEnv"]["WORKSPACE_BASENAME"],
        "upstream-style-substitution"
    );
    assert_eq!(
        payload["configuration"]["containerEnv"]["DEFAULTED_ENV"],
        "fallback-value"
    );
    assert_eq!(
        payload["configuration"]["containerEnv"]["DEFAULT_WITH_COLONS"],
        "fallback"
    );
    assert_eq!(
        payload["configuration"]["containerEnv"]["MISSING_WITHOUT_DEFAULT"],
        "before--after"
    );
}

#[test]
fn read_configuration_merged_output_uses_upstream_pluralized_fields() {
    let root = unique_temp_dir("devcontainer-cli-smoke");
    let config_dir = root.join(".devcontainer");
    fs::create_dir_all(&config_dir).expect("config dir");
    fs::write(
        config_dir.join("devcontainer.json"),
        "{\n  \"image\": \"debian:bookworm\",\n  \"postAttachCommand\": \"echo attached\",\n  \"remoteUser\": \"vscode\"\n}\n",
    )
    .expect("config write");

    let (_, payload) = run_read_configuration(
        &[
            "--workspace-folder",
            root.to_string_lossy().as_ref(),
            "--include-merged-configuration",
        ],
        None,
    );

    assert_eq!(
        payload["configuration"]["postAttachCommand"],
        "echo attached"
    );
    assert_eq!(
        payload["mergedConfiguration"]["postAttachCommands"]
            .as_array()
            .expect("post attach commands")
            .len(),
        1
    );
    assert!(payload["mergedConfiguration"]
        .get("postAttachCommand")
        .is_none());
    assert_eq!(payload["mergedConfiguration"]["remoteUser"], "vscode");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn read_configuration_supports_override_config_without_workspace_devcontainer_file() {
    let root = unique_temp_dir("devcontainer-cli-smoke");
    let config_dir = root.join(".devcontainer");
    let override_path = config_dir.join("override.json");
    fs::create_dir_all(&config_dir).expect("config dir");
    fs::write(
        &override_path,
        "{\n  \"image\": \"alpine:3.20\",\n  \"workspaceFolder\": \"/override-workspace\"\n}\n",
    )
    .expect("override config write");

    let (output, payload) = run_read_configuration(
        &[
            "--override-config",
            override_path.to_string_lossy().as_ref(),
        ],
        None,
    );

    assert!(output.status.success());
    assert_eq!(payload["configuration"]["image"], "alpine:3.20");
    let expected_config = root
        .canonicalize()
        .expect("canonical workspace")
        .join(".devcontainer")
        .join("devcontainer.json");
    assert_eq!(
        payload["configuration"]["configFilePath"],
        expected_config.display().to_string()
    );
    assert_eq!(
        payload["workspace"]["workspaceFolder"],
        "/override-workspace"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn read_configuration_uses_git_root_mount_for_nested_workspaces() {
    let root = unique_temp_dir("devcontainer-cli-smoke");
    let repo_root = root.join("repo");
    let workspace = repo_root.join("packages").join("app");
    fs::create_dir_all(&workspace).expect("workspace dir");
    init_git_repo(&repo_root);
    let expected_repo_root = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.clone());
    fs::create_dir_all(workspace.join(".devcontainer")).expect("config dir");
    fs::write(
        workspace.join(".devcontainer").join("devcontainer.json"),
        "{\n  \"image\": \"alpine:3.20\"\n}\n",
    )
    .expect("config write");

    let (output, payload) = run_read_configuration(
        &["--workspace-folder", workspace.to_string_lossy().as_ref()],
        None,
    );

    assert!(output.status.success());
    let workspace_mount = payload["workspace"]["workspaceMount"]
        .as_str()
        .expect("workspace mount");
    assert!(workspace_mount.contains(&format!("source={}", expected_repo_root.display())));
    assert!(workspace_mount.contains("target=/workspaces/repo"));
    assert_eq!(
        payload["workspace"]["workspaceFolder"],
        "/workspaces/repo/packages/app"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn read_configuration_preserves_git_root_mount_for_nested_workspace_folder_targets() {
    let root = unique_temp_dir("devcontainer-cli-smoke");
    let repo_root = root.join("repo");
    let workspace = repo_root.join("packages").join("app");
    fs::create_dir_all(&workspace).expect("workspace dir");
    init_git_repo(&repo_root);
    let expected_repo_root = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.clone());
    fs::create_dir_all(workspace.join(".devcontainer")).expect("config dir");
    fs::write(
        workspace.join(".devcontainer").join("devcontainer.json"),
        "{\n  \"image\": \"alpine:3.20\",\n  \"workspaceFolder\": \"/workspace\"\n}\n",
    )
    .expect("config write");

    let (output, payload) = run_read_configuration(
        &["--workspace-folder", workspace.to_string_lossy().as_ref()],
        None,
    );

    assert!(output.status.success());
    let workspace_mount = payload["workspace"]["workspaceMount"]
        .as_str()
        .expect("workspace mount");
    let options = split_mount_options(workspace_mount);
    assert!(options.contains(&format!("source={}", expected_repo_root.display())));
    assert!(!options.contains(&format!("source={}", workspace.display())));
    assert!(options.contains(&"target=/workspace".to_string()));
    assert_eq!(payload["workspace"]["workspaceFolder"], "/workspace");

    let _ = fs::remove_dir_all(root);
}

fn init_git_repo(root: &std::path::Path) {
    let status = Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(root)
        .status()
        .expect("git init");
    assert!(status.success(), "git init failed: {status:?}");
}

fn split_mount_options(mount: &str) -> Vec<String> {
    let mut options = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for character in mount.chars() {
        match character {
            '"' => {
                in_quotes = !in_quotes;
                current.push(character);
            }
            ',' if !in_quotes => {
                options.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(character),
        }
    }
    if !current.is_empty() {
        options.push(current.trim().to_string());
    }
    options
}
