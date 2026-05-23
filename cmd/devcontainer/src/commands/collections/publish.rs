//! Feature publishing command helpers for collection workflows.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use flate2::write::GzEncoder;
use flate2::Compression;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tar::Builder;

use crate::commands::common;

fn io_error_to_string(error: io::Error) -> String {
    error.to_string()
}

fn serde_json_error_to_string(error: serde_json::Error) -> String {
    error.to_string()
}

pub(super) fn publish_collection_target_to_oci(
    target: &Path,
    manifest_name: &str,
    prefix: &str,
    command: &str,
    args: &[String],
) -> Result<Value, String> {
    let manifest = common::parse_manifest(target, manifest_name)?;
    let archive = package_collection_target(target, manifest_name, prefix)?;
    let version = manifest
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or("latest");
    let registry = common::parse_option_value(args, "--registry").unwrap_or("ghcr.io".to_string());
    let namespace = common::parse_option_value(args, "--namespace");
    let output_dir = match common::parse_option_value(args, "--output-dir") {
        Some(output_dir) => PathBuf::from(output_dir),
        None => target
            .parent()
            .unwrap_or(target)
            .join(format!("{prefix}-oci-layout")),
    };
    let resource = match (
        namespace.as_ref(),
        manifest.get("id").and_then(Value::as_str),
    ) {
        (Some(namespace), Some(id)) => Some(format!("{registry}/{namespace}/{id}")),
        _ => None,
    };
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

pub(super) fn package_collection_target(
    target: &Path,
    manifest_name: &str,
    prefix: &str,
) -> Result<PathBuf, String> {
    let _ = common::parse_manifest(target, manifest_name)?;
    let archive_name = format!(
        "{}-{}.tgz",
        prefix,
        target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(prefix)
    );
    let archive_path = target.parent().unwrap_or(target).join(archive_name);
    let archive_file = fs::File::create(&archive_path).map_err(io_error_to_string)?;
    let encoder = GzEncoder::new(archive_file, Compression::default());
    let mut archive = Builder::new(encoder);
    archive
        .append_dir_all(".", target)
        .map_err(io_error_to_string)?;
    let encoder = archive.into_inner().map_err(io_error_to_string)?;
    encoder.finish().map_err(io_error_to_string)?;
    Ok(archive_path)
}

fn write_oci_layout(
    output_dir: &Path,
    archive: &Path,
    metadata: &Value,
    resource: Option<&str>,
    published_tags: &[String],
) -> Result<String, String> {
    fs::create_dir_all(output_dir.join("blobs").join("sha256")).map_err(io_error_to_string)?;
    fs::write(
        output_dir.join("oci-layout"),
        "{\n  \"imageLayoutVersion\": \"1.0.0\"\n}\n",
    )
    .map_err(io_error_to_string)?;

    let config_bytes = b"{}".to_vec();
    let config_digest = sha256_digest(&config_bytes);
    fs::write(
        output_dir.join("blobs").join("sha256").join(&config_digest),
        &config_bytes,
    )
    .map_err(io_error_to_string)?;

    let layer_bytes = fs::read(archive).map_err(io_error_to_string)?;
    let layer_digest = sha256_digest(&layer_bytes);
    fs::write(
        output_dir.join("blobs").join("sha256").join(&layer_digest),
        &layer_bytes,
    )
    .map_err(io_error_to_string)?;

    let mut annotations = json!({
        "dev.containers.metadata": serde_json::to_string(metadata)
            .expect("serializing JSON value cannot fail"),
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
        serde_json::to_vec_pretty(&manifest_json).expect("serializing JSON value cannot fail");
    let manifest_digest = sha256_digest(&manifest_bytes);
    fs::write(
        output_dir
            .join("blobs")
            .join("sha256")
            .join(&manifest_digest),
        &manifest_bytes,
    )
    .map_err(io_error_to_string)?;

    let mut manifests = existing_index_manifests(output_dir)?;
    manifests.retain(|entry| {
        entry["annotations"]["org.opencontainers.image.ref.name"]
            .as_str()
            .is_none_or(|tag| !published_tags.iter().any(|published| published == tag))
    });
    for tag in published_tags {
        manifests.push(json!({
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "digest": format!("sha256:{manifest_digest}"),
            "size": manifest_bytes.len(),
            "annotations": {
                "org.opencontainers.image.ref.name": tag,
            }
        }));
    }
    fs::write(
        output_dir.join("index.json"),
        serde_json::to_string_pretty(&json!({
            "schemaVersion": 2,
            "manifests": manifests
        }))
        .expect("serializing JSON value cannot fail"),
    )
    .map_err(io_error_to_string)?;

    Ok(format!("sha256:{manifest_digest}"))
}

fn sha256_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn published_tags_from_layout(output_dir: &Path) -> Result<Vec<String>, String> {
    let mut tags = Vec::new();
    for entry in existing_index_manifests(output_dir)? {
        if let Some(tag) = entry["annotations"]["org.opencontainers.image.ref.name"].as_str() {
            tags.push(tag.to_string());
        }
    }
    Ok(tags)
}

fn existing_index_manifests(output_dir: &Path) -> Result<Vec<Value>, String> {
    let index_path = output_dir.join("index.json");
    if !index_path.is_file() {
        return Ok(Vec::new());
    }

    let index_raw = fs::read_to_string(index_path).map_err(io_error_to_string)?;
    let index: Value = serde_json::from_str(&index_raw).map_err(serde_json_error_to_string)?;
    match index.get("manifests").and_then(Value::as_array) {
        Some(manifests) => Ok(manifests.clone()),
        None => Ok(Vec::new()),
    }
}

fn semantic_tags_for_version(version: &str, existing_tags: &[String]) -> Vec<String> {
    let Some(parsed) = parse_semver(version) else {
        return vec![version.to_string()];
    };
    let mut tags = Vec::new();
    if should_publish_tag(existing_tags, parsed, TagRange::Major) {
        tags.push(parsed.major.to_string());
    }
    if should_publish_tag(existing_tags, parsed, TagRange::Minor) {
        tags.push(format!("{}.{}", parsed.major, parsed.minor));
    }
    tags.push(version.to_string());
    if should_publish_tag(existing_tags, parsed, TagRange::Any) {
        tags.push("latest".to_string());
    }
    tags
}

fn should_publish_tag(existing_tags: &[String], version: SemVer, range: TagRange) -> bool {
    let mut published_max = None;
    for tag in existing_tags {
        let Some(candidate) = parse_semver(tag) else {
            continue;
        };
        if !range.matches(version, candidate) {
            continue;
        }
        if published_max < Some(candidate) {
            published_max = Some(candidate);
        }
    }
    match published_max {
        Some(published_max) => version >= published_max,
        None => true,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TagRange {
    Major,
    Minor,
    Any,
}

impl TagRange {
    fn matches(self, version: SemVer, candidate: SemVer) -> bool {
        match self {
            TagRange::Major => candidate.major == version.major,
            TagRange::Minor => candidate.major == version.major && candidate.minor == version.minor,
            TagRange::Any => true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SemVer {
    major: u64,
    minor: u64,
    patch: u64,
}

#[allow(clippy::question_mark)]
fn parse_semver(input: &str) -> Option<SemVer> {
    let mut parts = input.split('.');
    let major = match parts
        .next()
        .expect("split always yields one segment")
        .parse()
    {
        Ok(major) => major,
        Err(_) => return None,
    };
    let Some(minor) = parts.next() else {
        return None;
    };
    let minor = match minor.parse() {
        Ok(minor) => minor,
        Err(_) => return None,
    };
    let Some(patch) = parts.next() else {
        return None;
    };
    let patch = match patch.parse() {
        Ok(patch) => patch,
        Err(_) => return None,
    };
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

    use super::{existing_index_manifests, semantic_tags_for_version};

    #[test]
    fn zero_hit_existing_index_without_manifests_is_treated_as_empty() {
        let output_dir = crate::test_support::unique_temp_dir("devcontainer-publish-index");
        fs::create_dir_all(&output_dir).expect("output dir");
        fs::write(output_dir.join("index.json"), "{\"schemaVersion\":2}\n").expect("index");

        let manifests = existing_index_manifests(&output_dir).expect("manifests");

        assert!(manifests.is_empty());
        let _ = fs::remove_dir_all(output_dir);
    }

    #[test]
    fn semantic_tags_keep_highest_existing_match_when_existing_tags_are_unsorted() {
        let existing = vec![
            "1.0.2".to_string(),
            "1.0.1".to_string(),
            "not-semver".to_string(),
        ];

        assert_eq!(semantic_tags_for_version("1.0.1", &existing), vec!["1.0.1"]);
    }

    #[test]
    fn semantic_tags_treat_malformed_versions_as_exact_tags() {
        for version in ["x.2.3", "1.x.3", "1.2.x", "1", "1.2", "1.2.3.4"] {
            assert_eq!(semantic_tags_for_version(version, &[]), vec![version]);
        }
    }
}
