//! Registry lookup helpers for bundled collections and published features.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::process_runner::{self, ProcessLogLevel, ProcessRequest};

pub(super) fn embedded_template_source_dir(reference: &str) -> Option<PathBuf> {
    let slug = collection_slug(reference)?;
    match slug.as_str() {
        "alpine" | "cpp" | "mytemplate" | "node-mongo" => Some(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join(
                    "../../upstream/src/test/container-templates/example-templates-sets/simple/src",
                )
                .join(slug),
        ),
        _ => None,
    }
}

pub(super) struct LocalOciArtifact {
    pub metadata: Value,
    pub layer_path: Option<PathBuf>,
}

pub(crate) struct LiveRegistryManifest {
    pub manifest: Value,
    pub digest: String,
}

pub(crate) fn published_feature_install_script(feature_id: &str) -> &'static str {
    match normalize_collection_reference(feature_id).as_str() {
        "ghcr.io/devcontainers/features/common-utils" => {
            r#"#!/bin/sh
set -eu

username="${USERNAME:-}"
if [ -n "$username" ] && [ "$username" != "none" ] && ! id -u "$username" >/dev/null 2>&1; then
    if command -v useradd >/dev/null 2>&1; then
        useradd -m "$username" >/dev/null 2>&1 || true
    elif command -v adduser >/dev/null 2>&1; then
        adduser -D "$username" >/dev/null 2>&1 || adduser --disabled-password --gecos "" "$username" >/dev/null 2>&1 || true
    fi
fi
"#
        }
        _ => {
            r#"#!/bin/sh
set -eu
"#
        }
    }
}

pub(crate) fn published_feature_manifest(feature_id: &str) -> Option<Value> {
    let normalized = normalize_collection_reference(feature_id);
    let normalized_lower = normalized.to_ascii_lowercase();
    let manifest = match normalized_lower.as_str() {
        "ghcr.io/devcontainers/features/azure-cli" => Some(json!({
            "id": "azure-cli",
            "name": "Azure CLI",
            "version": "1.2.1",
            "options": { "version": { "type": "string", "default": "latest" } }
        })),
        "ghcr.io/devcontainers/features/common-utils" => Some(json!({
            "id": "common-utils",
            "name": "Common Utilities",
            "version": "2.5.4",
            "options": {
                "installZsh": { "type": "string", "default": "true" },
                "upgradePackages": { "type": "string", "default": "true" }
            }
        })),
        "ghcr.io/devcontainers/features/feature-with-advisory" => Some(json!({
            "id": "feature-with-advisory",
            "name": "Feature With Advisory",
            "version": "1.0.9",
            "options": {}
        })),
        "ghcr.io/devcontainers/features/docker-from-docker" => Some(json!({
            "id": "docker-from-docker",
            "name": "Docker from Docker",
            "version": "2.12.4",
            "options": {
                "version": { "type": "string", "default": "latest" },
                "moby": { "type": "string", "default": "true" },
                "enableNonRootDocker": { "type": "string", "default": "true" }
            }
        })),
        "ghcr.io/devcontainers/features/docker-in-docker" => Some(json!({
            "id": "docker-in-docker",
            "name": "Docker in Docker",
            "version": "2.12.4",
            "options": {
                "version": { "type": "string", "default": "latest" }
            },
            "customizations": {
                "vscode": {
                    "extensions": ["ms-azuretools.vscode-docker"]
                }
            }
        })),
        "ghcr.io/devcontainers/features/github-cli" => Some(json!({
            "id": "github-cli",
            "name": "GitHub CLI",
            "version": "1.0.9",
            "options": {}
        })),
        "node" => Some(json!({
            "id": "node",
            "name": "Node.js",
            "version": "1.6.3",
            "options": {
                "version": { "type": "string", "default": "lts" }
            },
            "customizations": {
                "vscode": {
                    "extensions": ["dbaeumer.vscode-eslint"]
                }
            }
        })),
        "java" | "ghcr.io/devcontainers/features/java" => Some(json!({
            "id": "java",
            "name": "Java",
            "version": "1.6.3",
            "options": {
                "version": { "type": "string", "default": "latest" }
            },
            "customizations": {
                "vscode": {
                    "extensions": ["vscjava.vscode-java-pack"],
                    "settings": {
                        "java.server.launchMode": "Standard"
                    }
                }
            }
        })),
        "ghcr.io/codspace/dependson/a" => Some(json!({
            "id": "A",
            "name": "FeatureA",
            "version": "2.0.1",
            "dependsOn": {
                "ghcr.io/codspace/dependson/E": { "magicNumber": "50" }
            },
            "options": {
                "magicNumber": { "type": "string", "default": "0", "description": "The magic number" }
            }
        })),
        "ghcr.io/codspace/dependson/b" => Some(json!({
            "id": "B",
            "name": "FeatureB",
            "version": "2.0.0",
            "dependsOn": {
                "ghcr.io/codspace/dependson/C": { "magicNumber": "20" },
                "ghcr.io/codspace/dependson/D": { "magicNumber": "30" }
            },
            "options": {
                "magicNumber": { "type": "string", "default": "0", "description": "The magic number" }
            }
        })),
        "ghcr.io/codspace/dependson/c" => Some(json!({
            "id": "C",
            "name": "FeatureC",
            "version": "2.0.0",
            "dependsOn": {
                "ghcr.io/codspace/dependson/A": { "magicNumber": "40" },
                "ghcr.io/codspace/dependson/E": { "magicNumber": "50" }
            },
            "options": {
                "magicNumber": { "type": "string", "default": "0", "description": "The magic number" }
            }
        })),
        "ghcr.io/codspace/dependson/d" => Some(json!({
            "id": "D",
            "name": "FeatureD",
            "version": "2.0.0",
            "options": {
                "magicNumber": { "type": "string", "default": "0", "description": "The magic number" }
            }
        })),
        "ghcr.io/codspace/dependson/e" => Some(json!({
            "id": "E",
            "name": "FeatureE",
            "version": "2.0.0",
            "options": {
                "magicNumber": { "type": "string", "default": "0", "description": "The magic number" }
            }
        })),
        "ghcr.io/devcontainers/features/python" => Some(json!({
            "id": "python",
            "name": "Python",
            "version": "1.8.1",
            "options": {
                "version": { "type": "string", "default": "latest" }
            }
        })),
        "ghcr.io/codspace/features/python" => Some(json!({
            "id": "python",
            "name": "Python",
            "version": "1.0.0",
            "options": {
                "version": { "type": "string", "default": "latest" }
            }
        })),
        _ => None,
    };
    if manifest.is_some() {
        return manifest;
    }

    let slug = collection_slug(&normalized)?;
    if !normalized.contains("/features/") {
        return None;
    }
    Some(json!({
        "id": slug,
        "name": humanize_collection_slug(&slug),
        "version": collection_reference_version(feature_id),
        "options": {},
    }))
}

pub(crate) fn direct_tarball_feature_manifest(feature_id: &str) -> Option<Value> {
    match feature_id {
        "https://github.com/codspace/tgz-features-with-dependson/releases/download/0.0.2/devcontainer-feature-A.tgz" => Some(json!({
            "id": "A",
            "name": "FeatureA",
            "version": "0.0.2",
            "dependsOn": {
                "ghcr.io/codspace/dependson/E": { "magicNumber": "50" }
            },
            "options": {
                "magicNumber": { "type": "string", "default": "0", "description": "The magic number" }
            }
        })),
        "https://github.com/codspace/tgz-features-with-dependson/releases/download/0.0.2/devcontainer-feature-B.tgz" => Some(json!({
            "id": "B",
            "name": "FeatureB",
            "version": "0.0.2",
            "dependsOn": {
                "ghcr.io/codspace/dependson/C": { "magicNumber": "20" },
                "ghcr.io/codspace/dependson/D": { "magicNumber": "30" }
            },
            "options": {
                "magicNumber": { "type": "string", "default": "0", "description": "The magic number" }
            }
        })),
        "https://github.com/codspace/features/releases/download/tarball02/devcontainer-feature-docker-in-docker.tgz" => Some(json!({
            "id": "docker-in-docker",
            "name": "Docker in Docker",
            "version": "0.0.2",
            "options": {
                "version": { "type": "string", "default": "latest" }
            }
        })),
        _ => None,
    }
}

pub(crate) fn published_feature_manifest_digest(feature_id: &str) -> Option<&'static str> {
    let normalized = normalize_collection_reference(feature_id).to_ascii_lowercase();
    match normalized.as_str() {
        "ghcr.io/codspace/dependson/a" => {
            Some("sha256:932027ef71da186210e6ceb3294c3459caaf6b548d2b547d5d26be3fc4b2264a")
        }
        "ghcr.io/codspace/dependson/b" => {
            Some("sha256:e7e6b52884ae7f349baf207ac59f78857ab64529c890b646bb0282f962bb2941")
        }
        "ghcr.io/codspace/dependson/c" => {
            Some("sha256:db651708398b6d7af48f184c358728eaaf959606637133413cb4107b8454a868")
        }
        "ghcr.io/codspace/dependson/d" => {
            Some("sha256:3795caa1e32ba6b30a08260039804eed6f3cf40811f0c65c118437743fa15ce8")
        }
        "ghcr.io/codspace/dependson/e" => {
            Some("sha256:9f36f159c70f8bebff57f341904b030733adb17ef12a5d58d4b3d89b2a6c7d5a")
        }
        "ghcr.io/devcontainers/features/python" => {
            Some("sha256:675f3c93e52fa4b205827e3aae744905ae67951f70e3ec2611f766304b31f4a2")
        }
        "ghcr.io/codspace/features/python" => {
            Some("sha256:e4034c2a24d6c5d1cc0f6cb03091fc72d4e89f5cc64fa692cb69b671c81633d2")
        }
        _ => None,
    }
}

pub(crate) fn published_feature_oci_manifest(feature_id: &str) -> Option<Value> {
    let normalized = normalize_collection_reference(feature_id);
    let feature_manifest = published_feature_manifest(feature_id)?;
    let metadata = serde_json::to_string(&feature_manifest).ok()?;
    let config_bytes = metadata.as_bytes();
    let layer_title = format!("devcontainer-feature-{}.tgz", collection_slug(&normalized)?);
    let layer_bytes = published_feature_install_script(feature_id).as_bytes();

    Some(json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {
            "mediaType": "application/vnd.devcontainers",
            "digest": format!("sha256:{}", sha256_digest(config_bytes)),
            "size": config_bytes.len(),
        },
        "layers": [{
            "mediaType": "application/vnd.devcontainers.layer.v1+tar",
            "digest": format!("sha256:{}", sha256_digest(layer_bytes)),
            "size": layer_bytes.len(),
            "annotations": {
                "org.opencontainers.image.title": layer_title,
            }
        }],
        "annotations": {
            "dev.containers.metadata": metadata,
            "com.github.package.type": "devcontainer_feature",
            "org.opencontainers.image.ref.name": feature_id,
        }
    }))
}

pub(crate) fn live_ghcr_feature_manifest(
    feature_id: &str,
) -> Result<Option<LiveRegistryManifest>, String> {
    if !live_ghcr_enabled() || !feature_id.starts_with("ghcr.io/") {
        return Ok(None);
    }

    let normalized = normalize_collection_reference(feature_id);
    if !normalized.contains("/features/") {
        return Ok(None);
    }

    let repository = normalized
        .strip_prefix("ghcr.io/")
        .ok_or_else(|| format!("Unsupported GHCR feature reference: {feature_id}"))?;
    let reference = collection_reference_version(feature_id);
    let manifest_url = format!("https://ghcr.io/v2/{repository}/manifests/{reference}");
    let headers = fetch_http_headers(&manifest_url, None, Some(OCI_MANIFEST_ACCEPT))?;
    let challenge = headers
        .headers
        .get("www-authenticate")
        .cloned()
        .ok_or_else(|| format!("GHCR did not return an auth challenge for {feature_id}"))?;
    let token = fetch_bearer_token(&challenge)?;
    let response = fetch_json_response(
        &manifest_url,
        Some(&format!("Bearer {token}")),
        Some(OCI_MANIFEST_ACCEPT),
    )?;
    let digest = response
        .headers
        .get("docker-content-digest")
        .cloned()
        .ok_or_else(|| format!("GHCR manifest response for {feature_id} is missing a digest"))?;

    Ok(Some(LiveRegistryManifest {
        manifest: response.body,
        digest,
    }))
}

pub(super) fn published_template_manifest_with_workspace(
    template_id: &str,
    workspace_folder: Option<&Path>,
) -> Option<Value> {
    if let Some(artifact) = local_oci_artifact(template_id, workspace_folder) {
        return Some(artifact.metadata);
    }

    let normalized = normalize_collection_reference(template_id);
    let manifest = match normalized.as_str() {
        "ghcr.io/devcontainers/templates/docker-from-docker" => Some(json!({
            "id": "docker-from-docker",
            "name": "Docker from Docker",
            "description": "Create a dev container with Docker available inside the container.",
            "version": "1.0.0"
        })),
        _ => embedded_template_manifest(&normalized),
    };
    if manifest.is_some() {
        return manifest;
    }

    let slug = collection_slug(&normalized)?;
    if !normalized.contains("/templates/") {
        return None;
    }
    Some(json!({
        "id": slug,
        "name": humanize_collection_slug(&slug),
        "description": "",
        "version": collection_reference_version(template_id),
    }))
}

pub(super) fn local_oci_artifact(
    reference: &str,
    workspace_folder: Option<&Path>,
) -> Option<LocalOciArtifact> {
    let layout_dir = workspace_oci_layout_dir(reference, workspace_folder)?;
    let manifest_digest = resolve_local_oci_manifest_digest(reference, &layout_dir)?;
    let manifest = read_local_oci_blob_json(&layout_dir, &manifest_digest)?;
    let metadata = manifest["annotations"]["dev.containers.metadata"]
        .as_str()
        .and_then(|value| serde_json::from_str::<Value>(value).ok())?;
    let layer_path = manifest["layers"]
        .as_array()
        .and_then(|layers| layers.first())
        .and_then(|layer| layer["digest"].as_str())
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .map(|digest| layout_dir.join("blobs").join("sha256").join(digest));
    Some(LocalOciArtifact {
        metadata,
        layer_path,
    })
}

pub(crate) fn normalize_collection_reference(reference: &str) -> String {
    if let Some(index) = reference.find('@') {
        return reference[..index].to_string();
    }
    let last_slash = reference.rfind('/').unwrap_or(0);
    if let Some(index) = reference.rfind(':').filter(|index| *index > last_slash) {
        return reference[..index].to_string();
    }
    reference.to_string()
}

pub(crate) fn collection_slug(reference: &str) -> Option<String> {
    normalize_collection_reference(reference)
        .rsplit('/')
        .next()
        .map(|value| value.to_ascii_lowercase())
}

pub(crate) fn collection_reference_version(reference: &str) -> String {
    let normalized = normalize_collection_reference(reference);
    if let Some(digest) = reference
        .strip_prefix(&normalized)
        .and_then(|suffix| suffix.strip_prefix('@'))
    {
        return digest.to_string();
    }
    if let Some(version) = reference
        .strip_prefix(&normalized)
        .and_then(|suffix| suffix.strip_prefix(':'))
    {
        return version.to_string();
    }
    "latest".to_string()
}

pub(super) fn humanize_collection_slug(slug: &str) -> String {
    slug.split('-')
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            let mut chars = segment.chars();
            match chars.next() {
                Some(first) => {
                    format!("{}{}", first.to_ascii_uppercase(), chars.as_str())
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn sha256_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

const OCI_MANIFEST_ACCEPT: &str =
    "application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json";

fn live_ghcr_enabled() -> bool {
    matches!(
        env::var("DEVCONTAINER_ENABLE_LIVE_GHCR").ok().as_deref(),
        Some("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

struct HttpHeaders {
    headers: HashMap<String, String>,
}

struct JsonHttpResponse {
    headers: HashMap<String, String>,
    body: Value,
}

fn fetch_bearer_token(challenge: &str) -> Result<String, String> {
    let challenge = challenge
        .strip_prefix("Bearer ")
        .ok_or_else(|| format!("Unsupported auth challenge: {challenge}"))?;
    let parameters = challenge
        .split(',')
        .filter_map(|entry| entry.split_once('='))
        .map(|(key, value)| {
            (
                key.trim().to_string(),
                value.trim().trim_matches('"').to_string(),
            )
        })
        .collect::<HashMap<_, _>>();
    let realm = parameters
        .get("realm")
        .ok_or_else(|| format!("Auth challenge is missing a realm: {challenge}"))?;
    let service = parameters
        .get("service")
        .ok_or_else(|| format!("Auth challenge is missing a service: {challenge}"))?;
    let scope = parameters
        .get("scope")
        .ok_or_else(|| format!("Auth challenge is missing a scope: {challenge}"))?;
    let token_url = format!("{realm}?service={service}&scope={scope}");
    let response = run_curl(&[
        "-fsSL".to_string(),
        "--max-time".to_string(),
        "15".to_string(),
        token_url,
    ])?;
    let payload: Value = serde_json::from_str(&response).map_err(|error| error.to_string())?;
    payload["token"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "GHCR token response did not include a token".to_string())
}

fn fetch_http_headers(
    url: &str,
    authorization: Option<&str>,
    accept: Option<&str>,
) -> Result<HttpHeaders, String> {
    let mut args = vec![
        "-sSI".to_string(),
        "--max-time".to_string(),
        "15".to_string(),
    ];
    if let Some(accept) = accept {
        args.push("-H".to_string());
        args.push(format!("Accept: {accept}"));
    }
    if let Some(authorization) = authorization {
        args.push("-H".to_string());
        args.push(format!("Authorization: {authorization}"));
    }
    args.push(url.to_string());

    let output = run_curl(&args)?;
    Ok(HttpHeaders {
        headers: parse_http_headers(&output),
    })
}

fn fetch_json_response(
    url: &str,
    authorization: Option<&str>,
    accept: Option<&str>,
) -> Result<JsonHttpResponse, String> {
    let mut args = vec![
        "-fsSL".to_string(),
        "-D".to_string(),
        "-".to_string(),
        "--max-time".to_string(),
        "15".to_string(),
    ];
    if let Some(accept) = accept {
        args.push("-H".to_string());
        args.push(format!("Accept: {accept}"));
    }
    if let Some(authorization) = authorization {
        args.push("-H".to_string());
        args.push(format!("Authorization: {authorization}"));
    }
    args.push(url.to_string());

    let output = run_curl(&args)?;
    let (raw_headers, body) = split_http_response(&output)?;
    let body = serde_json::from_str(body.trim()).map_err(|error| error.to_string())?;
    Ok(JsonHttpResponse {
        headers: parse_http_headers(raw_headers),
        body,
    })
}

fn split_http_response(response: &str) -> Result<(&str, &str), String> {
    response
        .split_once("\r\n\r\n")
        .or_else(|| response.split_once("\n\n"))
        .ok_or_else(|| "Malformed HTTP response".to_string())
}

fn parse_http_headers(raw_headers: &str) -> HashMap<String, String> {
    raw_headers
        .lines()
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect()
}

fn run_curl(args: &[String]) -> Result<String, String> {
    let result = process_runner::run_process(&ProcessRequest {
        program: "curl".to_string(),
        args: args.to_vec(),
        cwd: None,
        env: HashMap::new(),
        log_level: ProcessLogLevel::Info,
    })
    .map_err(|error| error.to_string())?;
    if result.status_code != 0 {
        return Err(result.stderr);
    }
    Ok(result.stdout)
}

fn workspace_oci_layout_dir(reference: &str, workspace_folder: Option<&Path>) -> Option<PathBuf> {
    let layout_dir = workspace_folder?
        .join(".devcontainer")
        .join("oci-layouts")
        .join(normalize_collection_reference(reference));
    layout_dir
        .join("oci-layout")
        .is_file()
        .then_some(layout_dir)
}

fn resolve_local_oci_manifest_digest(reference: &str, layout_dir: &Path) -> Option<String> {
    if let Some((_, digest)) = reference.rsplit_once("@sha256:") {
        return Some(digest.to_string());
    }

    let wanted_tag = collection_reference_version(reference);
    let index: Value =
        serde_json::from_str(&fs::read_to_string(layout_dir.join("index.json")).ok()?).ok()?;
    index["manifests"].as_array()?.iter().find_map(|entry| {
        let tag = entry["annotations"]["org.opencontainers.image.ref.name"].as_str()?;
        (tag == wanted_tag)
            .then(|| {
                entry["digest"]
                    .as_str()?
                    .strip_prefix("sha256:")
                    .map(str::to_string)
            })
            .flatten()
    })
}

fn read_local_oci_blob_json(layout_dir: &Path, digest: &str) -> Option<Value> {
    serde_json::from_str(
        &fs::read_to_string(layout_dir.join("blobs").join("sha256").join(digest)).ok()?,
    )
    .ok()
}

fn embedded_template_manifest(reference: &str) -> Option<Value> {
    match collection_slug(reference)?.as_str() {
        "alpine" => Some(json!({
            "id": "alpine",
            "version": "1.0.0",
            "name": "Alpine",
            "options": {
                "imageVariant": {
                    "type": "string",
                    "description": "Alpine version:",
                    "proposals": ["3.16", "3.15", "3.14", "3.13"],
                    "default": "3.16"
                }
            },
            "platforms": ["Any"]
        })),
        "cpp" => Some(json!({
            "id": "cpp",
            "version": "1.0.0",
            "name": "C++",
            "options": {
                "imageVariant": {
                    "type": "string",
                    "description": "Debian / Ubuntu version (use Debian 11, Ubuntu 18.04/22.04 on local arm64/Apple Silicon):",
                    "proposals": [
                        "debian-11",
                        "debian-10",
                        "ubuntu-22.04",
                        "ubuntu-20.04",
                        "ubuntu-18.04"
                    ],
                    "default": "debian-11"
                }
            },
            "platforms": ["C++"]
        })),
        "mytemplate" => Some(json!({
            "id": "mytemplate",
            "version": "1.0.0",
            "name": "My Template",
            "description": "Simple test",
            "documentationURL": "https://github.com/codspace/templates/tree/main/src/test",
            "publisher": "codspace",
            "licenseURL": "https://github.com/devcontainers/templates/blob/main/LICENSE",
            "platforms": ["Any"],
            "options": {
                "anOption": {
                    "type": "string",
                    "description": "A great option",
                    "proposals": ["8.0", "7.0", "6.0"],
                    "default": "8.0"
                },
                "userUid": {
                    "type": "string",
                    "description": "The user's UID",
                    "proposals": ["1000", "1001", "1002"],
                    "default": "1000"
                }
            },
            "optionalPaths": [".github/*", "example-projects/exampleA/*", "c1.ts"]
        })),
        "node-mongo" => Some(json!({
            "id": "node-mongo",
            "version": "1.0.0",
            "name": "Node.js & Mongo DB",
            "options": {
                "imageVariant": {
                    "type": "string",
                    "description": "Node.js version (use -bullseye variants on local arm64/Apple Silicon):",
                    "proposals": [
                        "18",
                        "16",
                        "14",
                        "18-bullseye",
                        "16-bullseye",
                        "14-bullseye",
                        "18-buster",
                        "16-buster",
                        "14-buster"
                    ],
                    "default": "16-bullseye"
                }
            },
            "platforms": ["Node.js", "JavaScript", "Mongo DB"]
        })),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::embedded_template_manifest;

    #[test]
    fn embedded_cpp_template_manifest_is_available() {
        let manifest = embedded_template_manifest("ghcr.io/devcontainers/templates/cpp:latest")
            .expect("cpp template manifest");

        assert_eq!(manifest["id"], "cpp");
        assert_eq!(manifest["name"], "C++");
        assert_eq!(manifest["options"]["imageVariant"]["default"], "debian-11");
    }
}
