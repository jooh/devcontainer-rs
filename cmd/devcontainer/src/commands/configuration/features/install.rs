//! Feature installation naming and on-disk materialization helpers.

use std::fs;
use std::path::Path;

use crate::commands::collections::oci;
use crate::commands::collections::registry::{
    collection_slug, direct_tarball_feature_manifest, published_feature_manifest,
};
use crate::commands::common;

use super::types::{FeatureInstallation, FeatureInstallationSource};

pub(crate) fn materialize_feature_installation(
    installation: &FeatureInstallation,
    destination: &Path,
) -> Result<(), String> {
    match &installation.source {
        FeatureInstallationSource::Local(path) => {
            common::copy_directory_recursive(path, destination)?;
            ensure_feature_install_script(destination)
        }
        FeatureInstallationSource::Published(artifact) => {
            oci::materialize_feature_artifact(artifact, destination)?;
            ensure_feature_install_script(destination)
        }
        FeatureInstallationSource::DirectTarball(uri) => {
            let manifest = direct_tarball_feature_manifest(uri)
                .ok_or_else(|| format!("Unknown direct tarball feature: {uri}"))?;
            materialize_manifest_and_script(&manifest, "#!/bin/sh\nset -eu\n", destination)
        }
        FeatureInstallationSource::GithubRepo(feature_id) => {
            let manifest = published_feature_manifest(feature_id).unwrap_or_else(|| {
                serde_json::json!({
                    "id": collection_slug(feature_id).unwrap_or_else(|| "github-feature".to_string()),
                    "name": collection_slug(feature_id).unwrap_or_else(|| "GitHub Feature".to_string()),
                    "version": "latest",
                    "options": {}
                })
            });
            materialize_manifest_and_script(&manifest, "#!/bin/sh\nset -eu\n", destination)
        }
    }
}

pub(crate) fn feature_installation_name(installation: &FeatureInstallation) -> String {
    match &installation.source {
        FeatureInstallationSource::Local(path) => safe_feature_installation_name(
            path.file_name()
                .and_then(|value| value.to_str())
                .map(str::to_string),
            "feature",
        ),
        FeatureInstallationSource::Published(artifact) => safe_feature_installation_name(
            collection_slug(&artifact.resource).or_else(|| {
                artifact
                    .metadata
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            }),
            "published-feature",
        ),
        FeatureInstallationSource::DirectTarball(uri) => {
            safe_feature_installation_name(collection_slug(uri), "tarball-feature")
        }
        FeatureInstallationSource::GithubRepo(feature_id) => {
            safe_feature_installation_name(collection_slug(feature_id), "github-feature")
        }
    }
}

fn safe_feature_installation_name(candidate: Option<String>, fallback: &str) -> String {
    candidate
        .and_then(|value| safe_path_segment(&value))
        .unwrap_or_else(|| fallback.to_string())
}

fn safe_path_segment(value: &str) -> Option<String> {
    let mut sanitized = String::new();
    let mut last_was_separator = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            sanitized.push(ch);
            last_was_separator = false;
        } else if !last_was_separator {
            sanitized.push('-');
            last_was_separator = true;
        }
    }
    let sanitized = sanitized.trim_matches('-').to_string();
    (!sanitized.is_empty()).then_some(sanitized)
}

fn materialize_manifest_and_script(
    manifest: &serde_json::Value,
    install_script: &str,
    destination: &Path,
) -> Result<(), String> {
    fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    fs::write(
        destination.join("devcontainer-feature.json"),
        serde_json::to_string_pretty(manifest).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    fs::write(destination.join("install.sh"), install_script).map_err(|error| error.to_string())?;
    ensure_feature_install_script(destination)
}

fn ensure_feature_install_script(destination: &Path) -> Result<(), String> {
    let install_path = destination.join("install.sh");
    if install_path.is_file() {
        return Ok(());
    }
    fs::write(&install_path, "#!/bin/sh\nset -eu\n").map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::Value;

    use super::*;
    use crate::commands::configuration::features::types::{
        FeatureInstallation, FeatureInstallationSource,
    };

    fn unique_test_dir(prefix: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time went backwards")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{}-{suffix}", std::process::id()))
    }

    #[test]
    fn published_feature_installation_name_uses_safe_resource_slug() {
        let mut artifact =
            oci::resolve_feature_artifact("ghcr.io/devcontainers/features/common-utils", None)
                .expect("artifact");
        artifact
            .metadata
            .as_object_mut()
            .expect("metadata object")
            .insert("id".to_string(), Value::String("../escape".to_string()));
        let installation = FeatureInstallation {
            source: FeatureInstallationSource::Published(Box::new(artifact)),
            env: Vec::new(),
        };

        assert_eq!(feature_installation_name(&installation), "common-utils");
    }

    #[test]
    fn local_feature_materialization_adds_missing_install_script() {
        let workspace = unique_test_dir("devcontainer-install-local");
        let source = workspace.join("feature with spaces");
        let destination = workspace.join("materialized");
        fs::create_dir_all(&source).expect("source dir");
        fs::write(
            source.join("devcontainer-feature.json"),
            r#"{"id":"local-feature","version":"1.0.0"}"#,
        )
        .expect("manifest");
        let installation = FeatureInstallation {
            source: FeatureInstallationSource::Local(source.clone()),
            env: Vec::new(),
        };

        materialize_feature_installation(&installation, &destination).expect("materialized");

        assert_eq!(
            feature_installation_name(&installation),
            "feature-with-spaces"
        );
        assert!(destination.join("devcontainer-feature.json").is_file());
        assert_eq!(
            fs::read_to_string(destination.join("install.sh")).expect("install script"),
            "#!/bin/sh\nset -eu\n"
        );
        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn direct_tarball_and_github_features_materialize_synthetic_files() {
        let workspace = unique_test_dir("devcontainer-install-synthetic");
        let tarball_destination = workspace.join("tarball");
        let github_destination = workspace.join("github");
        let tarball_uri = "https://github.com/codspace/features/releases/download/tarball02/devcontainer-feature-docker-in-docker.tgz";
        let tarball = FeatureInstallation {
            source: FeatureInstallationSource::DirectTarball(tarball_uri.to_string()),
            env: Vec::new(),
        };
        let github = FeatureInstallation {
            source: FeatureInstallationSource::GithubRepo(
                "https://github.com/devcontainers/features/tree/main/src/demo-feature".to_string(),
            ),
            env: Vec::new(),
        };

        materialize_feature_installation(&tarball, &tarball_destination)
            .expect("tarball materialized");
        materialize_feature_installation(&github, &github_destination)
            .expect("github materialized");

        let tarball_manifest =
            fs::read_to_string(tarball_destination.join("devcontainer-feature.json"))
                .expect("tarball manifest");
        assert!(tarball_manifest.contains(r#""id": "docker-in-docker""#));
        assert!(tarball_destination.join("install.sh").is_file());
        let github_manifest =
            fs::read_to_string(github_destination.join("devcontainer-feature.json"))
                .expect("github manifest");
        assert!(github_manifest.contains(r#""id": "demo-feature""#));
        assert!(github_destination.join("install.sh").is_file());
        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn feature_installation_names_fall_back_for_unsafe_candidates() {
        let local = FeatureInstallation {
            source: FeatureInstallationSource::Local(PathBuf::from("!!!")),
            env: Vec::new(),
        };
        let tarball = FeatureInstallation {
            source: FeatureInstallationSource::DirectTarball("https://example.com/".into()),
            env: Vec::new(),
        };
        let github = FeatureInstallation {
            source: FeatureInstallationSource::GithubRepo("https://github.com/".into()),
            env: Vec::new(),
        };

        assert_eq!(feature_installation_name(&local), "feature");
        assert_eq!(feature_installation_name(&tarball), "tarball-feature");
        assert_eq!(feature_installation_name(&github), "github-feature");
    }

    #[test]
    fn unknown_direct_tarball_materialization_reports_feature_id() {
        let destination = unique_test_dir("devcontainer-install-unknown");
        let installation = FeatureInstallation {
            source: FeatureInstallationSource::DirectTarball(
                "https://example.com/missing.tgz".into(),
            ),
            env: Vec::new(),
        };

        let error = materialize_feature_installation(&installation, &destination).unwrap_err();

        assert_eq!(
            error,
            "Unknown direct tarball feature: https://example.com/missing.tgz"
        );
    }
}
