//! Feature publishing command helpers for collection workflows.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::commands::common;

pub(super) fn publish_collection_target_to_oci(
    target: &Path,
    manifest_name: &str,
    prefix: &str,
    command: &str,
    args: &[String],
) -> Result<Value, String> {
    let manifest = common::parse_manifest(target, manifest_name)?;
    let archive = common::package_collection_target(target, manifest_name, prefix)?;
    let version = manifest
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or("latest");
    let registry =
        common::parse_option_value(args, "--registry").unwrap_or_else(|| "ghcr.io".to_string());
    let namespace = common::parse_option_value(args, "--namespace");
    let output_dir = common::parse_option_value(args, "--output-dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            target
                .parent()
                .unwrap_or(target)
                .join(format!("{prefix}-oci-layout"))
        });
    let resource = namespace.as_ref().and_then(|namespace| {
        manifest
            .get("id")
            .and_then(Value::as_str)
            .map(|id| format!("{registry}/{namespace}/{id}"))
    });
    let existing_tags = published_tags_from_layout(&output_dir)?;
    let published_tags = semantic_tags_for_version(version, &existing_tags);
    let digest = write_oci_layout(
        &output_dir,
        &archive,
        &manifest,
        resource.as_deref(),
        &published_tags,
    )?;
    let mut payload = json!({
        "outcome": "success",
        "command": command,
        "archive": archive,
        "published": true,
        "layout": output_dir,
        "mode": "local-oci-layout",
        "registry": registry,
        "namespace": namespace,
        "resource": resource,
    });
    payload
        .as_object_mut()
        .expect("payload object")
        .insert("digest".to_string(), Value::String(digest));
    payload
        .as_object_mut()
        .expect("payload object")
        .insert("publishedTags".to_string(), json!(published_tags));
    payload
        .as_object_mut()
        .expect("payload object")
        .insert("version".to_string(), Value::String(version.to_string()));
    Ok(payload)
}

fn write_oci_layout(
    output_dir: &Path,
    archive: &Path,
    metadata: &Value,
    resource: Option<&str>,
    published_tags: &[String],
) -> Result<String, String> {
    fs::create_dir_all(output_dir.join("blobs").join("sha256"))
        .map_err(|error| error.to_string())?;
    fs::write(
        output_dir.join("oci-layout"),
        "{\n  \"imageLayoutVersion\": \"1.0.0\"\n}\n",
    )
    .map_err(|error| error.to_string())?;

    let config_bytes = b"{}".to_vec();
    let config_digest = sha256_digest(&config_bytes);
    fs::write(
        output_dir.join("blobs").join("sha256").join(&config_digest),
        &config_bytes,
    )
    .map_err(|error| error.to_string())?;

    let layer_bytes = fs::read(archive).map_err(|error| error.to_string())?;
    let layer_digest = sha256_digest(&layer_bytes);
    fs::write(
        output_dir.join("blobs").join("sha256").join(&layer_digest),
        &layer_bytes,
    )
    .map_err(|error| error.to_string())?;

    let mut annotations = json!({
        "dev.containers.metadata": serde_json::to_string(metadata).map_err(|error| error.to_string())?,
    });
    if let Some(resource) = resource {
        annotations
            .as_object_mut()
            .expect("annotations object")
            .insert(
                "org.opencontainers.image.ref.name".to_string(),
                Value::String(resource.to_string()),
            );
    }
    let manifest_json = json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {
            "mediaType": "application/vnd.oci.empty.v1+json",
            "digest": format!("sha256:{config_digest}"),
            "size": config_bytes.len(),
        },
        "layers": [{
            "mediaType": "application/vnd.devcontainers.layer.v1+tar+gzip",
            "digest": format!("sha256:{layer_digest}"),
            "size": layer_bytes.len(),
        }],
        "annotations": annotations,
    });
    let manifest_bytes =
        serde_json::to_vec_pretty(&manifest_json).map_err(|error| error.to_string())?;
    let manifest_digest = sha256_digest(&manifest_bytes);
    fs::write(
        output_dir
            .join("blobs")
            .join("sha256")
            .join(&manifest_digest),
        &manifest_bytes,
    )
    .map_err(|error| error.to_string())?;

    let mut manifests = existing_index_manifests(output_dir)?;
    manifests.retain(|entry| {
        entry["annotations"]["org.opencontainers.image.ref.name"]
            .as_str()
            .is_none_or(|tag| !published_tags.iter().any(|published| published == tag))
    });
    manifests.extend(published_tags.iter().map(|tag| {
        json!({
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "digest": format!("sha256:{manifest_digest}"),
            "size": manifest_bytes.len(),
            "annotations": {
                "org.opencontainers.image.ref.name": tag,
            }
        })
    }));
    fs::write(
        output_dir.join("index.json"),
        serde_json::to_string_pretty(&json!({
            "schemaVersion": 2,
            "manifests": manifests
        }))
        .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;

    Ok(format!("sha256:{manifest_digest}"))
}

fn sha256_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn published_tags_from_layout(output_dir: &Path) -> Result<Vec<String>, String> {
    Ok(existing_index_manifests(output_dir)?
        .into_iter()
        .filter_map(|entry| {
            entry["annotations"]["org.opencontainers.image.ref.name"]
                .as_str()
                .map(str::to_string)
        })
        .collect())
}

fn existing_index_manifests(output_dir: &Path) -> Result<Vec<Value>, String> {
    let index_path = output_dir.join("index.json");
    if !index_path.is_file() {
        return Ok(Vec::new());
    }

    let index: Value =
        serde_json::from_str(&fs::read_to_string(index_path).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    Ok(index
        .get("manifests")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

fn semantic_tags_for_version(version: &str, existing_tags: &[String]) -> Vec<String> {
    let Some(parsed) = parse_semver(version) else {
        return vec![version.to_string()];
    };
    let mut tags = Vec::new();
    if should_publish_tag(existing_tags, parsed, |candidate| {
        candidate.major == parsed.major
    }) {
        tags.push(parsed.major.to_string());
    }
    if should_publish_tag(existing_tags, parsed, |candidate| {
        candidate.major == parsed.major && candidate.minor == parsed.minor
    }) {
        tags.push(format!("{}.{}", parsed.major, parsed.minor));
    }
    tags.push(version.to_string());
    if should_publish_tag(existing_tags, parsed, |_| true) {
        tags.push("latest".to_string());
    }
    tags
}

fn should_publish_tag<F>(existing_tags: &[String], version: SemVer, matches_range: F) -> bool
where
    F: Fn(SemVer) -> bool,
{
    existing_tags
        .iter()
        .filter_map(|tag| parse_semver(tag))
        .filter(|candidate| matches_range(*candidate))
        .max()
        .is_none_or(|published_max| version >= published_max)
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SemVer {
    major: u64,
    minor: u64,
    patch: u64,
}

fn parse_semver(input: &str) -> Option<SemVer> {
    let mut parts = input.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(SemVer {
        major,
        minor,
        patch,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use crate::test_support::unique_temp_dir;

    use super::{parse_semver, publish_collection_target_to_oci};

    #[test]
    fn publish_without_output_dir_uses_default_sibling_layout() {
        let root = unique_temp_dir("devcontainer-publish");
        let feature = root.join("demo");
        fs::create_dir_all(&feature).expect("feature dir");
        fs::write(
            feature.join("devcontainer-feature.json"),
            r#"{"id":"demo","version":"1.2.3"}"#,
        )
        .expect("manifest");
        fs::write(feature.join("install.sh"), "#!/bin/sh\n").expect("install script");

        let payload = publish_collection_target_to_oci(
            &feature,
            "devcontainer-feature.json",
            "feature",
            "features publish",
            &[
                "--registry".to_string(),
                "registry.example.com".to_string(),
                "--namespace".to_string(),
                "acme/features".to_string(),
            ],
        )
        .expect("publish");

        assert_eq!(payload["layout"], json!(root.join("feature-oci-layout")));
        assert_eq!(
            payload["resource"],
            "registry.example.com/acme/features/demo"
        );
        assert_eq!(
            payload["publishedTags"],
            json!(["1", "1.2", "1.2.3", "latest"])
        );
        assert!(root.join("feature-oci-layout").join("index.json").is_file());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn parse_semver_rejects_extra_components() {
        assert!(parse_semver("1.2.3").is_some());
        assert!(parse_semver("1.2.3.4").is_none());
    }
}
