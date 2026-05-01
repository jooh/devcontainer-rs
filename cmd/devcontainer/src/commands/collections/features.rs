//! Feature collection listing and inspection commands.

use std::path::Path;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::registry::{
    live_ghcr_feature_manifest, normalize_collection_reference, published_feature_manifest,
    published_feature_oci_manifest,
};
use crate::commands::common;
use crate::commands::configuration::{catalog_versions, published_feature_canonical_id};

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

pub(super) fn build_feature_info_payload(mode: &str, feature_path: &str) -> Result<Value, String> {
    let manifest = feature_manifest(feature_path)?;
    match mode {
        "manifest" => {
            if feature_path.starts_with("ghcr.io/") {
                let (manifest, canonical_id) = published_feature_manifest_payload(feature_path)?;
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
            if feature_path.starts_with("ghcr.io/") {
                Ok(json!({
                    "feature": normalize_collection_reference(feature_path),
                    "publishedTags": feature_tags(feature_path, &manifest),
                }))
            } else {
                Ok(json!({
                    "feature": normalize_collection_reference(feature_path),
                    "tags": feature_tags(feature_path, &manifest),
                }))
            }
        }
        "dependencies" => Ok(json!({
            "feature": normalize_collection_reference(feature_path),
            "dependsOn": manifest.get("dependsOn").cloned().unwrap_or_else(|| json!({})),
        })),
        "verbose" => {
            if feature_path.starts_with("ghcr.io/") {
                let (oci_manifest, canonical_id) =
                    published_feature_manifest_payload(feature_path)?;
                Ok(json!({
                    "feature": normalize_collection_reference(feature_path),
                    "manifest": oci_manifest,
                    "canonicalId": canonical_id,
                    "publishedTags": feature_tags(feature_path, &manifest),
                    "dependsOn": manifest.get("dependsOn").cloned().unwrap_or_else(|| json!({})),
                }))
            } else {
                Ok(json!({
                    "feature": normalize_collection_reference(feature_path),
                    "manifest": manifest,
                    "tags": feature_tags(feature_path, &manifest),
                    "dependsOn": manifest.get("dependsOn").cloned().unwrap_or_else(|| json!({})),
                }))
            }
        }
        _ => Err(format!("Unsupported features info mode: {mode}")),
    }
}

fn feature_manifest(feature_path: &str) -> Result<Value, String> {
    if feature_path.starts_with("ghcr.io/") {
        published_feature_manifest(feature_path)
            .ok_or_else(|| format!("Unknown published feature: {feature_path}"))
    } else {
        common::parse_manifest(Path::new(feature_path), "devcontainer-feature.json")
    }
}

fn feature_tags(feature_path: &str, manifest: &Value) -> Vec<Value> {
    if feature_path.starts_with("ghcr.io/") {
        let normalized = normalize_collection_reference(feature_path);
        let tags = catalog_versions(&normalized)
            .into_iter()
            .map(Value::String)
            .collect::<Vec<_>>();
        if !tags.is_empty() {
            return tags;
        }
    }

    manifest
        .get("version")
        .cloned()
        .map(|version| vec![version])
        .unwrap_or_default()
}

fn published_feature_manifest_payload(feature_path: &str) -> Result<(Value, String), String> {
    if let Some(live_manifest) = live_ghcr_feature_manifest(feature_path)? {
        return Ok((
            live_manifest.manifest,
            format!(
                "{}@{}",
                normalize_collection_reference(feature_path),
                live_manifest.digest
            ),
        ));
    }

    let manifest = published_feature_oci_manifest(feature_path)
        .ok_or_else(|| format!("Unknown published feature: {feature_path}"))?;
    let canonical_id = match published_feature_canonical_id(feature_path) {
        Some(canonical_id) => canonical_id,
        None => canonical_feature_id(feature_path, &manifest)?,
    };
    Ok((manifest, canonical_id))
}

fn canonical_feature_id(feature_path: &str, manifest: &Value) -> Result<String, String> {
    let bytes = serde_json::to_vec(manifest).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(format!(
        "{}@sha256:{:x}",
        normalize_collection_reference(feature_path),
        hasher.finalize()
    ))
}
