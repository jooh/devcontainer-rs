//! Feature collection listing and inspection commands.

use std::path::Path;

use serde_json::{json, Value};

use super::oci;
use super::registry::normalize_collection_reference;
use crate::commands::common;

pub(super) fn build_features_resolve_dependencies_payload(
    args: &[String],
) -> Result<Value, String> {
    let (workspace_folder, config_file, configuration) = common::load_resolved_config(args)?;
    let resolved = crate::commands::configuration::resolve_feature_support_without_lockfile(
        args,
        &workspace_folder,
        &config_file,
        &configuration,
    )?;
    let ordered = if let Some(resolved) = resolved {
        let mut resolved_features = Vec::with_capacity(resolved.ordered_feature_ids.len());
        for id in resolved.ordered_feature_ids {
            resolved_features.push(Value::String(id));
        }
        let mut install_order = Vec::with_capacity(resolved.ordered_features.len());
        for feature in resolved.ordered_features {
            install_order.push(json!({
                "id": feature.id,
                "options": feature.options,
            }));
        }
        (resolved_features, install_order)
    } else {
        (Vec::new(), Vec::new())
    };

    Ok(json!({
        "outcome": "success",
        "command": "features resolve-dependencies",
        "resolvedFeatures": ordered.0,
        "installOrder": ordered.1,
    }))
}

#[cfg(test)]
pub(super) fn build_feature_info_payload(mode: &str, feature_path: &str) -> Result<Value, String> {
    build_feature_info_payload_with_workspace(mode, feature_path, None)
}

pub(super) fn build_feature_info_payload_with_workspace(
    mode: &str,
    feature_path: &str,
    workspace_folder: Option<&Path>,
) -> Result<Value, String> {
    match mode {
        "manifest" => {
            if oci::is_registry_qualified_reference(feature_path) {
                let (manifest, canonical_id) =
                    published_feature_manifest_payload(feature_path, workspace_folder)?;
                Ok(json!({
                    "manifest": manifest,
                    "canonicalId": canonical_id,
                }))
            } else {
                let manifest = feature_manifest(feature_path, workspace_folder)?;
                Ok(json!({
                    "id": manifest_value_or_string(&manifest, "id", "unknown"),
                    "name": manifest_value_or_string(&manifest, "name", "unknown"),
                    "version": manifest_value_or_string(&manifest, "version", "0.0.0"),
                    "options": manifest_value_or_empty_object(&manifest, "options"),
                }))
            }
        }
        "tags" => {
            if oci::is_registry_qualified_reference(feature_path) {
                let published_tags = published_feature_tags(feature_path, workspace_folder)?;
                Ok(json!({
                    "feature": normalize_collection_reference(feature_path),
                    "publishedTags": published_tags,
                }))
            } else {
                let manifest = feature_manifest(feature_path, workspace_folder)?;
                let tags = local_feature_tags(&manifest);
                Ok(json!({
                    "feature": normalize_collection_reference(feature_path),
                    "tags": tags,
                }))
            }
        }
        "dependencies" => {
            let manifest = feature_manifest(feature_path, workspace_folder)?;
            Ok(json!({
                "feature": normalize_collection_reference(feature_path),
                "dependsOn": manifest_value_or_empty_object(&manifest, "dependsOn"),
            }))
        }
        "verbose" => {
            if oci::is_registry_qualified_reference(feature_path) {
                let artifact = oci::resolve_feature_artifact(feature_path, workspace_folder)?;
                let published_tags = published_feature_tags(feature_path, workspace_folder)?;
                let canonical_id = oci::canonical_feature_id(&artifact);
                Ok(json!({
                    "feature": normalize_collection_reference(feature_path),
                    "manifest": artifact.manifest,
                    "canonicalId": canonical_id,
                    "publishedTags": published_tags,
                    "dependsOn": manifest_value_or_empty_object(&artifact.metadata, "dependsOn"),
                }))
            } else {
                let manifest = feature_manifest(feature_path, workspace_folder)?;
                let tags = local_feature_tags(&manifest);
                Ok(json!({
                    "feature": normalize_collection_reference(feature_path),
                    "manifest": manifest,
                    "tags": tags,
                    "dependsOn": manifest_value_or_empty_object(&manifest, "dependsOn"),
                }))
            }
        }
        _ => Err(format!("Unsupported features info mode: {mode}")),
    }
}

fn feature_manifest(feature_path: &str, workspace_folder: Option<&Path>) -> Result<Value, String> {
    if oci::is_registry_qualified_reference(feature_path) {
        let artifact = oci::resolve_feature_artifact(feature_path, workspace_folder)?;
        Ok(artifact.metadata)
    } else {
        common::parse_manifest(Path::new(feature_path), "devcontainer-feature.json")
    }
}

fn local_feature_tags(manifest: &Value) -> Vec<Value> {
    if let Some(version) = manifest.get("version") {
        vec![version.clone()]
    } else {
        Vec::new()
    }
}

fn published_feature_tags(
    feature_path: &str,
    workspace_folder: Option<&Path>,
) -> Result<Vec<Value>, String> {
    let tags = oci::list_feature_tags(feature_path, workspace_folder)?;
    let mut values = Vec::with_capacity(tags.len());
    for tag in tags {
        values.push(Value::String(tag));
    }
    Ok(values)
}

fn published_feature_manifest_payload(
    feature_path: &str,
    workspace_folder: Option<&Path>,
) -> Result<(Value, String), String> {
    let artifact = oci::resolve_feature_artifact(feature_path, workspace_folder)?;
    Ok((
        artifact.manifest.clone(),
        oci::canonical_feature_id(&artifact),
    ))
}

fn manifest_value_or_string(manifest: &Value, key: &str, default: &str) -> Value {
    match manifest.get(key) {
        Some(value) => value.clone(),
        None => Value::String(default.to_string()),
    }
}

fn manifest_value_or_empty_object(manifest: &Value, key: &str) -> Value {
    match manifest.get(key) {
        Some(value) => value.clone(),
        None => json!({}),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use sha2::{Digest, Sha256};

    use super::build_feature_info_payload_with_workspace;

    #[test]
    fn zero_hit_published_feature_dependencies_read_metadata_from_local_oci_layout() {
        let workspace = crate::test_support::unique_temp_dir("devcontainer-feature-info-oci");
        let resource = "ghcr.io/acme/features/local-feature";
        let layout_dir = workspace
            .join(".devcontainer")
            .join("oci-layouts")
            .join(resource);
        fs::create_dir_all(layout_dir.join("blobs").join("sha256")).expect("layout blobs");
        fs::write(
            layout_dir.join("oci-layout"),
            "{\n  \"imageLayoutVersion\": \"1.0.0\"\n}\n",
        )
        .expect("layout marker");

        let manifest = json!({
            "schemaVersion": 2,
            "layers": [],
            "annotations": {
                "dev.containers.metadata": json!({
                    "id": "local-feature",
                    "version": "1.0.0",
                    "dependsOn": {
                        "ghcr.io/acme/features/base": { "enabled": true }
                    }
                }).to_string(),
            }
        });
        let manifest_bytes = serde_json::to_vec_pretty(&manifest).expect("manifest");
        let manifest_digest = sha256(&manifest_bytes);
        fs::write(
            layout_dir
                .join("blobs")
                .join("sha256")
                .join(&manifest_digest),
            &manifest_bytes,
        )
        .expect("manifest blob");
        fs::write(
            layout_dir.join("index.json"),
            serde_json::to_string_pretty(&json!({
                "schemaVersion": 2,
                "manifests": [{
                    "digest": format!("sha256:{manifest_digest}"),
                    "annotations": {
                        "org.opencontainers.image.ref.name": "1.0.0",
                    }
                }]
            }))
            .expect("index"),
        )
        .expect("index write");

        let payload = build_feature_info_payload_with_workspace(
            "dependencies",
            "ghcr.io/acme/features/local-feature:1.0.0",
            Some(workspace.as_path()),
        )
        .expect("feature dependencies");

        assert_eq!(
            payload["dependsOn"]["ghcr.io/acme/features/base"]["enabled"],
            true
        );
        let _ = fs::remove_dir_all(workspace);
    }

    fn sha256(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        format!("{:x}", hasher.finalize())
    }
}
