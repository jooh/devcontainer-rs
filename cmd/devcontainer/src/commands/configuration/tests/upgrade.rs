//! Unit tests for configuration upgrade and lockfile behavior.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;
use sha2::{Digest, Sha256};

use super::support::unique_temp_dir;
use crate::commands::configuration::ensure_native_lockfile;
use crate::commands::configuration::upgrade::{
    build_outdated_payload, feature_id_without_version, lockfile_path, run_upgrade_lockfile,
};

#[test]
fn outdated_payload_reports_remote_feature_versions() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");
    fs::write(
        root.join(".devcontainer.json"),
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"ghcr.io/devcontainers/features/git:1.0\": \"latest\",\n    \"./local-feature\": {}\n  }\n}\n",
    )
    .expect("failed to write config");
    fs::write(
        root.join(".devcontainer-lock.json"),
        "{\n  \"features\": {\n    \"ghcr.io/devcontainers/features/git:1.0\": {\n      \"version\": \"1.0.4\",\n      \"resolved\": \"ghcr.io/devcontainers/features/git@sha256:0bb490abcc0a3fb23937d29e2c18a225b51c5584edc0d9eb4131569a980f60b6\",\n      \"integrity\": \"sha256:0bb490abcc0a3fb23937d29e2c18a225b51c5584edc0d9eb4131569a980f60b6\"\n    }\n  }\n}\n",
    )
    .expect("failed to write lockfile");

    let args = vec!["--workspace-folder".to_string(), root.display().to_string()];
    let payload = build_outdated_payload(&args).expect("payload");

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
    assert!(payload["features"]["./local-feature"].is_null());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn upgrade_lockfile_uses_root_relative_lockfile_for_dotfile_configs() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");
    fs::write(
        root.join(".devcontainer.json"),
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"ghcr.io/devcontainers/features/github-cli\": \"latest\"\n  }\n}\n",
    )
    .expect("failed to write config");

    let lockfile =
        run_upgrade_lockfile(&["--workspace-folder".to_string(), root.display().to_string()])
            .expect("lockfile payload");

    let lockfile_path = root.join(".devcontainer-lock.json");
    assert!(lockfile_path.is_file());
    assert_eq!(
        lockfile.features["ghcr.io/devcontainers/features/github-cli"].version,
        "1.0.9"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn feature_id_without_version_handles_tags_and_digests() {
    assert_eq!(
        feature_id_without_version("ghcr.io/devcontainers/features/git:1.0"),
        "ghcr.io/devcontainers/features/git"
    );
    assert_eq!(
        feature_id_without_version(
            "ghcr.io/devcontainers/features/git-lfs@sha256:24d5802c837b2519b666a8403a9514c7296d769c9607048e9f1e040e7d7e331c"
        ),
        "ghcr.io/devcontainers/features/git-lfs"
    );
}

#[test]
fn lockfile_path_matches_upstream_dotfile_rule() {
    assert_eq!(
        lockfile_path(Path::new("/tmp/workspace/.devcontainer.json")),
        PathBuf::from("/tmp/workspace/.devcontainer-lock.json")
    );
    assert_eq!(
        lockfile_path(Path::new("/tmp/workspace/.devcontainer/devcontainer.json")),
        PathBuf::from("/tmp/workspace/.devcontainer/devcontainer-lock.json")
    );
}

#[test]
fn outdated_payload_reads_workspace_oci_layout_versions() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");
    let digest = write_workspace_layout_version(
        &root,
        "ghcr.io/acme/features/published-feature",
        "1.0.0",
        None,
    );
    write_workspace_layout_version(
        &root,
        "ghcr.io/acme/features/published-feature",
        "1.0.1",
        None,
    );
    write_workspace_layout_version(
        &root,
        "ghcr.io/acme/features/published-feature",
        "2.0.0",
        None,
    );
    fs::write(
        root.join(".devcontainer.json"),
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"ghcr.io/acme/features/published-feature:1.0\": {}\n  }\n}\n",
    )
    .expect("failed to write config");
    fs::write(
        root.join(".devcontainer-lock.json"),
        format!(
            "{{\n  \"features\": {{\n    \"ghcr.io/acme/features/published-feature:1.0\": {{\n      \"version\": \"1.0.0\",\n      \"resolved\": \"ghcr.io/acme/features/published-feature@sha256:{digest}\",\n      \"integrity\": \"sha256:{digest}\"\n    }}\n  }}\n}}\n"
        ),
    )
    .expect("failed to write lockfile");

    let args = vec!["--workspace-folder".to_string(), root.display().to_string()];
    let payload = build_outdated_payload(&args).expect("payload");

    assert_eq!(
        payload["features"]["ghcr.io/acme/features/published-feature:1.0"]["current"],
        "1.0.0"
    );
    assert_eq!(
        payload["features"]["ghcr.io/acme/features/published-feature:1.0"]["wanted"],
        "1.0.1"
    );
    assert_eq!(
        payload["features"]["ghcr.io/acme/features/published-feature:1.0"]["latest"],
        "2.0.0"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn upgrade_lockfile_reads_workspace_oci_layout_digests() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");
    write_workspace_layout_version(
        &root,
        "ghcr.io/acme/features/published-feature",
        "1.0.0",
        None,
    );
    let latest_digest = write_workspace_layout_version(
        &root,
        "ghcr.io/acme/features/published-feature",
        "1.1.0",
        Some(&["ghcr.io/acme/features/dependency"]),
    );
    fs::write(
        root.join(".devcontainer.json"),
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"ghcr.io/acme/features/published-feature:1\": {}\n  }\n}\n",
    )
    .expect("failed to write config");

    let lockfile =
        run_upgrade_lockfile(&["--workspace-folder".to_string(), root.display().to_string()])
            .expect("lockfile payload");

    assert_eq!(
        lockfile.features["ghcr.io/acme/features/published-feature:1"].version,
        "1.1.0"
    );
    assert_eq!(
        lockfile.features["ghcr.io/acme/features/published-feature:1"].resolved,
        format!("ghcr.io/acme/features/published-feature@sha256:{latest_digest}")
    );
    assert_eq!(
        lockfile.features["ghcr.io/acme/features/published-feature:1"].depends_on,
        Some(vec!["ghcr.io/acme/features/dependency".to_string()])
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn ensure_native_lockfile_uses_shared_lockfile_format() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");
    let config_file = root.join(".devcontainer.json");

    ensure_native_lockfile(
        &[
            "--workspace-folder".to_string(),
            root.display().to_string(),
            "--experimental-lockfile".to_string(),
        ],
        &config_file,
        &json!({
            "image": "debian:bookworm",
            "features": {
                "ghcr.io/devcontainers/features/github-cli": {}
            }
        }),
    )
    .expect("lockfile write");

    let lockfile = fs::read_to_string(root.join(".devcontainer-lock.json")).expect("lockfile");
    assert!(!lockfile.ends_with('\n'));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn upgrade_lockfile_uses_shared_lockfile_format() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");
    fs::write(
        root.join(".devcontainer.json"),
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"ghcr.io/devcontainers/features/github-cli\": {}\n  }\n}\n",
    )
    .expect("failed to write config");

    run_upgrade_lockfile(&["--workspace-folder".to_string(), root.display().to_string()])
        .expect("lockfile payload");

    let lockfile = fs::read_to_string(root.join(".devcontainer-lock.json")).expect("lockfile");
    assert!(!lockfile.ends_with('\n'));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn ensure_native_lockfile_reports_missing_frozen_lockfile() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");
    let config_file = root.join(".devcontainer.json");

    let error = ensure_native_lockfile(
        &[
            "--workspace-folder".to_string(),
            root.display().to_string(),
            "--experimental-frozen-lockfile".to_string(),
        ],
        &config_file,
        &json!({
            "image": "debian:bookworm",
            "features": {
                "ghcr.io/devcontainers/features/github-cli": {}
            }
        }),
    )
    .expect_err("missing frozen lockfile error");

    assert_eq!(error, "Lockfile does not exist.");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn ensure_native_lockfile_accepts_semantically_identical_existing_json() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");
    let config_file = root.join(".devcontainer.json");
    ensure_native_lockfile(
        &[
            "--workspace-folder".to_string(),
            root.display().to_string(),
            "--experimental-lockfile".to_string(),
        ],
        &config_file,
        &json!({
            "image": "debian:bookworm",
            "features": {
                "ghcr.io/devcontainers/features/github-cli": {}
            }
        }),
    )
    .expect("lockfile seed");
    let lockfile_path = root.join(".devcontainer-lock.json");
    let lockfile = fs::read_to_string(&lockfile_path).expect("lockfile");
    let reformatted = lockfile.trim_end_matches('\n').to_string();
    fs::write(&lockfile_path, reformatted).expect("lockfile rewrite");

    ensure_native_lockfile(
        &[
            "--workspace-folder".to_string(),
            root.display().to_string(),
            "--experimental-frozen-lockfile".to_string(),
        ],
        &config_file,
        &json!({
            "image": "debian:bookworm",
            "features": {
                "ghcr.io/devcontainers/features/github-cli": {}
            }
        }),
    )
    .expect("lockfile match");

    let _ = fs::remove_dir_all(root);
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
        "dependsOn": depends_on.map(|entries| entries.iter().copied().collect::<Vec<_>>()),
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
