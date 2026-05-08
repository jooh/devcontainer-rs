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
    let ordered = crate::commands::configuration::resolve_feature_support(
        args,
        &workspace_folder,
        &config_file,
        &configuration,
    )?
    .map(|resolved| {
        let resolved_features = resolved
            .ordered_feature_ids
            .iter()
            .cloned()
            .map(Value::String)
            .collect::<Vec<_>>();
        let install_order = resolved
            .ordered_features
            .into_iter()
            .map(|feature| {
                json!({
                    "id": feature.id,
                    "options": feature.options,
                })
            })
            .collect::<Vec<_>>();
        (resolved_features, install_order)
    })
    .unwrap_or_default();

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
    let manifest = feature_manifest(feature_path, workspace_folder)?;
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
                Ok(json!({
                    "id": manifest.get("id").cloned().unwrap_or_else(|| Value::String("unknown".to_string())),
                    "name": manifest.get("name").cloned().unwrap_or_else(|| Value::String("unknown".to_string())),
                    "version": manifest.get("version").cloned().unwrap_or_else(|| Value::String("0.0.0".to_string())),
                    "options": manifest.get("options").cloned().unwrap_or_else(|| json!({})),
                }))
            }
        }
        "tags" => {
            if oci::is_registry_qualified_reference(feature_path) {
                Ok(json!({
                    "feature": normalize_collection_reference(feature_path),
                    "publishedTags": feature_tags(feature_path, &manifest, workspace_folder)?,
                }))
            } else {
                Ok(json!({
                    "feature": normalize_collection_reference(feature_path),
                    "tags": feature_tags(feature_path, &manifest, workspace_folder)?,
                }))
            }
        }
        "dependencies" => Ok(json!({
            "feature": normalize_collection_reference(feature_path),
            "dependsOn": manifest.get("dependsOn").cloned().unwrap_or_else(|| json!({})),
        })),
        "verbose" => {
            if oci::is_registry_qualified_reference(feature_path) {
                let (oci_manifest, canonical_id) =
                    published_feature_manifest_payload(feature_path, workspace_folder)?;
                Ok(json!({
                    "feature": normalize_collection_reference(feature_path),
                    "manifest": oci_manifest,
                    "canonicalId": canonical_id,
                    "publishedTags": feature_tags(feature_path, &manifest, workspace_folder)?,
                    "dependsOn": manifest.get("dependsOn").cloned().unwrap_or_else(|| json!({})),
                }))
            } else {
                Ok(json!({
                    "feature": normalize_collection_reference(feature_path),
                    "manifest": manifest,
                    "tags": feature_tags(feature_path, &manifest, workspace_folder)?,
                    "dependsOn": manifest.get("dependsOn").cloned().unwrap_or_else(|| json!({})),
                }))
            }
        }
        _ => Err(format!("Unsupported features info mode: {mode}")),
    }
}

fn feature_manifest(feature_path: &str, workspace_folder: Option<&Path>) -> Result<Value, String> {
    if oci::is_registry_qualified_reference(feature_path) {
        oci::resolve_feature_artifact(feature_path, workspace_folder)
            .map(|artifact| artifact.metadata)
    } else {
        common::parse_manifest(Path::new(feature_path), "devcontainer-feature.json")
    }
}

fn feature_tags(
    feature_path: &str,
    manifest: &Value,
    workspace_folder: Option<&Path>,
) -> Result<Vec<Value>, String> {
    if oci::is_registry_qualified_reference(feature_path) {
        return oci::list_feature_tags(feature_path, workspace_folder)
            .map(|tags| tags.into_iter().map(Value::String).collect());
    }

    Ok(manifest
        .get("version")
        .cloned()
        .map(|version| vec![version])
        .unwrap_or_default())
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
