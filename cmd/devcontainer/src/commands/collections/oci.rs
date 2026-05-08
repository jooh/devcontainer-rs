//! Native OCI Distribution helpers for published devcontainer Feature artifacts.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, Cursor, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use flate2::read::GzDecoder;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tar::Archive;

use crate::process_runner::{self, ProcessLogLevel, ProcessRequest};

const OCI_MANIFEST_ACCEPT: &str =
    "application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json";
const OCI_BLOB_ACCEPT: &str = "application/octet-stream, application/vnd.devcontainers.layer.v1+tar, application/vnd.devcontainers.layer.v1+tar+gzip";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OciReference {
    pub(crate) original: String,
    pub(crate) resource: String,
    pub(crate) registry: String,
    pub(crate) repository: String,
    pub(crate) tag: Option<String>,
    pub(crate) digest: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct OciFeatureArtifact {
    pub(crate) original_reference: String,
    pub(crate) resource: String,
    pub(crate) registry: String,
    pub(crate) repository: String,
    pub(crate) tag: Option<String>,
    pub(crate) reference_digest: Option<String>,
    pub(crate) manifest_digest: String,
    pub(crate) manifest: Value,
    pub(crate) metadata: Value,
    pub(crate) layer: OciFeatureLayer,
}

#[derive(Clone, Debug)]
pub(crate) enum OciFeatureLayer {
    Registry {
        digest: String,
        media_type: String,
    },
    LocalPath {
        digest: String,
        media_type: String,
        path: PathBuf,
    },
    Generated {
        install_script: String,
    },
    Missing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct OciCatalogEntry {
    version: String,
    integrity: String,
}

#[derive(Debug)]
struct OciHttpResponse {
    status: u16,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

trait OciTransport {
    fn get(&self, url: &str, headers: &[(String, String)]) -> Result<OciHttpResponse, String>;
}

struct CurlTransport;

pub(crate) fn parse_oci_reference(input: &str) -> Option<OciReference> {
    let reference_without_selector = reference_without_selector(input);
    let resource = if is_registry_qualified_resource(&reference_without_selector) {
        reference_without_selector.to_ascii_lowercase()
    } else {
        format!(
            "ghcr.io/devcontainers/features/{}",
            reference_without_selector.to_ascii_lowercase()
        )
    };
    let (registry, repository) = {
        let (registry, repository) = resource.split_once('/')?;
        (registry.to_string(), repository.to_string())
    };
    let suffix = input
        .strip_prefix(&reference_without_selector)
        .unwrap_or("");
    let (tag, digest) = if let Some(digest) = suffix.strip_prefix('@') {
        (None, Some(digest.to_string()))
    } else if let Some(tag) = suffix.strip_prefix(':') {
        (Some(tag.to_string()), None)
    } else {
        (None, None)
    };

    Some(OciReference {
        original: input.to_string(),
        resource,
        registry,
        repository,
        tag,
        digest,
    })
}

pub(crate) fn is_registry_qualified_reference(input: &str) -> bool {
    if input.starts_with("http://") || input.starts_with("https://") || input.starts_with("file://")
    {
        return false;
    }
    is_registry_qualified_resource(&reference_without_selector(input))
}

pub(crate) fn resolve_feature_artifact(
    reference: &str,
    workspace_folder: Option<&Path>,
) -> Result<OciFeatureArtifact, String> {
    let parsed = parse_oci_reference(reference)
        .ok_or_else(|| format!("Invalid OCI Feature reference: {reference}"))?;
    resolve_feature_artifact_for_reference(&parsed, workspace_folder, &CurlTransport)
}

pub(crate) fn list_feature_tags(
    reference: &str,
    workspace_folder: Option<&Path>,
) -> Result<Vec<String>, String> {
    let parsed = parse_oci_reference(reference)
        .ok_or_else(|| format!("Invalid OCI Feature reference: {reference}"))?;
    if let Some(tags) = list_local_layout_tags(&parsed, workspace_folder)? {
        return Ok(tags);
    }
    if let Some(tags) = fixture_tags(&parsed.resource) {
        return Ok(tags);
    }
    registry_tags(&parsed, &CurlTransport)
}

pub(crate) fn feature_ref_json(artifact: &OciFeatureArtifact) -> Value {
    let id = artifact
        .metadata
        .get("id")
        .and_then(Value::as_str)
        .or_else(|| artifact.resource.rsplit('/').next())
        .unwrap_or_default();
    let version = artifact
        .metadata
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or_else(|| artifact.tag.as_deref().unwrap_or("latest"));
    let mut feature_ref = serde_json::Map::new();
    feature_ref.insert(
        "registry".to_string(),
        Value::String(artifact.registry.clone()),
    );
    feature_ref.insert(
        "path".to_string(),
        Value::String(artifact.repository.clone()),
    );
    feature_ref.insert(
        "resource".to_string(),
        Value::String(artifact.resource.clone()),
    );
    feature_ref.insert("id".to_string(), Value::String(id.to_string()));
    feature_ref.insert("version".to_string(), Value::String(version.to_string()));
    if let Some(tag) = &artifact.tag {
        feature_ref.insert("tag".to_string(), Value::String(tag.clone()));
    }
    if let Some(digest) = &artifact.reference_digest {
        feature_ref.insert("digest".to_string(), Value::String(digest.clone()));
    }
    Value::Object(feature_ref)
}

pub(crate) fn canonical_feature_id(artifact: &OciFeatureArtifact) -> String {
    format!("{}@{}", artifact.resource, artifact.manifest_digest)
}

pub(crate) fn materialize_feature_artifact(
    artifact: &OciFeatureArtifact,
    destination: &Path,
) -> Result<(), String> {
    match &artifact.layer {
        OciFeatureLayer::Registry { digest, media_type } => {
            let bytes = registry_blob(artifact, digest, &CurlTransport)?;
            verify_digest(digest, &bytes, "Feature layer")?;
            extract_feature_layer(&bytes, media_type, destination)
        }
        OciFeatureLayer::LocalPath {
            digest,
            media_type,
            path,
        } => {
            let bytes = fs::read(path).map_err(|error| error.to_string())?;
            verify_digest(digest, &bytes, "Feature layer")?;
            extract_feature_layer(&bytes, media_type, destination)
        }
        OciFeatureLayer::Generated { install_script } => {
            fs::create_dir_all(destination).map_err(|error| error.to_string())?;
            fs::write(
                destination.join("devcontainer-feature.json"),
                serde_json::to_string_pretty(&artifact.metadata)
                    .map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
            fs::write(destination.join("install.sh"), install_script)
                .map_err(|error| error.to_string())
        }
        OciFeatureLayer::Missing => Err(format!(
            "OCI Feature {} does not include a devcontainer Feature layer",
            artifact.original_reference
        )),
    }
}

fn resolve_feature_artifact_for_reference(
    parsed: &OciReference,
    workspace_folder: Option<&Path>,
    transport: &dyn OciTransport,
) -> Result<OciFeatureArtifact, String> {
    if let Some(artifact) = local_layout_feature_artifact(parsed, workspace_folder)? {
        return Ok(artifact);
    }
    if let Some(artifact) = fixture_feature_artifact(parsed)? {
        return Ok(artifact);
    }
    registry_feature_artifact(parsed, transport)
}

fn registry_feature_artifact(
    parsed: &OciReference,
    transport: &dyn OciTransport,
) -> Result<OciFeatureArtifact, String> {
    let manifest_reference = registry_manifest_reference(parsed, transport)?;
    let manifest_url = format!(
        "https://{}/v2/{}/manifests/{}",
        parsed.registry, parsed.repository, manifest_reference
    );
    let response = registry_get(
        transport,
        &parsed.registry,
        &manifest_url,
        &[("Accept".to_string(), OCI_MANIFEST_ACCEPT.to_string())],
    )?;
    if response.status != 200 {
        return Err(format!(
            "OCI registry returned HTTP {} for manifest {}",
            response.status, parsed.original
        ));
    }
    let header_digest = response.headers.get("docker-content-digest").cloned();
    let manifest_digest = verify_manifest_digest(parsed, header_digest, &response.body)?;
    let manifest: Value = serde_json::from_slice(&response.body).map_err(|error| {
        format!(
            "OCI registry returned an invalid manifest for {}: {error}",
            parsed.original
        )
    })?;
    artifact_from_manifest(
        parsed,
        manifest_reference,
        manifest_digest,
        manifest,
        None,
        transport,
    )
}

fn artifact_from_manifest(
    parsed: &OciReference,
    manifest_reference: String,
    manifest_digest: String,
    manifest: Value,
    local_layout_dir: Option<&Path>,
    transport: &dyn OciTransport,
) -> Result<OciFeatureArtifact, String> {
    let layer = feature_layer(&manifest, local_layout_dir)?;
    let metadata = match metadata_from_manifest_annotation(&manifest)? {
        Some(metadata) => metadata,
        None => metadata_from_feature_layer(parsed, &layer, transport)?,
    };
    Ok(OciFeatureArtifact {
        original_reference: parsed.original.clone(),
        resource: parsed.resource.clone(),
        registry: parsed.registry.clone(),
        repository: parsed.repository.clone(),
        tag: Some(manifest_reference).filter(|reference| !reference.starts_with("sha256:")),
        reference_digest: parsed.digest.clone(),
        manifest_digest,
        manifest,
        metadata,
        layer,
    })
}

fn registry_manifest_reference(
    parsed: &OciReference,
    transport: &dyn OciTransport,
) -> Result<String, String> {
    if let Some(digest) = &parsed.digest {
        return Ok(digest.clone());
    }
    let tag = parsed.tag.as_deref().unwrap_or("latest");
    if tag == "latest" || exact_semver(tag).is_some() {
        return Ok(tag.to_string());
    }
    let Some(selector) = VersionSelector::parse(tag) else {
        return Ok(tag.to_string());
    };
    let tags = registry_tags(parsed, transport)?;
    tags.into_iter()
        .filter(|candidate| selector.matches(candidate))
        .max_by(|left, right| compare_versions_asc(left, right))
        .ok_or_else(|| {
            format!(
                "No published versions of {} match selector {}",
                parsed.resource, tag
            )
        })
}

fn registry_tags(
    parsed: &OciReference,
    transport: &dyn OciTransport,
) -> Result<Vec<String>, String> {
    let url = format!(
        "https://{}/v2/{}/tags/list",
        parsed.registry, parsed.repository
    );
    let response = registry_get(transport, &parsed.registry, &url, &[])?;
    if response.status != 200 {
        return Err(format!(
            "OCI registry returned HTTP {} for tag list {}",
            response.status, parsed.resource
        ));
    }
    let payload: Value = serde_json::from_slice(&response.body).map_err(|error| {
        format!(
            "OCI registry returned an invalid tag list for {}: {error}",
            parsed.resource
        )
    })?;
    Ok(payload["tags"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|tag| tag.as_str().map(str::to_string))
        .collect())
}

fn registry_blob(
    artifact: &OciFeatureArtifact,
    digest: &str,
    transport: &dyn OciTransport,
) -> Result<Vec<u8>, String> {
    let url = format!(
        "https://{}/v2/{}/blobs/{}",
        artifact.registry, artifact.repository, digest
    );
    let response = registry_get(
        transport,
        &artifact.registry,
        &url,
        &[("Accept".to_string(), OCI_BLOB_ACCEPT.to_string())],
    )?;
    if response.status != 200 {
        return Err(format!(
            "OCI registry returned HTTP {} for blob {}",
            response.status, digest
        ));
    }
    Ok(response.body)
}

fn registry_get(
    transport: &dyn OciTransport,
    registry: &str,
    url: &str,
    headers: &[(String, String)],
) -> Result<OciHttpResponse, String> {
    let mut request_headers = headers.to_vec();
    if let Some(authorization) = configured_authorization(registry) {
        request_headers.push(("Authorization".to_string(), authorization));
    }
    let response = transport.get(url, &request_headers)?;
    if response.status != 401 {
        return Ok(response);
    }

    let Some(challenge) = response.headers.get("www-authenticate") else {
        return Ok(response);
    };
    let basic = configured_basic_authorization(registry);
    let token = fetch_bearer_token(transport, registry, challenge, basic.as_deref())?;
    let mut retry_headers = headers.to_vec();
    retry_headers.push(("Authorization".to_string(), format!("Bearer {token}")));
    transport.get(url, &retry_headers)
}

fn fetch_bearer_token(
    transport: &dyn OciTransport,
    registry: &str,
    challenge: &str,
    basic_authorization: Option<&str>,
) -> Result<String, String> {
    let challenge = challenge
        .strip_prefix("Bearer ")
        .or_else(|| challenge.strip_prefix("bearer "))
        .ok_or_else(|| format!("Unsupported OCI auth challenge: {challenge}"))?;
    let parameters = challenge_parameters(challenge);
    let realm = parameters
        .get("realm")
        .ok_or_else(|| format!("OCI auth challenge is missing a realm: {challenge}"))?;
    let service = parameters
        .get("service")
        .cloned()
        .unwrap_or_else(|| registry.to_string());
    let mut token_url = format!("{realm}?service={service}");
    if let Some(scope) = parameters.get("scope") {
        token_url.push_str("&scope=");
        token_url.push_str(scope);
    }
    let mut headers = Vec::new();
    if let Some(authorization) = basic_authorization {
        headers.push(("Authorization".to_string(), authorization.to_string()));
    }
    let response = transport.get(&token_url, &headers)?;
    if response.status != 200 {
        return Err(format!(
            "OCI token service returned HTTP {} for {registry}",
            response.status
        ));
    }
    let payload: Value = serde_json::from_slice(&response.body)
        .map_err(|error| format!("OCI token service returned invalid JSON: {error}"))?;
    payload["token"]
        .as_str()
        .or_else(|| payload["access_token"].as_str())
        .map(str::to_string)
        .ok_or_else(|| "OCI token service response did not include a token".to_string())
}

fn challenge_parameters(challenge: &str) -> HashMap<String, String> {
    challenge
        .split(',')
        .filter_map(|entry| entry.split_once('='))
        .map(|(key, value)| {
            (
                key.trim().to_string(),
                value.trim().trim_matches('"').to_string(),
            )
        })
        .collect()
}

fn local_layout_feature_artifact(
    parsed: &OciReference,
    workspace_folder: Option<&Path>,
) -> Result<Option<OciFeatureArtifact>, String> {
    let Some(layout_dir) = workspace_oci_layout_dir(&parsed.resource, workspace_folder) else {
        return Ok(None);
    };
    let Some((manifest_digest, tag)) = local_layout_manifest_digest(parsed, &layout_dir)? else {
        return Ok(None);
    };
    let manifest_bytes = read_layout_blob(&layout_dir, &manifest_digest)?;
    verify_digest(&manifest_digest, &manifest_bytes, "OCI layout manifest")?;
    let manifest: Value = serde_json::from_slice(&manifest_bytes).map_err(|error| {
        format!(
            "OCI layout manifest {} is invalid JSON: {error}",
            manifest_digest
        )
    })?;
    let artifact = artifact_from_manifest(
        parsed,
        tag.unwrap_or_else(|| manifest_digest.clone()),
        manifest_digest,
        manifest,
        Some(&layout_dir),
        &CurlTransport,
    )?;
    Ok(Some(artifact))
}

fn list_local_layout_tags(
    parsed: &OciReference,
    workspace_folder: Option<&Path>,
) -> Result<Option<Vec<String>>, String> {
    let Some(layout_dir) = workspace_oci_layout_dir(&parsed.resource, workspace_folder) else {
        return Ok(None);
    };
    let tags = local_layout_index_manifests(&layout_dir)?
        .into_iter()
        .filter_map(|entry| {
            entry["annotations"]["org.opencontainers.image.ref.name"]
                .as_str()
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    Ok(Some(tags))
}

fn local_layout_manifest_digest(
    parsed: &OciReference,
    layout_dir: &Path,
) -> Result<Option<(String, Option<String>)>, String> {
    if let Some(digest) = &parsed.digest {
        return Ok(Some((digest.clone(), None)));
    }
    let tag = parsed.tag.as_deref().unwrap_or("latest");
    let manifests = local_layout_index_manifests(layout_dir)?;
    if tag == "latest" {
        if let Some(entry) = manifests.iter().find(|entry| {
            entry["annotations"]["org.opencontainers.image.ref.name"].as_str() == Some("latest")
        }) {
            return Ok(entry["digest"]
                .as_str()
                .map(|digest| (digest.to_string(), Some("latest".to_string()))));
        }
    }
    if exact_semver(tag).is_some() {
        return Ok(manifests.iter().find_map(|entry| {
            (entry["annotations"]["org.opencontainers.image.ref.name"].as_str() == Some(tag))
                .then(|| {
                    entry["digest"]
                        .as_str()
                        .map(|digest| (digest.to_string(), Some(tag.to_string())))
                })
                .flatten()
        }));
    }
    if let Some(entry) = manifests.iter().find(|entry| {
        entry["annotations"]["org.opencontainers.image.ref.name"].as_str() == Some(tag)
    }) {
        return Ok(entry["digest"]
            .as_str()
            .map(|digest| (digest.to_string(), Some(tag.to_string()))));
    }
    let Some(selector) = VersionSelector::parse(tag) else {
        return Ok(None);
    };
    Ok(manifests
        .into_iter()
        .filter_map(|entry| {
            let tag = entry["annotations"]["org.opencontainers.image.ref.name"].as_str()?;
            if !selector.matches(tag) {
                return None;
            }
            let digest = entry["digest"].as_str()?;
            Some((tag.to_string(), digest.to_string()))
        })
        .max_by(|left, right| compare_versions_asc(&left.0, &right.0))
        .map(|(tag, digest)| (digest, Some(tag))))
}

fn local_layout_index_manifests(layout_dir: &Path) -> Result<Vec<Value>, String> {
    let index: Value = serde_json::from_str(
        &fs::read_to_string(layout_dir.join("index.json")).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    Ok(index["manifests"].as_array().cloned().unwrap_or_default())
}

fn workspace_oci_layout_dir(resource: &str, workspace_folder: Option<&Path>) -> Option<PathBuf> {
    let layout_dir = workspace_folder?
        .join(".devcontainer")
        .join("oci-layouts")
        .join(resource);
    layout_dir
        .join("oci-layout")
        .is_file()
        .then_some(layout_dir)
}

fn read_layout_blob(layout_dir: &Path, digest: &str) -> Result<Vec<u8>, String> {
    let hex = digest
        .strip_prefix("sha256:")
        .ok_or_else(|| format!("Unsupported OCI digest: {digest}"))?;
    fs::read(layout_dir.join("blobs").join("sha256").join(hex)).map_err(|error| error.to_string())
}

fn feature_layer(
    manifest: &Value,
    local_layout_dir: Option<&Path>,
) -> Result<OciFeatureLayer, String> {
    let Some(layer) = manifest["layers"].as_array().and_then(|layers| {
        layers.iter().find(|layer| {
            layer["mediaType"].as_str().is_some_and(|media_type| {
                media_type.starts_with("application/vnd.devcontainers.layer.")
            })
        })
    }) else {
        return Ok(OciFeatureLayer::Missing);
    };
    let digest = layer["digest"]
        .as_str()
        .ok_or_else(|| "OCI Feature layer descriptor is missing a digest".to_string())?
        .to_string();
    let media_type = layer["mediaType"]
        .as_str()
        .unwrap_or("application/vnd.devcontainers.layer.v1+tar")
        .to_string();
    if let Some(layout_dir) = local_layout_dir {
        let path = digest
            .strip_prefix("sha256:")
            .map(|hex| layout_dir.join("blobs").join("sha256").join(hex))
            .ok_or_else(|| format!("Unsupported OCI Feature layer digest: {digest}"))?;
        return Ok(OciFeatureLayer::LocalPath {
            digest,
            media_type,
            path,
        });
    }
    Ok(OciFeatureLayer::Registry { digest, media_type })
}

fn metadata_from_manifest_annotation(manifest: &Value) -> Result<Option<Value>, String> {
    let Some(raw) = manifest["annotations"]["dev.containers.metadata"].as_str() else {
        return Ok(None);
    };
    serde_json::from_str(raw)
        .map(Some)
        .map_err(|error| format!("OCI Feature metadata annotation is invalid JSON: {error}"))
}

fn metadata_from_feature_layer(
    parsed: &OciReference,
    layer: &OciFeatureLayer,
    transport: &dyn OciTransport,
) -> Result<Value, String> {
    let (bytes, media_type) = match layer {
        OciFeatureLayer::Registry { digest, media_type } => {
            let placeholder = OciFeatureArtifact {
                original_reference: parsed.original.clone(),
                resource: parsed.resource.clone(),
                registry: parsed.registry.clone(),
                repository: parsed.repository.clone(),
                tag: parsed.tag.clone(),
                reference_digest: parsed.digest.clone(),
                manifest_digest: String::new(),
                manifest: json!({}),
                metadata: json!({}),
                layer: layer.clone(),
            };
            (
                registry_blob(&placeholder, digest, transport)?,
                media_type.clone(),
            )
        }
        OciFeatureLayer::LocalPath {
            digest,
            media_type,
            path,
        } => {
            let bytes = fs::read(path).map_err(|error| error.to_string())?;
            verify_digest(digest, &bytes, "Feature layer")?;
            (bytes, media_type.clone())
        }
        OciFeatureLayer::Generated { .. } | OciFeatureLayer::Missing => {
            return Err(format!(
                "OCI Feature {} does not provide metadata",
                parsed.original
            ));
        }
    };
    feature_manifest_from_layer(&bytes, &media_type)
}

fn feature_manifest_from_layer(bytes: &[u8], media_type: &str) -> Result<Value, String> {
    let reader = feature_layer_reader(bytes, media_type);
    let mut archive = Archive::new(reader);
    for entry in archive.entries().map_err(|error| error.to_string())? {
        let mut entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path().map_err(|error| error.to_string())?;
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "devcontainer-feature.json")
        {
            let mut contents = String::new();
            entry
                .read_to_string(&mut contents)
                .map_err(|error| error.to_string())?;
            return serde_json::from_str(&contents).map_err(|error| error.to_string());
        }
    }
    Err("OCI Feature layer does not contain devcontainer-feature.json".to_string())
}

fn extract_feature_layer(bytes: &[u8], media_type: &str, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    let reader = feature_layer_reader(bytes, media_type);
    let mut archive = Archive::new(reader);
    for entry in archive.entries().map_err(|error| error.to_string())? {
        let mut entry = entry.map_err(|error| error.to_string())?;
        let relative_path = safe_archive_path(&entry.path().map_err(|error| error.to_string())?)?;
        if relative_path.as_os_str().is_empty() {
            continue;
        }
        let destination_path = destination.join(relative_path);
        let entry_type = entry.header().entry_type();
        if entry_type.is_dir() {
            fs::create_dir_all(&destination_path).map_err(|error| error.to_string())?;
        } else if entry_type.is_file() {
            if let Some(parent) = destination_path.parent() {
                fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            let mode = entry.header().mode().map_err(|error| error.to_string())?;
            {
                let mut output =
                    fs::File::create(&destination_path).map_err(|error| error.to_string())?;
                io::copy(&mut entry, &mut output).map_err(|error| error.to_string())?;
            }
            set_archive_file_mode(&destination_path, mode)?;
        } else {
            return Err(format!(
                "OCI Feature layer contains unsupported archive entry: {}",
                destination_path.display()
            ));
        }
    }
    Ok(())
}

fn set_archive_file_mode(path: &Path, mode: u32) -> Result<(), String> {
    #[cfg(unix)]
    {
        fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o7777))
            .map_err(|error| error.to_string())
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}

fn feature_layer_reader<'a>(bytes: &'a [u8], media_type: &str) -> Box<dyn Read + 'a> {
    if media_type.contains("gzip") || bytes.starts_with(&[0x1f, 0x8b]) {
        return Box::new(GzDecoder::new(Cursor::new(bytes)));
    }
    Box::new(Cursor::new(bytes))
}

fn safe_archive_path(path: &Path) -> Result<PathBuf, String> {
    let mut safe = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => safe.push(value),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "OCI Feature layer contains unsafe archive path: {}",
                    path.display()
                ));
            }
        }
    }
    Ok(safe)
}

fn verify_manifest_digest(
    parsed: &OciReference,
    header_digest: Option<String>,
    bytes: &[u8],
) -> Result<String, String> {
    let computed = format!("sha256:{}", sha256_digest(bytes));
    if let Some(header_digest) = header_digest {
        if header_digest != computed {
            return Err(format!(
                "OCI registry manifest digest mismatch for {}: header {header_digest}, body {computed}",
                parsed.original
            ));
        }
    }
    if let Some(expected) = &parsed.digest {
        if expected != &computed {
            return Err(format!(
                "OCI registry manifest digest mismatch for {}: expected {expected}, got {computed}",
                parsed.original
            ));
        }
    }
    Ok(computed)
}

fn verify_digest(expected: &str, bytes: &[u8], label: &str) -> Result<(), String> {
    let computed = format!("sha256:{}", sha256_digest(bytes));
    if expected == computed {
        Ok(())
    } else {
        Err(format!(
            "{label} digest mismatch: expected {expected}, got {computed}"
        ))
    }
}

fn sha256_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn reference_without_selector(reference: &str) -> String {
    if let Some(index) = reference.find('@') {
        return reference[..index].to_string();
    }
    let last_slash = reference.rfind('/').unwrap_or(0);
    if let Some(index) = reference.rfind(':').filter(|index| *index > last_slash) {
        return reference[..index].to_string();
    }
    reference.to_string()
}

fn is_registry_qualified_resource(resource: &str) -> bool {
    let Some((registry, _)) = resource.split_once('/') else {
        return false;
    };
    registry.contains('.') || registry.contains(':') || registry == "localhost"
}

fn configured_authorization(registry: &str) -> Option<String> {
    configured_bearer_authorization(registry).or_else(|| configured_basic_authorization(registry))
}

fn configured_bearer_authorization(registry: &str) -> Option<String> {
    let config = docker_config_auth(registry)?;
    config.identity_token.map(|token| format!("Bearer {token}"))
}

fn configured_basic_authorization(registry: &str) -> Option<String> {
    if let Some(auth) = env_oci_auth(registry) {
        return Some(auth);
    }
    if registry == "ghcr.io" {
        if let Ok(token) = env::var("GITHUB_TOKEN") {
            if !token.is_empty() {
                return Some(basic_authorization("x-access-token", &token));
            }
        }
    }
    docker_config_auth(registry).and_then(|auth| match (auth.username, auth.secret) {
        (Some(username), Some(secret)) => Some(basic_authorization(&username, &secret)),
        _ => None,
    })
}

fn env_oci_auth(registry: &str) -> Option<String> {
    let raw = env::var("DEVCONTAINERS_OCI_AUTH").ok()?;
    let mut parts = raw.splitn(3, '|');
    let configured_registry = parts.next()?;
    let username = parts.next()?;
    let token = parts.next()?;
    (configured_registry == registry).then(|| basic_authorization(username, token))
}

#[derive(Default)]
struct RegistryAuth {
    username: Option<String>,
    secret: Option<String>,
    identity_token: Option<String>,
}

fn docker_config_auth(registry: &str) -> Option<RegistryAuth> {
    let config_path = docker_config_path()?;
    let config: Value = serde_json::from_str(&fs::read_to_string(config_path).ok()?).ok()?;
    if let Some(helper) = config["credHelpers"][registry].as_str() {
        if let Some(auth) = credential_helper_auth(helper, registry) {
            return Some(auth);
        }
    }
    if let Some(helper) = config["credsStore"].as_str() {
        if let Some(auth) = credential_helper_auth(helper, registry) {
            return Some(auth);
        }
    }
    for key in registry_config_keys(registry) {
        if let Some(entry) = config["auths"].get(&key) {
            if let Some(token) = entry["identitytoken"].as_str() {
                return Some(RegistryAuth {
                    identity_token: Some(token.to_string()),
                    ..RegistryAuth::default()
                });
            }
            if let Some(auth) = entry["auth"].as_str() {
                if let Ok(decoded) = BASE64.decode(auth) {
                    if let Ok(decoded) = String::from_utf8(decoded) {
                        if let Some((username, secret)) = decoded.split_once(':') {
                            return Some(RegistryAuth {
                                username: Some(username.to_string()),
                                secret: Some(secret.to_string()),
                                identity_token: None,
                            });
                        }
                    }
                }
            }
            if let (Some(username), Some(secret)) =
                (entry["username"].as_str(), entry["password"].as_str())
            {
                return Some(RegistryAuth {
                    username: Some(username.to_string()),
                    secret: Some(secret.to_string()),
                    identity_token: None,
                });
            }
        }
    }
    platform_default_credential_helper().and_then(|helper| credential_helper_auth(helper, registry))
}

fn docker_config_path() -> Option<PathBuf> {
    if let Ok(config_dir) = env::var("DOCKER_CONFIG") {
        return Some(PathBuf::from(config_dir).join("config.json"));
    }
    home_dir().map(|home| home.join(".docker").join("config.json"))
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

fn registry_config_keys(registry: &str) -> Vec<String> {
    vec![
        registry.to_string(),
        format!("https://{registry}"),
        format!("https://{registry}/v1/"),
    ]
}

fn platform_default_credential_helper() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        Some("osxkeychain")
    } else if cfg!(target_os = "windows") {
        Some("wincred")
    } else {
        None
    }
}

fn credential_helper_auth(helper: &str, registry: &str) -> Option<RegistryAuth> {
    let program = format!("docker-credential-{helper}");
    let mut child = Command::new(program)
        .arg("get")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.as_mut()?.write_all(registry.as_bytes()).ok()?;
    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        return None;
    }
    let payload: Value = serde_json::from_slice(&output.stdout).ok()?;
    Some(RegistryAuth {
        username: payload["Username"].as_str().map(str::to_string),
        secret: payload["Secret"].as_str().map(str::to_string),
        identity_token: None,
    })
}

fn basic_authorization(username: &str, secret: &str) -> String {
    format!(
        "Basic {}",
        BASE64.encode(format!("{username}:{secret}").as_bytes())
    )
}

fn fixture_feature_artifact(parsed: &OciReference) -> Result<Option<OciFeatureArtifact>, String> {
    if !fixture_registry_enabled(&parsed.resource) {
        return Ok(None);
    }
    let selected_entry = select_fixture_catalog_entry(parsed);
    let fixture_reference = selected_entry
        .as_ref()
        .map(|entry| format!("{}:{}", parsed.resource, entry.version))
        .unwrap_or_else(|| {
            if super::registry::published_feature_manifest(&parsed.original).is_some()
                || is_registry_qualified_reference(&parsed.original)
            {
                parsed.original.clone()
            } else {
                parsed.resource.clone()
            }
        });
    let Some(mut metadata) = super::registry::published_feature_manifest(&fixture_reference)
        .or_else(|| synthetic_fixture_manifest(parsed, selected_entry.as_ref()))
    else {
        return Ok(None);
    };
    if let Some(entry) = &selected_entry {
        if let Some(object) = metadata.as_object_mut() {
            object.insert("version".to_string(), Value::String(entry.version.clone()));
        }
    }
    let manifest = generated_feature_oci_manifest(&fixture_reference, &metadata)?;
    let manifest_digest = selected_entry
        .as_ref()
        .map(|entry| entry.integrity.clone())
        .or_else(|| {
            super::registry::published_feature_manifest_digest(&parsed.original).map(str::to_string)
        })
        .unwrap_or_else(|| {
            serde_json::to_vec(&manifest)
                .map(|bytes| format!("sha256:{}", sha256_digest(&bytes)))
                .unwrap_or_default()
        });
    if let Some(expected) = &parsed.digest {
        if expected != &manifest_digest {
            return Err(format!(
                "OCI registry manifest digest mismatch for {}: expected {expected}, got {manifest_digest}",
                parsed.original
            ));
        }
    }
    Ok(Some(OciFeatureArtifact {
        original_reference: parsed.original.clone(),
        resource: parsed.resource.clone(),
        registry: parsed.registry.clone(),
        repository: parsed.repository.clone(),
        tag: selected_entry
            .map(|entry| entry.version)
            .or_else(|| parsed.tag.clone())
            .or_else(|| Some("latest".to_string())),
        reference_digest: parsed.digest.clone(),
        manifest_digest,
        manifest,
        metadata,
        layer: OciFeatureLayer::Generated {
            install_script: super::registry::published_feature_install_script(&parsed.original)
                .to_string(),
        },
    }))
}

fn select_fixture_catalog_entry(parsed: &OciReference) -> Option<OciCatalogEntry> {
    let entries = fixture_catalog_entries(&parsed.resource);
    if entries.is_empty() {
        return None;
    }
    if let Some(digest) = &parsed.digest {
        return entries.into_iter().find(|entry| entry.integrity == *digest);
    }
    let tag = parsed.tag.as_deref().unwrap_or("latest");
    if tag == "latest" {
        return entries
            .into_iter()
            .max_by(|left, right| compare_versions_asc(&left.version, &right.version));
    }
    if exact_semver(tag).is_some() {
        return entries.into_iter().find(|entry| entry.version == tag);
    }
    let selector = VersionSelector::parse(tag)?;
    entries
        .into_iter()
        .filter(|entry| selector.matches(&entry.version))
        .max_by(|left, right| compare_versions_asc(&left.version, &right.version))
}

fn synthetic_fixture_manifest(
    parsed: &OciReference,
    selected_entry: Option<&OciCatalogEntry>,
) -> Option<Value> {
    if parsed.resource != "ghcr.io/codspace/doesnotexist" {
        return None;
    }
    Some(json!({
        "id": "doesnotexist",
        "name": "Doesnotexist",
        "version": selected_entry
            .map(|entry| entry.version.as_str())
            .or(parsed.tag.as_deref())
            .unwrap_or("latest"),
        "options": {},
    }))
}

fn generated_feature_oci_manifest(feature_id: &str, metadata: &Value) -> Result<Value, String> {
    let metadata = serde_json::to_string(metadata).map_err(|error| error.to_string())?;
    let config_bytes = metadata.as_bytes();
    let install_script = super::registry::published_feature_install_script(feature_id).as_bytes();
    let slug =
        super::registry::collection_slug(feature_id).unwrap_or_else(|| "feature".to_string());
    Ok(json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {
            "mediaType": "application/vnd.devcontainers",
            "digest": format!("sha256:{}", sha256_digest(config_bytes)),
            "size": config_bytes.len(),
        },
        "layers": [{
            "mediaType": "application/vnd.devcontainers.layer.v1+tar",
            "digest": format!("sha256:{}", sha256_digest(install_script)),
            "size": install_script.len(),
            "annotations": {
                "org.opencontainers.image.title": format!("devcontainer-feature-{slug}.tgz"),
            },
        }],
        "annotations": {
            "dev.containers.metadata": metadata,
            "com.github.package.type": "devcontainer_feature",
            "org.opencontainers.image.ref.name": feature_id,
        },
    }))
}

fn fixture_registry_enabled(resource: &str) -> bool {
    resource.starts_with("ghcr.io/devcontainers/features/")
        || resource.starts_with("ghcr.io/codspace/")
}

fn fixture_tags(resource: &str) -> Option<Vec<String>> {
    let mut entries = fixture_catalog_entries(resource)
        .into_iter()
        .map(|entry| entry.version)
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| compare_versions_desc(left, right));
    entries.dedup();
    (!entries.is_empty()).then_some(entries)
}

fn fixture_catalog_entries(resource: &str) -> Vec<OciCatalogEntry> {
    match resource {
        "ghcr.io/devcontainers/features/git" => vec![
            fixture_entry(
                resource,
                "1.2.0",
                "sha256:1111111111111111111111111111111111111111111111111111111111111111",
                None,
            ),
            fixture_entry(
                resource,
                "1.1.5",
                "sha256:2ab83ca71d55d5c00a1255b07f3a83a53cd2de77ce8b9637abad38095d672a5b",
                None,
            ),
            fixture_entry(
                resource,
                "1.0.5",
                "sha256:2222222222222222222222222222222222222222222222222222222222222222",
                None,
            ),
            fixture_entry(
                resource,
                "1.0.4",
                "sha256:0bb490abcc0a3fb23937d29e2c18a225b51c5584edc0d9eb4131569a980f60b6",
                None,
            ),
        ],
        "ghcr.io/devcontainers/features/git-lfs" => vec![fixture_entry(
            resource,
            "1.0.6",
            "sha256:24d5802c837b2519b666a8403a9514c7296d769c9607048e9f1e040e7d7e331c",
            None,
        )],
        "ghcr.io/devcontainers/features/github-cli" => vec![fixture_entry(
            resource,
            "1.0.9",
            "sha256:9024deeca80347dea7603a3bb5b4951988f0bf5894ba036a6ee3f29c025692c6",
            None,
        )],
        "ghcr.io/devcontainers/features/azure-cli" => vec![fixture_entry(
            resource,
            "1.2.1",
            "sha256:a00aa292592a8df58a940d6f6dfcf2bfd3efab145f62a17ccb12656528793134",
            None,
        )],
        "ghcr.io/codspace/versioning/foo" => vec![
            fixture_entry(
                resource,
                "2.11.1",
                "sha256:3333333333333333333333333333333333333333333333333333333333333333",
                None,
            ),
            fixture_entry(
                resource,
                "0.3.1",
                "sha256:4444444444444444444444444444444444444444444444444444444444444444",
                None,
            ),
        ],
        "ghcr.io/codspace/versioning/bar" => vec![fixture_entry(
            resource,
            "1.0.0",
            "sha256:5555555555555555555555555555555555555555555555555555555555555555",
            None,
        )],
        "ghcr.io/codspace/doesnotexist" => vec![fixture_entry(
            resource,
            "0.1.2",
            "sha256:6666666666666666666666666666666666666666666666666666666666666666",
            None,
        )],
        "ghcr.io/codspace/dependson/a" => vec![fixture_entry(
            resource,
            "2.0.1",
            "sha256:932027ef71da186210e6ceb3294c3459caaf6b548d2b547d5d26be3fc4b2264a",
            Some(vec!["ghcr.io/codspace/dependson/E".to_string()]),
        )],
        "ghcr.io/codspace/dependson/e" => vec![
            fixture_entry(
                resource,
                "2.0.0",
                "sha256:9f36f159c70f8bebff57f341904b030733adb17ef12a5d58d4b3d89b2a6c7d5a",
                None,
            ),
            fixture_entry(
                resource,
                "1.0.0",
                "sha256:90b84127edab28ecb169cd6c6f2101ce0ea1d77589cee01951fec7f879f3a11c",
                None,
            ),
        ],
        _ => Vec::new(),
    }
}

fn fixture_entry(
    _resource: &str,
    version: &str,
    digest: &str,
    _depends_on: Option<Vec<String>>,
) -> OciCatalogEntry {
    OciCatalogEntry {
        version: version.to_string(),
        integrity: digest.to_string(),
    }
}

fn exact_semver(input: &str) -> Option<ParsedVersion> {
    let parts = input
        .split('.')
        .map(|part| part.parse::<u64>().ok())
        .collect::<Option<Vec<_>>>()?;
    match parts.as_slice() {
        [major, minor, patch] => Some(ParsedVersion {
            major: *major,
            minor: *minor,
            patch: *patch,
        }),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ParsedVersion {
    major: u64,
    minor: u64,
    patch: u64,
}

enum VersionSelector {
    Major(u64),
    MajorMinor(u64, u64),
}

impl VersionSelector {
    fn parse(input: &str) -> Option<Self> {
        let parts = input
            .split('.')
            .map(|part| part.parse::<u64>().ok())
            .collect::<Option<Vec<_>>>()?;
        match parts.as_slice() {
            [major] => Some(Self::Major(*major)),
            [major, minor] => Some(Self::MajorMinor(*major, *minor)),
            _ => None,
        }
    }

    fn matches(&self, version: &str) -> bool {
        let Some(parsed) = exact_semver(version) else {
            return false;
        };
        match self {
            Self::Major(major) => parsed.major == *major,
            Self::MajorMinor(major, minor) => parsed.major == *major && parsed.minor == *minor,
        }
    }
}

fn compare_versions_asc(left: &str, right: &str) -> Ordering {
    match (exact_semver(left), exact_semver(right)) {
        (Some(left), Some(right)) => left.cmp(&right),
        _ => left.cmp(right),
    }
}

fn compare_versions_desc(left: &str, right: &str) -> Ordering {
    compare_versions_asc(right, left)
}

type Ordering = std::cmp::Ordering;

impl OciTransport for CurlTransport {
    fn get(&self, url: &str, headers: &[(String, String)]) -> Result<OciHttpResponse, String> {
        let temp = TempHttpFiles::new();
        let mut args = vec![
            "-sSL".to_string(),
            "--max-time".to_string(),
            "30".to_string(),
            "-D".to_string(),
            temp.headers.display().to_string(),
            "-o".to_string(),
            temp.body.display().to_string(),
            "-w".to_string(),
            "%{http_code}".to_string(),
        ];
        for (name, value) in headers {
            args.push("-H".to_string());
            args.push(format!("{name}: {value}"));
        }
        args.push(url.to_string());

        let result = process_runner::run_process(&ProcessRequest {
            program: "curl".to_string(),
            args,
            cwd: None,
            env: HashMap::new(),
            log_level: ProcessLogLevel::Info,
        })
        .map_err(|error| error.to_string())?;
        if result.status_code != 0 {
            return Err(result.stderr);
        }
        let status = result
            .stdout
            .trim()
            .parse::<u16>()
            .map_err(|error| format!("curl did not return an HTTP status code: {error}"))?;
        let raw_headers = fs::read_to_string(&temp.headers).map_err(|error| error.to_string())?;
        let body = fs::read(&temp.body).map_err(|error| error.to_string())?;
        Ok(OciHttpResponse {
            status,
            headers: parse_http_headers(&raw_headers),
            body,
        })
    }
}

struct TempHttpFiles {
    headers: PathBuf,
    body: PathBuf,
}

impl TempHttpFiles {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nonce = COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let base = env::temp_dir().join(format!(
            "devcontainer-oci-{}-{now}-{nonce}",
            std::process::id()
        ));
        Self {
            headers: base.with_extension("headers"),
            body: base.with_extension("body"),
        }
    }
}

impl Drop for TempHttpFiles {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.headers);
        let _ = fs::remove_file(&self.body);
    }
}

fn parse_http_headers(raw_headers: &str) -> HashMap<String, String> {
    let normalized = raw_headers.replace("\r\n", "\n");
    let last_block = normalized
        .split("\n\n")
        .filter(|block| !block.trim().is_empty())
        .last()
        .unwrap_or("");
    last_block
        .lines()
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    use flate2::write::GzEncoder;
    use flate2::Compression;
    use serde_json::json;
    use tar::{Builder, Header};

    use super::{
        extract_feature_layer, feature_ref_json, parse_oci_reference,
        resolve_feature_artifact_for_reference, OciHttpResponse, OciReference, OciTransport,
    };

    #[derive(Clone, Default)]
    struct FakeTransport {
        routes: Arc<Mutex<HashMap<String, Vec<OciHttpResponse>>>>,
        seen_authorization: Arc<Mutex<Vec<Option<String>>>>,
    }

    impl FakeTransport {
        fn add(&self, url: &str, response: OciHttpResponse) {
            self.routes
                .lock()
                .expect("routes")
                .entry(url.to_string())
                .or_default()
                .push(response);
        }
    }

    impl OciTransport for FakeTransport {
        fn get(&self, url: &str, headers: &[(String, String)]) -> Result<OciHttpResponse, String> {
            self.seen_authorization.lock().expect("seen").push(
                headers
                    .iter()
                    .find(|(name, _)| name == "Authorization")
                    .map(|(_, value)| value.clone()),
            );
            self.routes
                .lock()
                .expect("routes")
                .get_mut(url)
                .and_then(|responses| {
                    if responses.is_empty() {
                        None
                    } else {
                        Some(responses.remove(0))
                    }
                })
                .ok_or_else(|| format!("missing fake route: {url}"))
        }
    }

    fn manifest_response(manifest: &serde_json::Value) -> OciHttpResponse {
        let body = serde_json::to_vec(manifest).expect("manifest bytes");
        let digest = format!("sha256:{}", super::sha256_digest(&body));
        OciHttpResponse {
            status: 200,
            headers: HashMap::from([("docker-content-digest".to_string(), digest)]),
            body,
        }
    }

    fn layer_bytes(gzip: bool) -> Vec<u8> {
        let mut archive = Vec::new();
        {
            let writer: Box<dyn Write> = if gzip {
                Box::new(GzEncoder::new(&mut archive, Compression::default()))
            } else {
                Box::new(&mut archive)
            };
            let mut builder = Builder::new(writer);
            append_file(
                &mut builder,
                "devcontainer-feature.json",
                br#"{"id":"fake","version":"1.0.0"}"#,
            );
            append_file(&mut builder, "install.sh", b"#!/bin/sh\nset -eu\n");
            append_file(&mut builder, "repo/data.txt", b"data");
            builder.finish().expect("finish archive");
        }
        archive
    }

    fn append_file<W: Write>(builder: &mut Builder<W>, path: &str, bytes: &[u8]) {
        append_file_with_mode(builder, path, bytes, 0o644);
    }

    fn append_file_with_mode<W: Write>(
        builder: &mut Builder<W>,
        path: &str,
        bytes: &[u8],
        mode: u32,
    ) {
        let mut header = Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(mode);
        header.set_cksum();
        builder
            .append_data(&mut header, path, bytes)
            .expect("append file");
    }

    #[test]
    fn parses_registry_refs_without_features_segment_and_with_ports() {
        let parsed =
            parse_oci_reference("ghcr.io/jooh/offline-apt-devcontainer-feature/offline-apt:1.0.0")
                .expect("parsed");
        assert_eq!(parsed.registry, "ghcr.io");
        assert_eq!(
            parsed.repository,
            "jooh/offline-apt-devcontainer-feature/offline-apt"
        );
        assert_eq!(parsed.tag.as_deref(), Some("1.0.0"));

        let parsed =
            parse_oci_reference("localhost:5000/acme/features/foo@sha256:abc").expect("parsed");
        assert_eq!(parsed.registry, "localhost:5000");
        assert_eq!(parsed.repository, "acme/features/foo");
        assert_eq!(parsed.digest.as_deref(), Some("sha256:abc"));
    }

    #[test]
    fn resolves_manifest_after_bearer_auth_retry() {
        let transport = FakeTransport::default();
        let reference = OciReference {
            original: "ghcr.io/acme/features/fake:1".to_string(),
            resource: "ghcr.io/acme/features/fake".to_string(),
            registry: "ghcr.io".to_string(),
            repository: "acme/features/fake".to_string(),
            tag: Some("1".to_string()),
            digest: None,
        };
        transport.add(
            "https://ghcr.io/v2/acme/features/fake/tags/list",
            OciHttpResponse {
                status: 401,
                headers: HashMap::from([(
                    "www-authenticate".to_string(),
                    "Bearer realm=\"https://ghcr.io/token\",service=\"ghcr.io\",scope=\"repository:acme/features/fake:pull\"".to_string(),
                )]),
                body: Vec::new(),
            },
        );
        transport.add(
            "https://ghcr.io/token?service=ghcr.io&scope=repository:acme/features/fake:pull",
            OciHttpResponse {
                status: 200,
                headers: HashMap::new(),
                body: br#"{"token":"token-1"}"#.to_vec(),
            },
        );
        transport.add(
            "https://ghcr.io/v2/acme/features/fake/tags/list",
            OciHttpResponse {
                status: 200,
                headers: HashMap::new(),
                body: br#"{"tags":["1.0.0","1.2.0","2.0.0"]}"#.to_vec(),
            },
        );
        let layer = layer_bytes(false);
        let layer_digest = format!("sha256:{}", super::sha256_digest(&layer));
        let metadata = json!({"id":"fake","version":"1.2.0"});
        let manifest = json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "annotations": {
                "dev.containers.metadata": metadata.to_string(),
            },
            "layers": [{
                "mediaType": "application/vnd.devcontainers.layer.v1+tar",
                "digest": layer_digest,
            }],
        });
        transport.add(
            "https://ghcr.io/v2/acme/features/fake/manifests/1.2.0",
            manifest_response(&manifest),
        );

        let artifact =
            resolve_feature_artifact_for_reference(&reference, None, &transport).expect("artifact");

        assert_eq!(artifact.tag.as_deref(), Some("1.2.0"));
        assert_eq!(artifact.metadata["id"], "fake");
        assert!(transport
            .seen_authorization
            .lock()
            .expect("seen")
            .iter()
            .any(|header| header.as_deref() == Some("Bearer token-1")));
    }

    #[test]
    fn resolves_non_semver_tags_as_exact_registry_references() {
        let transport = FakeTransport::default();
        let reference = OciReference {
            original: "ghcr.io/acme/features/fake:dev".to_string(),
            resource: "ghcr.io/acme/features/fake".to_string(),
            registry: "ghcr.io".to_string(),
            repository: "acme/features/fake".to_string(),
            tag: Some("dev".to_string()),
            digest: None,
        };
        let metadata = json!({"id":"fake","version":"dev"});
        let manifest = json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "annotations": {
                "dev.containers.metadata": metadata.to_string(),
            },
            "layers": [],
        });
        transport.add(
            "https://ghcr.io/v2/acme/features/fake/manifests/dev",
            manifest_response(&manifest),
        );

        let artifact =
            resolve_feature_artifact_for_reference(&reference, None, &transport).expect("artifact");

        assert_eq!(artifact.tag.as_deref(), Some("dev"));
        assert_eq!(artifact.metadata["id"], "fake");
    }

    #[test]
    fn fixture_artifact_rejects_unmatched_digest_pin() {
        let reference = parse_oci_reference(
            "ghcr.io/devcontainers/features/common-utils@sha256:0000000000000000000000000000000000000000000000000000000000000000",
        )
        .expect("reference");

        let error =
            resolve_feature_artifact_for_reference(&reference, None, &FakeTransport::default())
                .expect_err("digest mismatch");

        assert!(error.contains("digest mismatch"), "{error}");
    }

    #[test]
    fn feature_ref_digest_is_only_serialized_for_digest_pinned_references() {
        let tag_reference =
            parse_oci_reference("ghcr.io/devcontainers/features/azure-cli:1").expect("reference");
        let tag_artifact =
            resolve_feature_artifact_for_reference(&tag_reference, None, &FakeTransport::default())
                .expect("tag artifact");

        let tag_feature_ref = feature_ref_json(&tag_artifact);

        assert!(tag_feature_ref
            .as_object()
            .expect("featureRef object")
            .get("digest")
            .is_none());

        let digest = "sha256:a00aa292592a8df58a940d6f6dfcf2bfd3efab145f62a17ccb12656528793134";
        let digest_reference = parse_oci_reference(&format!(
            "ghcr.io/devcontainers/features/azure-cli@{digest}"
        ))
        .expect("reference");
        let digest_artifact = resolve_feature_artifact_for_reference(
            &digest_reference,
            None,
            &FakeTransport::default(),
        )
        .expect("digest artifact");

        assert_eq!(feature_ref_json(&digest_artifact)["digest"], digest);
    }

    #[test]
    fn rejects_manifest_digest_mismatch() {
        let transport = FakeTransport::default();
        let reference = OciReference {
            original: "ghcr.io/acme/features/fake@sha256:bad".to_string(),
            resource: "ghcr.io/acme/features/fake".to_string(),
            registry: "ghcr.io".to_string(),
            repository: "acme/features/fake".to_string(),
            tag: None,
            digest: Some("sha256:bad".to_string()),
        };
        transport.add(
            "https://ghcr.io/v2/acme/features/fake/manifests/sha256:bad",
            manifest_response(&json!({"schemaVersion": 2, "layers": []})),
        );

        let error =
            resolve_feature_artifact_for_reference(&reference, None, &transport).expect_err("err");

        assert!(error.contains("digest mismatch"), "{error}");
    }

    #[test]
    fn extracts_plain_and_gzip_feature_layers_safely() {
        for (gzip, media_type) in [
            (false, "application/vnd.devcontainers.layer.v1+tar"),
            (true, "application/vnd.devcontainers.layer.v1+tar+gzip"),
        ] {
            let destination = crate::test_support::unique_temp_dir("devcontainer-oci-extract-test");
            extract_feature_layer(&layer_bytes(gzip), media_type, &destination).expect("extract");
            assert!(destination.join("devcontainer-feature.json").is_file());
            assert!(destination.join("install.sh").is_file());
            assert!(destination.join("repo").join("data.txt").is_file());
            let _ = std::fs::remove_dir_all(destination);
        }
    }

    #[cfg(unix)]
    #[test]
    fn extract_feature_layer_preserves_file_modes() {
        use std::os::unix::fs::PermissionsExt;

        let mut archive = Vec::new();
        {
            let mut builder = Builder::new(&mut archive);
            append_file_with_mode(&mut builder, "bin/helper", b"#!/bin/sh\n", 0o755);
            builder.finish().expect("finish archive");
        }
        let destination = crate::test_support::unique_temp_dir("devcontainer-oci-mode-test");

        extract_feature_layer(
            &archive,
            "application/vnd.devcontainers.layer.v1+tar",
            &destination,
        )
        .expect("extract");

        let mode = std::fs::metadata(destination.join("bin").join("helper"))
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o755);
        let _ = std::fs::remove_dir_all(destination);
    }
}
