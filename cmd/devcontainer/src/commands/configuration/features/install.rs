//! Feature installation naming and on-disk materialization helpers.

use std::fs;
use std::path::Path;

use crate::commands::collections::registry::{
    collection_slug, direct_tarball_feature_manifest, published_feature_install_script,
    published_feature_manifest,
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
        FeatureInstallationSource::Published(feature_id) => {
            let manifest = published_feature_manifest(feature_id)
                .ok_or_else(|| format!("Unknown published feature: {feature_id}"))?;
            materialize_manifest_and_script(
                &manifest,
                published_feature_install_script(feature_id),
                destination,
            )
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
        FeatureInstallationSource::Local(path) => path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("feature")
            .to_string(),
        FeatureInstallationSource::Published(feature_id) => {
            collection_slug(feature_id).unwrap_or_else(|| "published-feature".to_string())
        }
        FeatureInstallationSource::DirectTarball(uri) => {
            collection_slug(uri).unwrap_or_else(|| "tarball-feature".to_string())
        }
        FeatureInstallationSource::GithubRepo(feature_id) => {
            collection_slug(feature_id).unwrap_or_else(|| "github-feature".to_string())
        }
    }
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
