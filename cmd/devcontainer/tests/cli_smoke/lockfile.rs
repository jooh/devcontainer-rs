//! CLI smoke tests for lockfile and upgrade commands.

use std::fs;
use std::path::Path;
use std::process::Output;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::support::test_support::{
    copy_recursive, devcontainer_command, repo_root, unique_temp_dir,
};

fn copied_lockfile_fixture(name: &str) -> std::path::PathBuf {
    let fixture = repo_root()
        .join("upstream")
        .join("src")
        .join("test")
        .join("container-features")
        .join("configs")
        .join(name);
    let workspace = unique_temp_dir("devcontainer-cli-smoke");
    copy_recursive(&fixture, &workspace);
    workspace
}

fn utf8_stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("utf8 stderr")
}

fn stderr_json_logs(output: &Output) -> Vec<Value> {
    utf8_stderr(output)
        .lines()
        .map(|line| serde_json::from_str(line).expect("json log line"))
        .collect()
}

#[test]
fn outdated_supports_upstream_json_output_fixture() {
    let workspace = copied_lockfile_fixture("lockfile-outdated-command");

    let output = devcontainer_command(None)
        .args([
            "outdated",
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--output-format",
            "json",
        ])
        .output()
        .expect("outdated should run");

    assert!(output.status.success(), "{output:?}");
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json payload");
    assert_eq!(
        payload["features"]["ghcr.io/devcontainers/features/git:1.0"]["current"],
        "1.0.4"
    );
    assert_eq!(
        payload["features"]["ghcr.io/devcontainers/features/git:1.0"]["wanted"],
        "1.0.5"
    );
    assert_eq!(
        payload["features"]["ghcr.io/devcontainers/features/git:1.0"]["latest"],
        "1.2.0"
    );
    assert_eq!(
        payload["features"]["ghcr.io/codspace/versioning/foo:0.3.1"]["latest"],
        "2.11.1"
    );
    assert!(payload["features"]
        .as_object()
        .expect("features object")
        .contains_key("ghcr.io/codspace/doesnotexist:0.1.2"));

    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn outdated_supports_text_output_fixture() {
    let workspace = copied_lockfile_fixture("lockfile-outdated-command");

    let output = devcontainer_command(None)
        .args([
            "outdated",
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--output-format",
            "text",
        ])
        .output()
        .expect("outdated should run");

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(stdout.contains("Current"));
    assert!(stdout.contains("Wanted"));
    assert!(stdout.contains("Latest"));
    assert!(stdout.contains("ghcr.io/devcontainers/features/git"));
    assert!(stdout.contains("ghcr.io/devcontainers/features/azure-cli"));
    assert!(!stdout.contains("mylocalfeature"));
    assert!(!stdout.contains("terraform"));

    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn outdated_emits_json_logs_when_requested() {
    let workspace = copied_lockfile_fixture("lockfile-outdated-command");

    let output = devcontainer_command(None)
        .args([
            "outdated",
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--output-format",
            "json",
            "--log-format",
            "json",
            "--log-level",
            "trace",
        ])
        .output()
        .expect("outdated should run");

    assert!(output.status.success(), "{output:?}");
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json payload");
    assert_eq!(
        payload["features"]["ghcr.io/devcontainers/features/git:1.0"]["latest"],
        "1.2.0"
    );

    let logs = stderr_json_logs(&output);
    assert!(!logs.is_empty(), "{logs:?}");
    assert!(
        logs.iter()
            .any(|entry| entry["type"] == "text" && entry["level"] == 1),
        "{logs:?}"
    );
    assert!(
        logs.iter()
            .all(|entry| entry["timestamp"].as_i64().is_some()),
        "{logs:?}"
    );
    assert!(
        logs.iter().any(|entry| entry["text"]
            .as_str()
            .is_some_and(|message| message.contains("Generated outdated payload"))),
        "{logs:?}"
    );

    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn outdated_trace_logs_include_terminal_dimensions() {
    let workspace = copied_lockfile_fixture("lockfile-outdated-command");

    let output = devcontainer_command(None)
        .args([
            "outdated",
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--output-format",
            "json",
            "--log-level",
            "trace",
            "--terminal-columns",
            "120",
            "--terminal-rows",
            "40",
        ])
        .output()
        .expect("outdated should run");

    assert!(output.status.success(), "{output:?}");
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json payload");
    assert_eq!(
        payload["features"]["ghcr.io/devcontainers/features/git:1.0"]["latest"],
        "1.2.0"
    );
    let stderr = utf8_stderr(&output);
    assert!(stderr.contains("terminal dimensions"), "{stderr}");
    assert!(stderr.contains("120"), "{stderr}");
    assert!(stderr.contains("40"), "{stderr}");

    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn outdated_rejects_unpaired_terminal_dimensions() {
    let workspace = copied_lockfile_fixture("lockfile-outdated-command");

    let output = devcontainer_command(None)
        .args([
            "outdated",
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--output-format",
            "json",
            "--terminal-columns",
            "120",
        ])
        .output()
        .expect("outdated should run");

    assert!(!output.status.success(), "{output:?}");
    let stderr = utf8_stderr(&output);
    assert!(stderr.contains("--terminal-columns"), "{stderr}");
    assert!(stderr.contains("--terminal-rows"), "{stderr}");

    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn outdated_rejects_non_numeric_terminal_dimensions() {
    let workspace = copied_lockfile_fixture("lockfile-outdated-command");

    let output = devcontainer_command(None)
        .args([
            "outdated",
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--output-format",
            "json",
            "--terminal-columns",
            "wide",
            "--terminal-rows",
            "40",
        ])
        .output()
        .expect("outdated should run");

    assert!(!output.status.success(), "{output:?}");
    let stderr = utf8_stderr(&output);
    assert!(stderr.contains("--terminal-columns"), "{stderr}");
    assert!(stderr.contains("number"), "{stderr}");

    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn outdated_rejects_invalid_log_level() {
    let workspace = copied_lockfile_fixture("lockfile-outdated-command");

    let output = devcontainer_command(None)
        .args([
            "outdated",
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--output-format",
            "json",
            "--log-level",
            "warning",
        ])
        .output()
        .expect("outdated should run");

    assert!(!output.status.success(), "{output:?}");
    let stderr = utf8_stderr(&output);
    assert!(stderr.contains("--log-level"), "{stderr}");
    assert!(stderr.contains("warning"), "{stderr}");

    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn upgrade_matches_upstream_lockfile_fixture() {
    let workspace = copied_lockfile_fixture("lockfile-upgrade-command");
    fs::copy(
        workspace.join("outdated.devcontainer-lock.json"),
        workspace.join(".devcontainer-lock.json"),
    )
    .expect("seed lockfile");

    let output = devcontainer_command(None)
        .args([
            "upgrade",
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
        ])
        .output()
        .expect("upgrade should run");

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(workspace.join(".devcontainer-lock.json")).expect("actual lockfile"),
        fs::read_to_string(workspace.join("upgraded.devcontainer-lock.json"))
            .expect("expected lockfile")
    );

    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn upgrade_trace_logs_cover_dry_run_milestones() {
    let workspace = copied_lockfile_fixture("lockfile-upgrade-command");

    let output = devcontainer_command(None)
        .args([
            "upgrade",
            "--dry-run",
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--log-level",
            "trace",
        ])
        .output()
        .expect("upgrade should run");

    assert!(output.status.success(), "{output:?}");
    let payload: Value = serde_json::from_slice(&output.stdout).expect("dry-run lockfile");
    let expected: Value = serde_json::from_str(
        &fs::read_to_string(workspace.join("upgraded.devcontainer-lock.json"))
            .expect("expected lockfile"),
    )
    .expect("expected lockfile json");
    assert_eq!(payload, expected);
    let stderr = utf8_stderr(&output);
    assert!(
        stderr.contains("Loading dev container configuration"),
        "{stderr}"
    );
    assert!(stderr.contains("Generating lockfile"), "{stderr}");
    assert!(
        stderr.contains("Dry-run lockfile generation complete"),
        "{stderr}"
    );

    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn upgrade_rejects_invalid_log_level() {
    let workspace = copied_lockfile_fixture("lockfile-upgrade-command");

    let output = devcontainer_command(None)
        .args([
            "upgrade",
            "--dry-run",
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--log-level",
            "warning",
        ])
        .output()
        .expect("upgrade should run");

    assert!(!output.status.success(), "{output:?}");
    let stderr = utf8_stderr(&output);
    assert!(stderr.contains("--log-level"), "{stderr}");
    assert!(stderr.contains("warning"), "{stderr}");

    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn upgrade_with_feature_updates_config_and_dry_run_lockfile() {
    let workspace = copied_lockfile_fixture("lockfile-upgrade-feature");
    fs::copy(
        workspace.join("input.devcontainer.json"),
        workspace.join(".devcontainer.json"),
    )
    .expect("seed config");

    let output = devcontainer_command(None)
        .args([
            "upgrade",
            "--dry-run",
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--feature",
            "ghcr.io/codspace/versioning/foo",
            "--target-version",
            "2",
        ])
        .output()
        .expect("upgrade should run");

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(workspace.join(".devcontainer.json")).expect("updated config"),
        fs::read_to_string(workspace.join("expected.devcontainer.json")).expect("expected config")
    );
    let payload: Value = serde_json::from_slice(&output.stdout).expect("dry-run lockfile");
    assert_eq!(
        payload["features"]["ghcr.io/codspace/versioning/foo:2"]["version"],
        "2.11.1"
    );

    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn upgrade_supports_upstream_dependson_lockfile_fixture() {
    let workspace = copied_lockfile_fixture("lockfile-dependson");

    let output = devcontainer_command(None)
        .args([
            "upgrade",
            "--dry-run",
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
        ])
        .output()
        .expect("upgrade should run");

    assert!(output.status.success(), "{output:?}");
    let payload: Value = serde_json::from_slice(&output.stdout).expect("dry-run lockfile");
    assert_eq!(
        payload["features"]["ghcr.io/codspace/dependson/A:2"]["version"],
        "2.0.1"
    );
    assert_eq!(
        payload["features"]["ghcr.io/codspace/dependson/E:1"]["version"],
        "1.0.0"
    );
    assert_eq!(
        payload["features"]["https://github.com/codspace/tgz-features-with-dependson/releases/download/0.0.2/devcontainer-feature-A.tgz"]["version"],
        "2.0.1"
    );

    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn upgrade_reads_workspace_oci_layout_mirror() {
    let workspace = unique_temp_dir("devcontainer-cli-smoke");
    fs::create_dir_all(&workspace).expect("workspace");
    write_workspace_layout_version(
        &workspace,
        "ghcr.io/acme/features/published-feature",
        "1.0.0",
        None,
    );
    let latest_digest = write_workspace_layout_version(
        &workspace,
        "ghcr.io/acme/features/published-feature",
        "1.1.0",
        Some(&["ghcr.io/acme/features/dependency"]),
    );
    fs::write(
        workspace.join(".devcontainer.json"),
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"ghcr.io/acme/features/published-feature:1\": {}\n  }\n}\n",
    )
    .expect("config");

    let output = devcontainer_command(None)
        .args([
            "upgrade",
            "--dry-run",
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
        ])
        .output()
        .expect("upgrade should run");

    assert!(output.status.success(), "{output:?}");
    let payload: Value = serde_json::from_slice(&output.stdout).expect("dry-run lockfile");
    assert_eq!(
        payload["features"]["ghcr.io/acme/features/published-feature:1"]["version"],
        "1.1.0"
    );
    assert_eq!(
        payload["features"]["ghcr.io/acme/features/published-feature:1"]["resolved"],
        format!("ghcr.io/acme/features/published-feature@sha256:{latest_digest}")
    );

    let _ = fs::remove_dir_all(workspace);
}

fn write_workspace_layout_version(
    workspace_root: &Path,
    base: &str,
    version: &str,
    depends_on: Option<&[&str]>,
) -> String {
    let layout_dir = workspace_root
        .join(".devcontainer")
        .join("oci-layouts")
        .join(base);
    fs::create_dir_all(layout_dir.join("blobs").join("sha256")).expect("layout blobs");
    fs::write(
        layout_dir.join("oci-layout"),
        "{\n  \"imageLayoutVersion\": \"1.0.0\"\n}\n",
    )
    .expect("layout marker");

    let metadata = json!({
        "id": "published-feature",
        "version": version,
        "dependsOn": depends_on.map(<[_]>::to_vec),
    });
    let manifest = json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "annotations": {
            "dev.containers.metadata": metadata.to_string(),
        }
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).expect("manifest bytes");
    let digest = sha256_digest(&manifest_bytes);
    fs::write(
        layout_dir.join("blobs").join("sha256").join(&digest),
        &manifest_bytes,
    )
    .expect("manifest blob");

    let mut manifests = if layout_dir.join("index.json").is_file() {
        let index: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(layout_dir.join("index.json")).expect("index"),
        )
        .expect("index json");
        index["manifests"].as_array().cloned().unwrap_or_default()
    } else {
        Vec::new()
    };
    manifests.push(json!({
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "digest": format!("sha256:{digest}"),
        "size": manifest_bytes.len(),
        "annotations": {
            "org.opencontainers.image.ref.name": version,
        }
    }));
    fs::write(
        layout_dir.join("index.json"),
        serde_json::to_string_pretty(&json!({
            "schemaVersion": 2,
            "manifests": manifests,
        }))
        .expect("index payload"),
    )
    .expect("index write");

    digest
}

fn sha256_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}
