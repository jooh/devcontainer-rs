//! Native OCI Distribution helpers for published devcontainer Feature artifacts.

#[cfg(test)]
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::fs::OpenOptions;
use std::io::{self, Cursor, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
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

use crate::commands::common;
use crate::process_runner::{self, ProcessLogLevel, ProcessRequest};

const OCI_MANIFEST_ACCEPT: &str =
    "application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json";
const OCI_BLOB_ACCEPT: &str = "application/octet-stream, application/vnd.devcontainers.layer.v1+tar, application/vnd.devcontainers.layer.v1+tar+gzip";

fn io_error_to_string(error: io::Error) -> String {
    error.to_string()
}

fn serde_json_error_to_string(error: serde_json::Error) -> String {
    error.to_string()
}

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

#[derive(Debug)]
struct OciHttpExchange {
    response: OciHttpResponse,
    response_url: String,
    redirected: bool,
}

trait OciTransport {
    fn get(&self, url: &str, headers: &[(String, String)]) -> Result<OciHttpResponse, String>;

    fn get_exchange(
        &self,
        url: &str,
        headers: &[(String, String)],
    ) -> Result<OciHttpExchange, String> {
        self.get(url, headers).map(|response| OciHttpExchange {
            response,
            response_url: url.to_string(),
            redirected: false,
        })
    }

    fn get_no_redirects(
        &self,
        url: &str,
        headers: &[(String, String)],
    ) -> Result<OciHttpResponse, String> {
        self.get(url, headers)
    }

    fn get_no_redirects_exchange(
        &self,
        url: &str,
        headers: &[(String, String)],
    ) -> Result<OciHttpExchange, String> {
        self.get_no_redirects(url, headers)
            .map(|response| OciHttpExchange {
                response,
                response_url: url.to_string(),
                redirected: false,
            })
    }

    fn post_no_redirects_exchange(
        &self,
        _url: &str,
        _headers: &[(String, String)],
        _body: &[u8],
    ) -> Result<OciHttpExchange, String> {
        Err("OCI transport does not support POST requests".to_string())
    }

    fn post_exchange(
        &self,
        _url: &str,
        _headers: &[(String, String)],
        _body: &[u8],
    ) -> Result<OciHttpExchange, String> {
        Err("OCI transport does not support POST requests".to_string())
    }
}

struct CurlTransport;

#[cfg(test)]
thread_local! {
    static TEST_TOOL_DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

#[cfg(test)]
fn replace_test_tool_dir(dir: Option<PathBuf>) -> Option<PathBuf> {
    TEST_TOOL_DIR.with(|cell| cell.replace(dir))
}

fn tool_program(name: &str) -> String {
    #[cfg(test)]
    {
        let path = TEST_TOOL_DIR.with(|cell| cell.borrow().as_ref().map(|dir| dir.join(name)));
        if let Some(path) = path {
            return path.display().to_string();
        }
    }

    name.to_string()
}

#[cfg(test)]
fn parse_oci_reference(input: &str) -> Option<OciReference> {
    Some(parse_oci_reference_value(input))
}

fn parse_oci_reference_value(input: &str) -> OciReference {
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
        let (registry, repository) = resource
            .split_once('/')
            .expect("OCI resource contains a registry and repository");
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

    OciReference {
        original: input.to_string(),
        resource,
        registry,
        repository,
        tag,
        digest,
    }
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
    let parsed = parse_oci_reference_value(reference);
    resolve_feature_artifact_for_reference(&parsed, workspace_folder, &CurlTransport)
}

pub(crate) fn resolve_feature_artifact_with_digest(
    reference: &str,
    manifest_digest: &str,
    workspace_folder: Option<&Path>,
) -> Result<OciFeatureArtifact, String> {
    let mut parsed = parse_oci_reference_value(reference);
    parsed.digest = Some(manifest_digest.to_string());
    resolve_feature_artifact_for_reference(&parsed, workspace_folder, &CurlTransport)
}

pub(crate) fn list_feature_tags(
    reference: &str,
    workspace_folder: Option<&Path>,
) -> Result<Vec<String>, String> {
    let parsed = parse_oci_reference_value(reference);
    if let Some(tags) = list_local_layout_tags(&parsed, workspace_folder)? {
        return Ok(tags);
    }
    if let Some(tags) = fixture_tags(&parsed.resource) {
        return Ok(tags);
    }
    registry_tags(&parsed, &CurlTransport)
}

pub(crate) fn feature_ref_json(artifact: &OciFeatureArtifact) -> Value {
    let id = match artifact.metadata.get("id").and_then(Value::as_str) {
        Some(id) => id,
        None => artifact.resource.rsplit('/').next().unwrap_or_default(),
    };
    let version = match artifact.metadata.get("version").and_then(Value::as_str) {
        Some(version) => version,
        None => artifact.tag.as_deref().unwrap_or("latest"),
    };
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
    materialize_feature_artifact_with_transport(artifact, destination, &CurlTransport)
}

fn materialize_feature_artifact_with_transport(
    artifact: &OciFeatureArtifact,
    destination: &Path,
    transport: &dyn OciTransport,
) -> Result<(), String> {
    match &artifact.layer {
        OciFeatureLayer::Registry { digest, media_type } => {
            let bytes = registry_blob(artifact, digest, transport)?;
            verify_digest(digest, &bytes, "Feature layer")?;
            extract_feature_layer(&bytes, media_type, destination)
        }
        OciFeatureLayer::LocalPath {
            digest,
            media_type,
            path,
        } => {
            let bytes = fs::read(path).map_err(io_error_to_string)?;
            verify_digest(digest, &bytes, "Feature layer")?;
            extract_feature_layer(&bytes, media_type, destination)
        }
        OciFeatureLayer::Generated { install_script } => {
            fs::create_dir_all(destination).map_err(io_error_to_string)?;
            fs::write(
                destination.join("devcontainer-feature.json"),
                serde_json::to_string_pretty(&artifact.metadata)
                    .map_err(serde_json_error_to_string)?,
            )
            .map_err(io_error_to_string)?;
            fs::write(destination.join("install.sh"), install_script).map_err(io_error_to_string)
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
        "{}://{}/v2/{}/manifests/{}",
        registry_scheme(&parsed.registry),
        parsed.registry,
        parsed.repository,
        manifest_reference
    );
    let accept_headers = [("Accept".to_string(), OCI_MANIFEST_ACCEPT.to_string())];
    let response = registry_get(transport, &parsed.registry, &manifest_url, &accept_headers)?;
    if response.status != 200 {
        return Err(format!(
            "OCI registry returned HTTP {} for manifest {}",
            response.status, parsed.original
        ));
    }
    let header_digest = response.headers.get("docker-content-digest").cloned();
    let manifest_digest = verify_manifest_digest(parsed, header_digest, &response.body)?;
    let manifest: Value = match serde_json::from_slice(&response.body) {
        Ok(manifest) => manifest,
        Err(error) => {
            return Err(format!(
                "OCI registry returned an invalid manifest for {}: {error}",
                parsed.original
            ));
        }
    };
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
    match tags
        .into_iter()
        .filter(|candidate| selector.matches(candidate))
        .max_by(|left, right| compare_versions_asc(left, right))
    {
        Some(tag) => Ok(tag),
        None => Err(format!(
            "No published versions of {} match selector {}",
            parsed.resource, tag
        )),
    }
}

fn registry_tags(
    parsed: &OciReference,
    transport: &dyn OciTransport,
) -> Result<Vec<String>, String> {
    let url = format!(
        "{}://{}/v2/{}/tags/list",
        registry_scheme(&parsed.registry),
        parsed.registry,
        parsed.repository
    );
    let response = registry_get(transport, &parsed.registry, &url, &[])?;
    if response.status != 200 {
        return Err(format!(
            "OCI registry returned HTTP {} for tag list {}",
            response.status, parsed.resource
        ));
    }
    let payload: Value = match serde_json::from_slice(&response.body) {
        Ok(payload) => payload,
        Err(error) => {
            return Err(format!(
                "OCI registry returned an invalid tag list for {}: {error}",
                parsed.resource
            ));
        }
    };
    let mut tags = Vec::new();
    if let Some(values) = payload["tags"].as_array() {
        for tag in values {
            if let Some(tag) = tag.as_str() {
                tags.push(tag.to_string());
            }
        }
    }
    Ok(tags)
}

fn registry_blob(
    artifact: &OciFeatureArtifact,
    digest: &str,
    transport: &dyn OciTransport,
) -> Result<Vec<u8>, String> {
    let url = format!(
        "{}://{}/v2/{}/blobs/{}",
        registry_scheme(&artifact.registry),
        artifact.registry,
        artifact.repository,
        digest
    );
    let accept_headers = [("Accept".to_string(), OCI_BLOB_ACCEPT.to_string())];
    let response = registry_get(transport, &artifact.registry, &url, &accept_headers)?;
    if response.status != 200 {
        return Err(format!(
            "OCI registry returned HTTP {} for blob {}",
            response.status, digest
        ));
    }
    Ok(response.body)
}

const BUILT_IN_CROSS_ORIGIN_AUTH_HOSTS: &[&str] = &[
    "registry-1.docker.io=auth.docker.io",
    "registry.docker.io=auth.docker.io",
    "docker.io=auth.docker.io",
    "index.docker.io=auth.docker.io",
    "registry.gitlab.com=gitlab.com",
];
const DOCKER_HUB_REGISTRY_HOSTS: &[&str] = &[
    "registry-1.docker.io",
    "registry.docker.io",
    "docker.io",
    "index.docker.io",
];

#[derive(Debug, Eq, PartialEq)]
struct ParsedHttpUrl {
    scheme: String,
    authority: String,
    hostname: String,
}

pub(crate) fn parse_cross_origin_auth_hosts(
    entries: &[String],
) -> Result<HashMap<String, HashSet<String>>, String> {
    let mut mappings = HashMap::<String, HashSet<String>>::new();
    for entry in entries {
        let Some((registry, auth_host)) = entry.split_once('=') else {
            return Err(invalid_cross_origin_auth_host(entry));
        };
        if registry.is_empty()
            || auth_host.is_empty()
            || auth_host.contains('=')
            || registry.contains('=')
        {
            return Err(invalid_cross_origin_auth_host(entry));
        }
        let registry = normalize_authority(registry, Some(443))
            .map_err(|_| invalid_cross_origin_auth_host(entry))?
            .0;
        let auth_host = normalize_authority(auth_host, Some(443))
            .map_err(|_| invalid_cross_origin_auth_host(entry))?
            .0;
        mappings.entry(registry).or_default().insert(auth_host);
    }
    Ok(mappings)
}

pub(crate) fn is_allowed_token_service_realm(
    realm: &str,
    registry_url: &str,
    configured_entries: &[String],
) -> bool {
    let Ok(realm) = parse_http_url(realm) else {
        return false;
    };
    let Ok(registry) = parse_http_url(registry_url) else {
        return false;
    };
    if realm.authority == registry.authority
        && (realm.scheme == "https" || realm.scheme == "http" && realm.hostname == "localhost")
    {
        return true;
    }
    if realm.scheme != "https" {
        return false;
    }

    let mut entries = BUILT_IN_CROSS_ORIGIN_AUTH_HOSTS
        .iter()
        .map(|entry| (*entry).to_string())
        .collect::<Vec<_>>();
    entries.extend_from_slice(configured_entries);
    parse_cross_origin_auth_hosts(&entries)
        .ok()
        .and_then(|mappings| mappings.get(&registry.authority).cloned())
        .is_some_and(|auth_hosts| auth_hosts.contains(&realm.authority))
}

fn invalid_cross_origin_auth_host(entry: &str) -> String {
    format!("Invalid cross-origin auth host '{entry}'. Expected '<registry-host>=<auth-host>'.")
}

fn parse_http_url(value: &str) -> Result<ParsedHttpUrl, String> {
    let Some((scheme, remainder)) = value.split_once("://") else {
        return Err(format!("Invalid URL: {value}"));
    };
    let scheme = scheme.to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https") {
        return Err(format!("Invalid URL scheme: {scheme}"));
    }
    let authority_end = remainder.find(['/', '?', '#']).unwrap_or(remainder.len());
    let authority = &remainder[..authority_end];
    let default_port = if scheme == "https" { 443 } else { 80 };
    let (authority, hostname) = normalize_authority(authority, Some(default_port))?;
    Ok(ParsedHttpUrl {
        scheme,
        authority,
        hostname,
    })
}

fn registry_scheme(registry: &str) -> &'static str {
    let authority = normalize_authority(registry, None);
    match authority {
        Ok((_, hostname)) if hostname.eq_ignore_ascii_case("localhost") => "http",
        _ => "https",
    }
}

fn is_oci_registry_origin(url: &str, registry: &str) -> bool {
    let Ok(candidate) = parse_http_url(url) else {
        return false;
    };
    let expected_url = format!("{}://{registry}/", registry_scheme(registry));
    let Ok(expected) = parse_http_url(&expected_url) else {
        return false;
    };
    if candidate.scheme == expected.scheme && candidate.authority == expected.authority {
        return true;
    }
    candidate.scheme == "https"
        && expected.scheme == "https"
        && DOCKER_HUB_REGISTRY_HOSTS.contains(&candidate.authority.as_str())
        && DOCKER_HUB_REGISTRY_HOSTS.contains(&expected.authority.as_str())
}

fn normalize_authority(
    authority: &str,
    default_port: Option<u16>,
) -> Result<(String, String), String> {
    if authority.is_empty()
        || authority
            .chars()
            .any(|character| character.is_whitespace() || "/?#@\\".contains(character))
    {
        return Err(format!("Invalid authority: {authority}"));
    }

    let (hostname, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let Some(closing) = bracketed.find(']') else {
            return Err(format!("Invalid authority: {authority}"));
        };
        let hostname = &bracketed[..closing];
        if hostname.is_empty()
            || !hostname.chars().all(|character| {
                character.is_ascii_hexdigit() || character == ':' || character == '.'
            })
        {
            return Err(format!("Invalid authority: {authority}"));
        }
        let suffix = &bracketed[closing + 1..];
        let port = if suffix.is_empty() {
            None
        } else {
            Some(
                suffix
                    .strip_prefix(':')
                    .ok_or_else(|| format!("Invalid authority: {authority}"))?,
            )
        };
        (format!("[{}]", hostname.to_ascii_lowercase()), port)
    } else {
        if authority.matches(':').count() > 1 {
            return Err(format!("Invalid authority: {authority}"));
        }
        let (hostname, port) = match authority.rsplit_once(':') {
            Some((hostname, port)) => (hostname, Some(port)),
            None => (authority, None),
        };
        if hostname.is_empty()
            || !hostname.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
            })
        {
            return Err(format!("Invalid authority: {authority}"));
        }
        (hostname.to_ascii_lowercase(), port)
    };

    let port = port
        .map(|value| {
            if value.is_empty() || !value.chars().all(|character| character.is_ascii_digit()) {
                return Err(format!("Invalid authority: {authority}"));
            }
            value
                .parse::<u16>()
                .map_err(|_| format!("Invalid authority: {authority}"))
        })
        .transpose()?;
    let normalized = match port.filter(|port| Some(*port) != default_port) {
        Some(port) => format!("{hostname}:{port}"),
        None => hostname.clone(),
    };
    Ok((normalized, hostname))
}

pub(crate) fn token_service_url(
    realm: &str,
    service: &str,
    scope: Option<&str>,
) -> Result<String, String> {
    parse_http_url(realm)?;
    let without_fragment = realm.split_once('#').map_or(realm, |(base, _)| base);
    let (base, query) = without_fragment
        .split_once('?')
        .map_or((without_fragment, ""), |(base, query)| (base, query));
    let mut parameters = query
        .split('&')
        .filter(|parameter| !parameter.is_empty())
        .filter(|parameter| {
            let key = parameter.split_once('=').map_or(*parameter, |(key, _)| key);
            !matches!(form_urldecode_component(key).as_str(), "service" | "scope")
        })
        .map(str::to_string)
        .collect::<Vec<_>>();
    parameters.push(format!("service={}", form_urlencode_component(service)));
    parameters.push(format!(
        "scope={}",
        form_urlencode_component(scope.unwrap_or_default())
    ));
    Ok(format!("{base}?{}", parameters.join("&")))
}

fn form_urldecode_component(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
            {
                decoded.push((high << 4) | low);
                index += 3;
                continue;
            }
        }
        decoded.push(if bytes[index] == b'+' {
            b' '
        } else {
            bytes[index]
        });
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn form_urlencode_component(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'*' | b'-' | b'.' | b'_') {
            encoded.push(char::from(byte));
        } else if byte == b' ' {
            encoded.push('+');
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    encoded
}

fn registry_get(
    transport: &dyn OciTransport,
    registry: &str,
    url: &str,
    headers: &[(String, String)],
) -> Result<OciHttpResponse, String> {
    common::mark_oci_auth_attempted();
    let auth_options = common::current_oci_auth_options();
    let requested_registry_origin = is_oci_registry_origin(url, registry);
    let safe_headers = headers
        .iter()
        .filter(|(name, _)| {
            !auth_options.hardening
                || requested_registry_origin
                || !name.eq_ignore_ascii_case("authorization")
        })
        .cloned()
        .collect::<Vec<_>>();
    let initial_exchange = transport.get_exchange(url, &safe_headers)?;
    if initial_exchange.response.status != 401 && initial_exchange.response.status != 403 {
        return Ok(initial_exchange.response);
    }

    let challenge_registry_origin =
        is_oci_registry_origin(&initial_exchange.response_url, registry);
    if !requested_registry_origin || !challenge_registry_origin {
        common::record_oci_auth_diagnostic(
            common::OciAuthDiagnostic::RegistryRedirectWouldPreventCredentialForwarding,
        );
    }

    let Some(challenge) = initial_exchange.response.headers.get("www-authenticate") else {
        return Ok(initial_exchange.response);
    };
    let credentials_allowed =
        !auth_options.hardening || requested_registry_origin && challenge_registry_origin;
    if challenge
        .split_whitespace()
        .next()
        .is_some_and(|method| method.eq_ignore_ascii_case("basic"))
    {
        if !credentials_allowed {
            return Ok(initial_exchange.response);
        }
        let Some(authorization) = configured_basic_authorization(registry) else {
            return Ok(initial_exchange.response);
        };
        let mut retry_headers = safe_headers.clone();
        retry_headers.push(("Authorization".to_string(), authorization));
        return transport.get(url, &retry_headers);
    }

    let basic = credentials_allowed
        .then(|| configured_basic_authorization(registry))
        .flatten();
    let refresh_token = credentials_allowed
        .then(|| configured_refresh_token(registry))
        .flatten();
    let token = fetch_bearer_token_for_registry_url(
        transport,
        registry,
        &initial_exchange.response_url,
        challenge,
        basic.as_deref(),
        refresh_token.as_deref(),
        credentials_allowed,
    )?;
    let mut retry_headers = safe_headers;
    retry_headers.push(("Authorization".to_string(), format!("Bearer {token}")));
    transport.get(url, &retry_headers)
}

#[cfg(test)]
fn fetch_bearer_token(
    transport: &dyn OciTransport,
    registry: &str,
    challenge: &str,
    basic_authorization: Option<&str>,
) -> Result<String, String> {
    let registry_url = format!("{}://{registry}/v2/", registry_scheme(registry));
    fetch_bearer_token_for_registry_url(
        transport,
        registry,
        &registry_url,
        challenge,
        basic_authorization,
        None,
        true,
    )
}

fn fetch_bearer_token_for_registry_url(
    transport: &dyn OciTransport,
    registry: &str,
    registry_url: &str,
    challenge: &str,
    basic_authorization: Option<&str>,
    refresh_token: Option<&str>,
    credentials_allowed: bool,
) -> Result<String, String> {
    let Some((method, challenge_value)) = challenge.split_once(' ') else {
        return Err(format!("Unsupported OCI auth challenge: {challenge}"));
    };
    if !method.eq_ignore_ascii_case("bearer") {
        return Err(format!("Unsupported OCI auth challenge: {challenge}"));
    }
    let challenge = challenge_value;
    let parameters = challenge_parameters(challenge);
    let realm = match parameters.get("realm") {
        Some(realm) => realm,
        None => {
            return Err(format!(
                "OCI auth challenge is missing a realm: {challenge}"
            ))
        }
    };
    let service = parameters
        .get("service")
        .cloned()
        .unwrap_or(registry.to_string());
    let auth_options = common::current_oci_auth_options();
    let realm_allowed = is_allowed_token_service_realm(
        realm,
        registry_url,
        &auth_options.allowed_cross_origin_auth_hosts,
    );
    if !realm_allowed {
        common::record_oci_auth_diagnostic(common::OciAuthDiagnostic::AuthLookupWouldBeBlocked);
    }
    if auth_options.hardening && !realm_allowed {
        let challenge_registry = parse_http_url(registry_url)
            .map(|url| url.authority)
            .unwrap_or_else(|_| registry.to_string());
        let hint = parse_http_url(realm)
            .ok()
            .filter(|realm| realm.scheme == "https")
            .map(|realm| {
                format!(
                    " Use '--allow-cross-origin-auth-host {challenge_registry}={}' to trust this registry-to-auth-host mapping.",
                    realm.authority
                )
            })
            .unwrap_or_default();
        return Err(format!(
            "Registry '{challenge_registry}' requested authentication from untrusted realm '{realm}'.{hint}"
        ));
    }
    let scope = parameters.get("scope").map(String::as_str);
    let token_url = token_service_url(realm, &service, scope)?;
    let refresh_token = credentials_allowed.then_some(refresh_token).flatten();
    let basic_authorization = credentials_allowed.then_some(basic_authorization).flatten();
    let sent_credentials = refresh_token.is_some() || basic_authorization.is_some();
    let mut exchange = if let Some(refresh_token) = refresh_token {
        let headers = [
            ("User-Agent".to_string(), "devcontainer".to_string()),
            (
                "Content-Type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            ),
        ];
        let body = refresh_token_exchange_body(&service, scope, refresh_token);
        if auth_options.hardening {
            transport.post_no_redirects_exchange(realm, &headers, body.as_bytes())?
        } else {
            transport.post_exchange(realm, &headers, body.as_bytes())?
        }
    } else {
        let mut headers = vec![("User-Agent".to_string(), "devcontainer".to_string())];
        if let Some(authorization) = basic_authorization {
            headers.push(("Authorization".to_string(), authorization.to_string()));
        }
        if auth_options.hardening {
            transport.get_no_redirects_exchange(&token_url, &headers)?
        } else {
            transport.get_exchange(&token_url, &headers)?
        }
    };
    if exchange.redirected {
        common::record_oci_auth_diagnostic(common::OciAuthDiagnostic::AuthServerRedirect);
    }
    if sent_credentials && matches!(exchange.response.status, 401 | 403) {
        let anonymous_headers = [("User-Agent".to_string(), "devcontainer".to_string())];
        exchange = if auth_options.hardening {
            transport.get_no_redirects_exchange(&token_url, &anonymous_headers)?
        } else {
            transport.get_exchange(&token_url, &anonymous_headers)?
        };
        if exchange.redirected {
            common::record_oci_auth_diagnostic(common::OciAuthDiagnostic::AuthServerRedirect);
        }
    }
    let response = exchange.response;
    if auth_options.hardening && (300..400).contains(&response.status) {
        return Err(format!(
            "OCI token service redirected a hardened authentication request for {registry}"
        ));
    }
    if !(200..300).contains(&response.status) {
        return Err(format!(
            "OCI token service returned HTTP {} for {registry}",
            response.status
        ));
    }
    let payload: Value = match serde_json::from_slice(&response.body) {
        Ok(payload) => payload,
        Err(error) => return Err(format!("OCI token service returned invalid JSON: {error}")),
    };
    if let Some(token) = payload["token"].as_str().filter(|token| !token.is_empty()) {
        return Ok(token.to_string());
    }
    if let Some(token) = payload["access_token"]
        .as_str()
        .filter(|token| !token.is_empty())
    {
        return Ok(token.to_string());
    }
    Err("OCI token service response did not include a token".to_string())
}

fn refresh_token_exchange_body(service: &str, scope: Option<&str>, refresh_token: &str) -> String {
    [
        ("client_id", "devcontainer"),
        ("grant_type", "refresh_token"),
        ("service", service),
        ("scope", scope.unwrap_or_default()),
        ("refresh_token", refresh_token),
    ]
    .into_iter()
    .map(|(name, value)| format!("{name}={}", form_urlencode_component(value)))
    .collect::<Vec<_>>()
    .join("&")
}

fn challenge_parameters(challenge: &str) -> HashMap<String, String> {
    let mut parameters = HashMap::new();
    for entry in split_challenge_parameters(challenge) {
        if let Some((key, value)) = entry.split_once('=') {
            parameters.insert(key.trim().to_string(), challenge_parameter_value(value));
        }
    }
    parameters
}

fn split_challenge_parameters(challenge: &str) -> Vec<&str> {
    let mut parameters = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    for (index, character) in challenge.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if quoted && character == '\\' {
            escaped = true;
        } else if character == '"' {
            quoted = !quoted;
        } else if character == ',' && !quoted {
            parameters.push(&challenge[start..index]);
            start = index + character.len_utf8();
        }
    }
    parameters.push(&challenge[start..]);
    parameters
}

fn challenge_parameter_value(raw_value: &str) -> String {
    let value = raw_value.trim();
    let Some(value) = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    else {
        return value.to_string();
    };

    let mut unescaped = String::with_capacity(value.len());
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character == '\\' {
            match characters.next() {
                Some(escaped) => unescaped.push(escaped),
                None => unescaped.push(character),
            }
        } else {
            unescaped.push(character);
        }
    }
    unescaped
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
    let manifest: Value = match serde_json::from_slice(&manifest_bytes) {
        Ok(manifest) => manifest,
        Err(error) => {
            return Err(format!(
                "OCI layout manifest {} is invalid JSON: {error}",
                manifest_digest
            ));
        }
    };
    let manifest_reference = tag.unwrap_or(manifest_digest.clone());
    let artifact = artifact_from_manifest(
        parsed,
        manifest_reference,
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
    let mut tags = Vec::new();
    for entry in local_layout_index_manifests(&layout_dir)? {
        if let Some(tag) = entry["annotations"]["org.opencontainers.image.ref.name"].as_str() {
            tags.push(tag.to_string());
        }
    }
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
        for entry in manifests {
            let entry_tag = entry["annotations"]["org.opencontainers.image.ref.name"].as_str();
            let digest = entry["digest"].as_str();
            if entry_tag == Some("latest") {
                return Ok(digest.map(|digest| (digest.to_string(), Some("latest".to_string()))));
            }
        }
        return Ok(None);
    }
    if exact_semver(tag).is_some() {
        for entry in manifests {
            if entry["annotations"]["org.opencontainers.image.ref.name"].as_str() == Some(tag) {
                return Ok(entry["digest"]
                    .as_str()
                    .map(|digest| (digest.to_string(), Some(tag.to_string()))));
            }
        }
        return Ok(None);
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
    let mut selected: Option<(String, String)> = None;
    for entry in manifests {
        let Some(entry_tag) = entry["annotations"]["org.opencontainers.image.ref.name"].as_str()
        else {
            continue;
        };
        if !selector.matches(entry_tag) {
            continue;
        }
        let Some(digest) = entry["digest"].as_str() else {
            continue;
        };
        let candidate = (entry_tag.to_string(), digest.to_string());
        let should_select = match &selected {
            Some(current) => compare_versions_asc(&current.0, &candidate.0).is_lt(),
            None => true,
        };
        if should_select {
            selected = Some(candidate);
        }
    }
    Ok(selected.map(|(tag, digest)| (digest, Some(tag))))
}

fn local_layout_index_manifests(layout_dir: &Path) -> Result<Vec<Value>, String> {
    let index: Value = serde_json::from_str(
        &fs::read_to_string(layout_dir.join("index.json")).map_err(io_error_to_string)?,
    )
    .map_err(serde_json_error_to_string)?;
    Ok(index["manifests"].as_array().cloned().unwrap_or_default())
}

fn workspace_oci_layout_dir(resource: &str, workspace_folder: Option<&Path>) -> Option<PathBuf> {
    let layout_dir = workspace_folder?
        .join(".devcontainer")
        .join("oci-layouts")
        .join(resource);
    if layout_dir.join("oci-layout").is_file() {
        Some(layout_dir)
    } else {
        None
    }
}

fn read_layout_blob(layout_dir: &Path, digest: &str) -> Result<Vec<u8>, String> {
    let Some(hex) = digest.strip_prefix("sha256:") else {
        return Err(format!("Unsupported OCI digest: {digest}"));
    };
    fs::read(layout_dir.join("blobs").join("sha256").join(hex)).map_err(io_error_to_string)
}

fn feature_layer(
    manifest: &Value,
    local_layout_dir: Option<&Path>,
) -> Result<OciFeatureLayer, String> {
    let Some(layers) = manifest["layers"].as_array() else {
        return Ok(OciFeatureLayer::Missing);
    };
    let mut feature_layer = None;
    for layer in layers {
        if layer["mediaType"].as_str().is_some_and(|media_type| {
            media_type.starts_with("application/vnd.devcontainers.layer.")
        }) {
            feature_layer = Some(layer);
            break;
        }
    }
    let Some(layer) = feature_layer else {
        return Ok(OciFeatureLayer::Missing);
    };
    let Some(digest) = layer["digest"].as_str() else {
        return Err("OCI Feature layer descriptor is missing a digest".to_string());
    };
    let digest = digest.to_string();
    let media_type = layer["mediaType"]
        .as_str()
        .unwrap_or("application/vnd.devcontainers.layer.v1+tar")
        .to_string();
    if let Some(layout_dir) = local_layout_dir {
        let Some(hex) = digest.strip_prefix("sha256:") else {
            return Err(format!("Unsupported OCI Feature layer digest: {digest}"));
        };
        let path = layout_dir.join("blobs").join("sha256").join(hex);
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
    match serde_json::from_str(raw) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) => Err(format!(
            "OCI Feature metadata annotation is invalid JSON: {error}"
        )),
    }
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
            let bytes = registry_blob(&placeholder, digest, transport)?;
            verify_digest(digest, &bytes, "Feature layer")?;
            (bytes, media_type.clone())
        }
        OciFeatureLayer::LocalPath {
            digest,
            media_type,
            path,
        } => {
            let bytes = fs::read(path).map_err(io_error_to_string)?;
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
    for entry in archive.entries().map_err(io_error_to_string)? {
        let mut entry = entry.map_err(io_error_to_string)?;
        let path = entry.path().map_err(io_error_to_string)?;
        let file_name = match path.file_name() {
            Some(name) => name.to_str(),
            None => None,
        };
        if file_name == Some("devcontainer-feature.json") {
            let mut contents = String::new();
            entry
                .read_to_string(&mut contents)
                .map_err(io_error_to_string)?;
            return serde_json::from_str(&contents).map_err(serde_json_error_to_string);
        }
    }
    Err("OCI Feature layer does not contain devcontainer-feature.json".to_string())
}

fn extract_feature_layer(bytes: &[u8], media_type: &str, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination).map_err(io_error_to_string)?;
    let reader = feature_layer_reader(bytes, media_type);
    let mut archive = Archive::new(reader);
    for entry in archive.entries().map_err(io_error_to_string)? {
        let mut entry = entry.map_err(io_error_to_string)?;
        let relative_path = safe_archive_path(&entry.path().map_err(io_error_to_string)?)?;
        if relative_path.as_os_str().is_empty() {
            continue;
        }
        let destination_path = destination.join(relative_path);
        let entry_type = entry.header().entry_type();
        if entry_type.is_dir() {
            fs::create_dir_all(&destination_path).map_err(io_error_to_string)?;
        } else if entry_type.is_file() {
            fs::create_dir_all(
                destination_path
                    .parent()
                    .expect("archive destination path has a parent"),
            )
            .map_err(io_error_to_string)?;
            let mode = entry.header().mode().map_err(io_error_to_string)?;
            {
                let mut output = fs::File::create(&destination_path).map_err(io_error_to_string)?;
                io::copy(&mut entry, &mut output).map_err(io_error_to_string)?;
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
            .map_err(io_error_to_string)
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
    if let Some(expected) = parsed
        .digest
        .as_deref()
        .filter(|expected| *expected != computed)
    {
        return Err(format!(
            "OCI registry manifest digest mismatch for {}: expected {expected}, got {computed}",
            parsed.original
        ));
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

#[cfg(test)]
fn configured_authorization(registry: &str) -> Option<String> {
    configured_basic_authorization(registry)
}

fn configured_refresh_token(registry: &str) -> Option<String> {
    if env_oci_auth(registry).is_some() {
        return None;
    }
    if registry == "ghcr.io" && env::var("GITHUB_TOKEN").is_ok_and(|token| !token.is_empty()) {
        return None;
    }
    docker_config_auth(registry)?.refresh_token
}

fn configured_basic_authorization(registry: &str) -> Option<String> {
    if let Some(auth) = env_oci_auth(registry) {
        return Some(auth);
    }
    if registry == "ghcr.io" {
        let token = env::var("GITHUB_TOKEN").unwrap_or_default();
        if !token.is_empty() {
            return Some(basic_authorization("x-access-token", &token));
        }
    }
    let auth = docker_config_auth(registry)?;
    match (auth.username, auth.secret) {
        (Some(username), Some(secret)) => Some(basic_authorization(&username, &secret)),
        _ => None,
    }
}

fn env_oci_auth(registry: &str) -> Option<String> {
    let raw = env::var("DEVCONTAINERS_OCI_AUTH").ok()?;
    let parts = raw.splitn(3, '|').collect::<Vec<_>>();
    let [configured_registry, username, token] = parts.as_slice() else {
        return None;
    };
    if *configured_registry == registry {
        Some(basic_authorization(username, token))
    } else {
        None
    }
}

#[derive(Default)]
struct RegistryAuth {
    username: Option<String>,
    secret: Option<String>,
    refresh_token: Option<String>,
}

fn docker_config_auth(registry: &str) -> Option<RegistryAuth> {
    let config_path = docker_config_path()?;
    let config: Value = serde_json::from_str(&fs::read_to_string(config_path).ok()?).ok()?;
    let credential_auth = config["credHelpers"][registry]
        .as_str()
        .and_then(|helper| credential_helper_auth(helper, registry))
        .or_else(|| {
            config["credsStore"]
                .as_str()
                .and_then(|helper| credential_helper_auth(helper, registry))
        });
    if credential_auth.is_some() {
        return credential_auth;
    }
    for key in registry_config_keys(registry) {
        if let Some(entry) = config["auths"].get(&key) {
            if let Some(token) = entry["identitytoken"].as_str() {
                return Some(RegistryAuth {
                    refresh_token: Some(token.to_string()),
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
                                refresh_token: None,
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
                    refresh_token: None,
                });
            }
        }
    }
    platform_default_credential_auth(registry)
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

#[cfg(any(test, target_os = "macos", target_os = "windows"))]
fn platform_default_credential_helper() -> Option<&'static str> {
    #[cfg(target_os = "macos")]
    {
        Some("osxkeychain")
    }
    #[cfg(target_os = "windows")]
    {
        Some("wincred")
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

fn platform_default_credential_auth(registry: &str) -> Option<RegistryAuth> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let helper = platform_default_credential_helper()?;
        credential_helper_auth(helper, registry)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = registry;
        None
    }
}

fn credential_helper_auth(helper: &str, registry: &str) -> Option<RegistryAuth> {
    let program = tool_program(&format!("docker-credential-{helper}"));
    let mut child = Command::new(program)
        .arg("get")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child
        .stdin
        .as_mut()
        .expect("credential helper stdin is piped")
        .write_all(registry.as_bytes())
        .ok()?;
    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        return None;
    }
    let payload: Value = serde_json::from_slice(&output.stdout).ok()?;
    if payload["Username"].as_str() == Some("<token>") {
        return Some(RegistryAuth {
            refresh_token: payload["Secret"].as_str().map(str::to_string),
            ..RegistryAuth::default()
        });
    }
    Some(RegistryAuth {
        username: payload["Username"].as_str().map(str::to_string),
        secret: payload["Secret"].as_str().map(str::to_string),
        refresh_token: None,
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
    let fixture_reference = match &selected_entry {
        Some(entry) => format!("{}:{}", parsed.resource, entry.version),
        None => {
            if super::registry::published_feature_manifest(&parsed.original).is_some()
                || is_registry_qualified_reference(&parsed.original)
            {
                parsed.original.clone()
            } else {
                parsed.resource.clone()
            }
        }
    };
    let mut metadata = match super::registry::published_feature_manifest(&fixture_reference) {
        Some(metadata) => metadata,
        None => match synthetic_fixture_manifest(parsed, selected_entry.as_ref()) {
            Some(metadata) => metadata,
            None => return Ok(None),
        },
    };
    if let Some(entry) = &selected_entry {
        let object = metadata
            .as_object_mut()
            .expect("fixture Feature metadata is a JSON object");
        object.insert("version".to_string(), Value::String(entry.version.clone()));
    }
    let manifest = generated_feature_oci_manifest(&fixture_reference, &metadata);
    let manifest_digest = if let Some(entry) = &selected_entry {
        entry.integrity.clone()
    } else if let Some(digest) =
        super::registry::published_feature_manifest_digest(&parsed.original)
    {
        digest.to_string()
    } else {
        serde_json::to_vec(&manifest)
            .map(|bytes| format!("sha256:{}", sha256_digest(&bytes)))
            .unwrap_or_default()
    };
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
        tag: match selected_entry {
            Some(entry) => Some(entry.version),
            None => Some(parsed.tag.clone().unwrap_or("latest".to_string())),
        },
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
        "version": match selected_entry {
            Some(entry) => entry.version.as_str(),
            None => parsed.tag.as_deref().unwrap_or("latest"),
        },
        "options": {},
    }))
}

fn generated_feature_oci_manifest(feature_id: &str, metadata: &Value) -> Value {
    let metadata = serde_json::to_string(metadata).expect("serializing JSON value cannot fail");
    let config_bytes = metadata.as_bytes();
    let install_script = super::registry::published_feature_install_script(feature_id).as_bytes();
    let slug = super::registry::collection_slug(feature_id).unwrap_or("feature".to_string());
    json!({
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
    })
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
    if entries.is_empty() {
        None
    } else {
        Some(entries)
    }
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
        self.request(url, headers, None, true)
            .map(|exchange| exchange.response)
    }

    fn get_exchange(
        &self,
        url: &str,
        headers: &[(String, String)],
    ) -> Result<OciHttpExchange, String> {
        self.request(url, headers, None, true)
    }

    fn get_no_redirects(
        &self,
        url: &str,
        headers: &[(String, String)],
    ) -> Result<OciHttpResponse, String> {
        self.request(url, headers, None, false)
            .map(|exchange| exchange.response)
    }

    fn get_no_redirects_exchange(
        &self,
        url: &str,
        headers: &[(String, String)],
    ) -> Result<OciHttpExchange, String> {
        self.request(url, headers, None, false)
    }

    fn post_no_redirects_exchange(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<OciHttpExchange, String> {
        self.request(url, headers, Some(body), false)
    }

    fn post_exchange(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<OciHttpExchange, String> {
        self.request(url, headers, Some(body), true)
    }
}

impl CurlTransport {
    fn request(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: Option<&[u8]>,
        follow_redirects: bool,
    ) -> Result<OciHttpExchange, String> {
        let temp = TempHttpFiles::new();
        write_private_file(&temp.headers, &[])?;
        write_private_file(&temp.body, &[])?;
        let mut args = vec![
            "-q".to_string(),
            "-sS".to_string(),
            "--max-time".to_string(),
            "30".to_string(),
            "-D".to_string(),
            temp.headers.display().to_string(),
            "-o".to_string(),
            temp.body.display().to_string(),
            "-w".to_string(),
            "%{http_code}\n%{url_effective}\n%{num_redirects}".to_string(),
        ];
        if follow_redirects {
            args.push("-L".to_string());
        }
        let mut request_headers = String::new();
        for (name, value) in headers {
            if name.contains(['\r', '\n']) || value.contains(['\r', '\n']) {
                return Err("OCI HTTP headers must not contain newlines".to_string());
            }
            request_headers.push_str(name);
            request_headers.push_str(": ");
            request_headers.push_str(value);
            request_headers.push('\n');
        }
        if !request_headers.is_empty() {
            write_private_file(&temp.request_headers, request_headers.as_bytes())?;
            args.push("-H".to_string());
            args.push(format!("@{}", temp.request_headers.display()));
        }
        if let Some(body) = body {
            write_private_file(&temp.request_body, body)?;
            args.push("--data-binary".to_string());
            args.push(format!("@{}", temp.request_body.display()));
        }
        args.push(url.to_string());

        let result = process_runner::run_process(&ProcessRequest {
            program: tool_program("curl"),
            args,
            cwd: None,
            env: HashMap::new(),
            log_level: ProcessLogLevel::Info,
        });
        let result = match result {
            Ok(result) => result,
            Err(error) => return Err(error.to_string()),
        };
        if result.status_code != 0 {
            return Err(result.stderr);
        }
        let mut write_out = result.stdout.lines();
        let status = match write_out.next().unwrap_or_default().trim().parse::<u16>() {
            Ok(status) => status,
            Err(error) => return Err(format!("curl did not return an HTTP status code: {error}")),
        };
        let response_url = write_out.next().unwrap_or(url).trim().to_string();
        let redirected = write_out
            .next()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .is_some_and(|count| count > 0);
        let raw_headers = fs::read_to_string(&temp.headers).map_err(io_error_to_string)?;
        let body = fs::read(&temp.body).map_err(io_error_to_string)?;
        Ok(OciHttpExchange {
            response: OciHttpResponse {
                status,
                headers: parse_http_headers(&raw_headers),
                body,
            },
            redirected,
            response_url,
        })
    }
}

fn write_private_file(path: &Path, contents: &[u8]) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).map_err(io_error_to_string)?;
    file.write_all(contents).map_err(io_error_to_string)
}

struct TempHttpFiles {
    headers: PathBuf,
    body: PathBuf,
    request_headers: PathBuf,
    request_body: PathBuf,
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
            request_headers: base.with_extension("request-headers"),
            request_body: base.with_extension("request-body"),
        }
    }
}

impl Drop for TempHttpFiles {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.headers);
        let _ = fs::remove_file(&self.body);
        let _ = fs::remove_file(&self.request_headers);
        let _ = fs::remove_file(&self.request_body);
    }
}

fn parse_http_headers(raw_headers: &str) -> HashMap<String, String> {
    let normalized = raw_headers.replace("\r\n", "\n");
    let last_block = normalized
        .split("\n\n")
        .filter(|block| !block.trim().is_empty())
        .last()
        .unwrap_or("");
    let mut headers = HashMap::new();
    for line in last_block.lines() {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    headers
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::env;
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use base64::Engine as _;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use serde_json::json;
    use tar::{Builder, Header};

    use super::{
        canonical_feature_id, challenge_parameters, compare_versions_asc, compare_versions_desc,
        configured_basic_authorization, credential_helper_auth, docker_config_auth, exact_semver,
        extract_feature_layer, feature_layer, feature_manifest_from_layer, feature_ref_json,
        fetch_bearer_token, fixture_feature_artifact, fixture_tags, is_allowed_token_service_realm,
        is_oci_registry_origin, is_registry_qualified_reference, list_feature_tags,
        local_layout_feature_artifact, local_layout_manifest_digest, materialize_feature_artifact,
        materialize_feature_artifact_with_transport, metadata_from_feature_layer,
        parse_cross_origin_auth_hosts, parse_http_headers, parse_oci_reference,
        platform_default_credential_helper, registry_blob, registry_config_keys,
        registry_feature_artifact, registry_get, registry_scheme, registry_tags,
        resolve_feature_artifact, resolve_feature_artifact_for_reference, safe_archive_path,
        token_service_url, verify_manifest_digest, CurlTransport, OciFeatureArtifact,
        OciFeatureLayer, OciHttpResponse, OciReference, OciTransport, VersionSelector, BASE64,
    };

    type RequestHeaders = Vec<(String, String)>;

    #[derive(Clone, Default)]
    struct FakeTransport {
        routes: Arc<Mutex<HashMap<String, Vec<OciHttpResponse>>>>,
        response_urls: Arc<Mutex<HashMap<String, Vec<String>>>>,
        seen_authorization: Arc<Mutex<Vec<Option<String>>>>,
        seen_headers: Arc<Mutex<Vec<RequestHeaders>>>,
        seen_methods: Arc<Mutex<Vec<String>>>,
        seen_bodies: Arc<Mutex<Vec<Vec<u8>>>>,
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

        fn add_redirected(&self, url: &str, response_url: &str, response: OciHttpResponse) {
            self.add(url, response);
            self.response_urls
                .lock()
                .expect("response URLs")
                .entry(url.to_string())
                .or_default()
                .push(response_url.to_string());
        }

        fn request(
            &self,
            method: &str,
            url: &str,
            headers: &[(String, String)],
            body: &[u8],
        ) -> Result<OciHttpResponse, String> {
            let authorization = headers
                .iter()
                .find(|(name, _)| name == "Authorization")
                .map(|(_, value)| value.clone());
            self.seen_authorization
                .lock()
                .expect("seen")
                .push(authorization);
            self.seen_headers
                .lock()
                .expect("headers")
                .push(headers.to_vec());
            self.seen_methods
                .lock()
                .expect("methods")
                .push(method.to_string());
            self.seen_bodies.lock().expect("bodies").push(body.to_vec());
            let response = {
                let mut routes = self.routes.lock().expect("routes");
                match routes.get_mut(url) {
                    Some(responses) if !responses.is_empty() => Some(responses.remove(0)),
                    _ => None,
                }
            };
            match response {
                Some(response) => Ok(response),
                None => Err(format!("missing fake route: {url}")),
            }
        }

        fn exchange(
            &self,
            method: &str,
            url: &str,
            headers: &[(String, String)],
            body: &[u8],
        ) -> Result<super::OciHttpExchange, String> {
            let response = self.request(method, url, headers, body)?;
            let response_url = self
                .response_urls
                .lock()
                .expect("response URLs")
                .get_mut(url)
                .and_then(|urls| (!urls.is_empty()).then(|| urls.remove(0)))
                .unwrap_or_else(|| url.to_string());
            Ok(super::OciHttpExchange {
                response,
                redirected: response_url != url,
                response_url,
            })
        }
    }

    impl OciTransport for FakeTransport {
        fn get(&self, url: &str, headers: &[(String, String)]) -> Result<OciHttpResponse, String> {
            self.request("GET", url, headers, &[])
        }

        fn get_exchange(
            &self,
            url: &str,
            headers: &[(String, String)],
        ) -> Result<super::OciHttpExchange, String> {
            self.exchange("GET", url, headers, &[])
        }

        fn get_no_redirects_exchange(
            &self,
            url: &str,
            headers: &[(String, String)],
        ) -> Result<super::OciHttpExchange, String> {
            self.exchange("GET", url, headers, &[])
        }

        fn post_no_redirects_exchange(
            &self,
            url: &str,
            headers: &[(String, String)],
            body: &[u8],
        ) -> Result<super::OciHttpExchange, String> {
            self.exchange("POST", url, headers, body)
        }

        fn post_exchange(
            &self,
            url: &str,
            headers: &[(String, String)],
            body: &[u8],
        ) -> Result<super::OciHttpExchange, String> {
            self.exchange("POST", url, headers, body)
        }
    }

    struct DefaultMethodTransport;

    impl OciTransport for DefaultMethodTransport {
        fn get(
            &self,
            _url: &str,
            _headers: &[(String, String)],
        ) -> Result<OciHttpResponse, String> {
            Ok(OciHttpResponse {
                status: 204,
                headers: HashMap::new(),
                body: Vec::new(),
            })
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
        layer_bytes_with_manifest(gzip, br#"{"id":"fake","version":"1.0.0"}"#)
    }

    fn layer_bytes_with_manifest(gzip: bool, manifest: &[u8]) -> Vec<u8> {
        let mut archive = Vec::new();
        {
            let writer: Box<dyn Write> = if gzip {
                Box::new(GzEncoder::new(&mut archive, Compression::default()))
            } else {
                Box::new(&mut archive)
            };
            let mut builder = Builder::new(writer);
            append_file(&mut builder, "devcontainer-feature.json", manifest);
            append_file(&mut builder, "install.sh", b"#!/bin/sh\nset -eu\n");
            append_file(&mut builder, "repo/data.txt", b"data");
            builder.finish().expect("finish archive");
        }
        archive
    }

    fn write_local_layout_version(
        workspace: &Path,
        resource: &str,
        tag: &str,
        metadata: serde_json::Value,
        layer: &[u8],
    ) -> String {
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

        let layer_digest = super::sha256_digest(layer);
        fs::write(
            layout_dir.join("blobs").join("sha256").join(&layer_digest),
            layer,
        )
        .expect("layer blob");
        let manifest = json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "layers": [{
                "mediaType": "application/vnd.devcontainers.layer.v1+tar",
                "digest": format!("sha256:{layer_digest}"),
                "size": layer.len(),
            }],
            "annotations": {
                "dev.containers.metadata": metadata.to_string(),
            },
        });
        let manifest_bytes = serde_json::to_vec_pretty(&manifest).expect("manifest bytes");
        let manifest_digest = super::sha256_digest(&manifest_bytes);
        fs::write(
            layout_dir
                .join("blobs")
                .join("sha256")
                .join(&manifest_digest),
            &manifest_bytes,
        )
        .expect("manifest blob");

        let mut manifests = if layout_dir.join("index.json").is_file() {
            serde_json::from_str::<serde_json::Value>(
                &fs::read_to_string(layout_dir.join("index.json")).expect("index"),
            )
            .expect("index json")["manifests"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        manifests.push(json!({
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "digest": format!("sha256:{manifest_digest}"),
            "size": manifest_bytes.len(),
            "annotations": {
                "org.opencontainers.image.ref.name": tag,
            },
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
        manifest_digest
    }

    fn append_file<W: Write>(builder: &mut Builder<W>, path: &str, bytes: &[u8]) {
        append_file_with_mode(builder, path, bytes, 0o644);
    }

    fn append_dir<W: Write>(builder: &mut Builder<W>, path: &str) {
        let mut header = Header::new_gnu();
        header.set_entry_type(tar::EntryType::Directory);
        header.set_size(0);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, path, &b""[..])
            .expect("append dir");
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

    struct TestToolDirGuard {
        previous: Option<PathBuf>,
    }

    impl TestToolDirGuard {
        fn new(dir: &Path) -> Self {
            Self {
                previous: super::replace_test_tool_dir(Some(dir.to_path_buf())),
            }
        }
    }

    impl Drop for TestToolDirGuard {
        fn drop(&mut self) {
            super::replace_test_tool_dir(self.previous.take());
        }
    }

    #[test]
    fn tool_program_uses_test_override_and_restores_previous_value() {
        let first_dir = crate::test_support::unique_temp_dir("devcontainer-oci-tools-first");
        let second_dir = crate::test_support::unique_temp_dir("devcontainer-oci-tools-second");

        assert_eq!(super::tool_program("curl"), "curl");
        {
            let _first = TestToolDirGuard::new(&first_dir);
            assert_eq!(
                super::tool_program("curl"),
                first_dir.join("curl").display().to_string()
            );
            {
                let _second = TestToolDirGuard::new(&second_dir);
                assert_eq!(
                    super::tool_program("curl"),
                    second_dir.join("curl").display().to_string()
                );
            }
            assert_eq!(
                super::tool_program("curl"),
                first_dir.join("curl").display().to_string()
            );
        }
        assert_eq!(super::tool_program("curl"), "curl");
    }

    #[test]
    fn transport_defaults_wrap_get_and_reject_post() {
        let transport = DefaultMethodTransport;
        let exchange = transport
            .get_exchange("https://registry.example/v2/", &[])
            .expect("default GET exchange");
        assert_eq!(exchange.response.status, 204);
        assert_eq!(exchange.response_url, "https://registry.example/v2/");
        assert!(!exchange.redirected);

        let response = transport
            .get_no_redirects("https://registry.example/token", &[])
            .expect("default GET without redirects");
        assert_eq!(response.status, 204);
        let exchange = transport
            .get_no_redirects_exchange("https://registry.example/token", &[])
            .expect("default GET exchange without redirects");
        assert_eq!(exchange.response.status, 204);
        assert_eq!(exchange.response_url, "https://registry.example/token");
        assert!(!exchange.redirected);

        assert_eq!(
            transport
                .post_no_redirects_exchange("https://registry.example/token", &[], b"body")
                .expect_err("default POST without redirects"),
            "OCI transport does not support POST requests"
        );
        assert_eq!(
            transport
                .post_exchange("https://registry.example/token", &[], b"body")
                .expect_err("default POST"),
            "OCI transport does not support POST requests"
        );
    }

    #[test]
    fn parses_registry_refs_without_features_segment_and_with_ports() {
        let short = parse_oci_reference("git").expect("short reference");
        assert_eq!(short.registry, "ghcr.io");
        assert_eq!(short.repository, "devcontainers/features/git");
        assert_eq!(short.resource, "ghcr.io/devcontainers/features/git");
        assert_eq!(short.tag, None);
        assert_eq!(short.digest, None);

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

        assert!(is_registry_qualified_reference(
            "localhost:5000/acme/features/foo"
        ));
        assert!(is_registry_qualified_reference(
            "example.com/acme/features/foo"
        ));
        assert!(!is_registry_qualified_reference(
            "https://example.com/feature.tgz"
        ));
        assert!(!is_registry_qualified_reference("file:///tmp/feature"));
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
            "https://ghcr.io/token?service=ghcr.io&scope=repository%3Aacme%2Ffeatures%2Ffake%3Apull",
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
    fn registry_resolution_reports_manifest_and_tag_errors() {
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
                status: 200,
                headers: HashMap::new(),
                body: br#"{"tags":["0.9.0","2.0.0","dev"]}"#.to_vec(),
            },
        );

        let error = resolve_feature_artifact_for_reference(&reference, None, &transport)
            .expect_err("selector should not match");
        assert!(error.contains("No published versions"), "{error}");

        let transport = FakeTransport::default();
        let reference = OciReference {
            tag: Some("1.0.0".to_string()),
            ..reference
        };
        transport.add(
            "https://ghcr.io/v2/acme/features/fake/manifests/1.0.0",
            OciHttpResponse {
                status: 404,
                headers: HashMap::new(),
                body: Vec::new(),
            },
        );

        let error = resolve_feature_artifact_for_reference(&reference, None, &transport)
            .expect_err("manifest request should fail");
        assert!(error.contains("HTTP 404"), "{error}");
    }

    #[test]
    fn public_digest_and_registry_tag_helpers_use_fixture_and_curl_paths() {
        let digest = "sha256:a00aa292592a8df58a940d6f6dfcf2bfd3efab145f62a17ccb12656528793134";
        let artifact = super::resolve_feature_artifact_with_digest(
            "ghcr.io/devcontainers/features/azure-cli:1",
            digest,
            None,
        )
        .expect("digest artifact");
        assert_eq!(artifact.reference_digest.as_deref(), Some(digest));
        assert_eq!(artifact.manifest_digest, digest);
        assert_eq!(
            list_feature_tags("ghcr.io/devcontainers/features/git-lfs", None)
                .expect("fixture tags"),
            vec!["1.0.6"]
        );

        let bin_dir = crate::test_support::unique_temp_dir("devcontainer-oci-curl-tags");
        fs::create_dir_all(&bin_dir).expect("bin dir");
        crate::test_support::write_executable_script(
            &bin_dir.join("curl"),
            r#"#!/bin/sh
headers=
body=
url=
while [ "$#" -gt 0 ]; do
    case "$1" in
        -D) headers="$2"; shift 2 ;;
        -o) body="$2"; shift 2 ;;
        -H) shift 2 ;;
        -w) shift 2 ;;
        --max-time) shift 2 ;;
        -sSL) shift ;;
        *) url="$1"; shift ;;
    esac
done
printf 'HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n' > "$headers"
printf '{"tags":["1.0.0","dev"]}' > "$body"
case "$url" in
    *invalid-status*) printf 'not-a-status' ;;
    *) printf '200' ;;
esac
"#,
        );
        let _tools = TestToolDirGuard::new(&bin_dir);

        let tags = list_feature_tags("registry.example.com/acme/features/fake", None)
            .expect("registry tags");
        assert_eq!(tags, vec!["1.0.0", "dev"]);

        let response = CurlTransport
            .get(
                "https://registry.example.com/v2/acme/features/fake/tags/list",
                &[("Accept".to_string(), "application/json".to_string())],
            )
            .expect("curl response");
        assert_eq!(response.status, 200);
        assert_eq!(response.body, br#"{"tags":["1.0.0","dev"]}"#);

        let error = CurlTransport
            .get("https://registry.example.com/invalid-status", &[])
            .expect_err("invalid curl status");
        assert!(error.contains("HTTP status code"), "{error}");

        let _ = fs::remove_dir_all(bin_dir);
    }

    #[test]
    fn registry_resolution_falls_back_to_metadata_in_layer() {
        let transport = FakeTransport::default();
        let reference = OciReference {
            original: "ghcr.io/acme/features/fake:1.0.0".to_string(),
            resource: "ghcr.io/acme/features/fake".to_string(),
            registry: "ghcr.io".to_string(),
            repository: "acme/features/fake".to_string(),
            tag: Some("1.0.0".to_string()),
            digest: None,
        };
        let layer = layer_bytes_with_manifest(
            false,
            br#"{"id":"fake","version":"1.0.0","dependsOn":["ghcr.io/acme/features/base"]}"#,
        );
        let layer_digest = format!("sha256:{}", super::sha256_digest(&layer));
        let manifest = json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "layers": [{
                "mediaType": "application/vnd.devcontainers.layer.v1+tar",
                "digest": layer_digest,
            }],
        });
        transport.add(
            "https://ghcr.io/v2/acme/features/fake/manifests/1.0.0",
            manifest_response(&manifest),
        );
        transport.add(
            &format!("https://ghcr.io/v2/acme/features/fake/blobs/{layer_digest}"),
            OciHttpResponse {
                status: 200,
                headers: HashMap::new(),
                body: layer,
            },
        );

        let artifact =
            resolve_feature_artifact_for_reference(&reference, None, &transport).expect("artifact");

        assert_eq!(artifact.metadata["id"], "fake");
        assert_eq!(
            artifact.metadata["dependsOn"][0],
            "ghcr.io/acme/features/base"
        );
        assert_eq!(
            canonical_feature_id(&artifact),
            format!("{}@{}", artifact.resource, artifact.manifest_digest)
        );
    }

    #[test]
    fn registry_blob_reports_non_success_status() {
        let transport = FakeTransport::default();
        let artifact = OciFeatureArtifact {
            original_reference: "ghcr.io/acme/features/fake:1.0.0".to_string(),
            resource: "ghcr.io/acme/features/fake".to_string(),
            registry: "ghcr.io".to_string(),
            repository: "acme/features/fake".to_string(),
            tag: Some("1.0.0".to_string()),
            reference_digest: None,
            manifest_digest: "sha256:manifest".to_string(),
            manifest: json!({}),
            metadata: json!({}),
            layer: OciFeatureLayer::Missing,
        };
        transport.add(
            "https://ghcr.io/v2/acme/features/fake/blobs/sha256:layer",
            OciHttpResponse {
                status: 503,
                headers: HashMap::new(),
                body: Vec::new(),
            },
        );

        let error = registry_blob(&artifact, "sha256:layer", &transport).expect_err("blob error");

        assert!(error.contains("HTTP 503"), "{error}");
    }

    #[test]
    fn materialize_feature_artifact_fetches_registry_layers_with_transport() {
        let transport = FakeTransport::default();
        let destination = crate::test_support::unique_temp_dir("devcontainer-oci-registry-layer");
        let layer = layer_bytes(false);
        let layer_digest = format!("sha256:{}", super::sha256_digest(&layer));
        let artifact = OciFeatureArtifact {
            original_reference: "ghcr.io/acme/features/fake:1.0.0".to_string(),
            resource: "ghcr.io/acme/features/fake".to_string(),
            registry: "ghcr.io".to_string(),
            repository: "acme/features/fake".to_string(),
            tag: Some("1.0.0".to_string()),
            reference_digest: None,
            manifest_digest: "sha256:manifest".to_string(),
            manifest: json!({}),
            metadata: json!({"id":"fake","version":"1.0.0"}),
            layer: OciFeatureLayer::Registry {
                digest: layer_digest.clone(),
                media_type: "application/vnd.devcontainers.layer.v1+tar".to_string(),
            },
        };
        transport.add(
            &format!("https://ghcr.io/v2/acme/features/fake/blobs/{layer_digest}"),
            OciHttpResponse {
                status: 200,
                headers: HashMap::new(),
                body: layer,
            },
        );

        materialize_feature_artifact_with_transport(&artifact, &destination, &transport)
            .expect("registry materialize");

        assert!(destination.join("repo").join("data.txt").is_file());
        let _ = fs::remove_dir_all(destination);
    }

    #[test]
    fn fake_transport_reports_exhausted_routes() {
        let transport = FakeTransport::default();
        transport.add(
            "https://registry.example.com/v2/",
            OciHttpResponse {
                status: 200,
                headers: HashMap::new(),
                body: Vec::new(),
            },
        );

        assert_eq!(
            transport
                .get("https://registry.example.com/v2/", &[])
                .expect("first response")
                .status,
            200
        );
        let error = transport
            .get("https://registry.example.com/v2/", &[])
            .expect_err("exhausted route");
        assert!(error.contains("missing fake route"), "{error}");
    }

    #[test]
    fn registry_tag_manifest_and_token_errors_are_reported() {
        let reference = OciReference {
            original: "registry.example.com/acme/features/fake:1.0.0".to_string(),
            resource: "registry.example.com/acme/features/fake".to_string(),
            registry: "registry.example.com".to_string(),
            repository: "acme/features/fake".to_string(),
            tag: Some("1.0.0".to_string()),
            digest: None,
        };
        let transport = FakeTransport::default();
        transport.add(
            "https://registry.example.com/v2/acme/features/fake/tags/list",
            OciHttpResponse {
                status: 500,
                headers: HashMap::new(),
                body: Vec::new(),
            },
        );
        let error = registry_tags(&reference, &transport).expect_err("tag status");
        assert!(error.contains("HTTP 500"), "{error}");

        let transport = FakeTransport::default();
        transport.add(
            "https://registry.example.com/v2/acme/features/fake/tags/list",
            OciHttpResponse {
                status: 200,
                headers: HashMap::new(),
                body: b"not-json".to_vec(),
            },
        );
        let error = registry_tags(&reference, &transport).expect_err("tag json");
        assert!(error.contains("invalid tag list"), "{error}");

        let transport = FakeTransport::default();
        transport.add(
            "https://registry.example.com/v2/acme/features/fake/tags/list",
            OciHttpResponse {
                status: 200,
                headers: HashMap::new(),
                body: br#"{"tags":["1.0.0",42,null,"2.0.0"]}"#.to_vec(),
            },
        );
        let tags = registry_tags(&reference, &transport).expect("mixed tag list");
        assert_eq!(tags, vec!["1.0.0", "2.0.0"]);

        let transport = FakeTransport::default();
        transport.add(
            "https://registry.example.com/v2/acme/features/fake/tags/list",
            OciHttpResponse {
                status: 200,
                headers: HashMap::new(),
                body: br#"{"name":"fake"}"#.to_vec(),
            },
        );
        let tags = registry_tags(&reference, &transport).expect("missing tags");
        assert!(tags.is_empty());

        let transport = FakeTransport::default();
        transport.add(
            "https://registry.example.com/v2/acme/features/fake/manifests/1.0.0",
            OciHttpResponse {
                status: 200,
                headers: HashMap::new(),
                body: b"not-json".to_vec(),
            },
        );
        let error = registry_feature_artifact(&reference, &transport).expect_err("manifest json");
        assert!(error.contains("invalid manifest"), "{error}");

        let transport = FakeTransport::default();
        transport.add(
            "https://registry.example.com/v2/acme/features/fake/tags/list",
            OciHttpResponse {
                status: 401,
                headers: HashMap::new(),
                body: Vec::new(),
            },
        );
        let response = registry_get(
            &transport,
            "registry.example.com",
            "https://registry.example.com/v2/acme/features/fake/tags/list",
            &[],
        )
        .expect("401 response without challenge");
        assert_eq!(response.status, 401);

        let transport = FakeTransport::default();
        let manifest = json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "layers": [],
        });
        transport.add(
            "https://registry.example.com/v2/acme/features/fake/manifests/1.0.0",
            OciHttpResponse {
                status: 200,
                headers: HashMap::from([(
                    "docker-content-digest".to_string(),
                    "sha256:wrong".to_string(),
                )]),
                body: serde_json::to_vec(&manifest).expect("manifest bytes"),
            },
        );
        let error =
            registry_feature_artifact(&reference, &transport).expect_err("header digest mismatch");
        assert!(error.contains("header sha256:wrong"), "{error}");

        let transport = FakeTransport::default();
        let manifest = json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "layers": [],
        });
        transport.add(
            "https://registry.example.com/v2/acme/features/fake/manifests/1.0.0",
            manifest_response(&manifest),
        );
        let error =
            registry_feature_artifact(&reference, &transport).expect_err("missing metadata");
        assert!(error.contains("does not provide metadata"), "{error}");

        let transport = FakeTransport::default();
        let manifest = json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "layers": [{
                "mediaType": "application/vnd.devcontainers.layer.v1+tar",
            }],
            "annotations": {
                "dev.containers.metadata": json!({"id":"fake","version":"1.0.0"}).to_string(),
            },
        });
        transport.add(
            "https://registry.example.com/v2/acme/features/fake/manifests/1.0.0",
            manifest_response(&manifest),
        );
        let error =
            registry_feature_artifact(&reference, &transport).expect_err("missing layer digest");
        assert!(
            error.contains("layer descriptor is missing a digest"),
            "{error}"
        );

        let transport = FakeTransport::default();
        let manifest = json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "layers": [],
            "annotations": {
                "dev.containers.metadata": "{not-json",
            },
        });
        transport.add(
            "https://registry.example.com/v2/acme/features/fake/manifests/1.0.0",
            manifest_response(&manifest),
        );
        let error =
            registry_feature_artifact(&reference, &transport).expect_err("invalid metadata");
        assert!(
            error.contains("metadata annotation is invalid JSON"),
            "{error}"
        );

        let error = fetch_bearer_token(
            &FakeTransport::default(),
            "registry.example.com",
            "Basic realm",
            None,
        )
        .expect_err("unsupported challenge");
        assert!(error.contains("Unsupported OCI auth challenge"), "{error}");
        let error = fetch_bearer_token(
            &FakeTransport::default(),
            "registry.example.com",
            r#"Bearer service="registry.example.com""#,
            None,
        )
        .expect_err("missing realm");
        assert!(error.contains("missing a realm"), "{error}");

        let transport = FakeTransport::default();
        transport.add(
            "https://issuer.example/token?service=registry.example.com&scope=",
            OciHttpResponse {
                status: 503,
                headers: HashMap::new(),
                body: Vec::new(),
            },
        );
        let error = fetch_bearer_token(
            &transport,
            "registry.example.com",
            r#"Bearer realm="https://issuer.example/token""#,
            None,
        )
        .expect_err("token status");
        assert!(error.contains("HTTP 503"), "{error}");

        let transport = FakeTransport::default();
        transport.add(
            "https://issuer.example/token?service=registry.example.com&scope=",
            OciHttpResponse {
                status: 200,
                headers: HashMap::new(),
                body: b"not-json".to_vec(),
            },
        );
        let error = fetch_bearer_token(
            &transport,
            "registry.example.com",
            r#"Bearer realm="https://issuer.example/token""#,
            None,
        )
        .expect_err("token json");
        assert!(error.contains("invalid JSON"), "{error}");

        let transport = FakeTransport::default();
        transport.add(
            "https://issuer.example/token?service=registry.example.com&scope=",
            OciHttpResponse {
                status: 200,
                headers: HashMap::new(),
                body: b"{}".to_vec(),
            },
        );
        let error = fetch_bearer_token(
            &transport,
            "registry.example.com",
            r#"Bearer realm="https://issuer.example/token""#,
            None,
        )
        .expect_err("missing token");
        assert!(error.contains("did not include a token"), "{error}");

        let transport = FakeTransport::default();
        transport.add(
            "https://issuer.example/token?service=registry.example.com&scope=repository%3Afake%3Apull",
            OciHttpResponse {
                status: 200,
                headers: HashMap::new(),
                body: br#"{"access_token":"access-1"}"#.to_vec(),
            },
        );
        let token = fetch_bearer_token(
            &transport,
            "registry.example.com",
            r#"Bearer realm="https://issuer.example/token",scope="repository:fake:pull""#,
            Some("Basic abc"),
        )
        .expect("access token");
        assert_eq!(token, "access-1");

        let transport = FakeTransport::default();
        transport.add(
            "https://issuer.example/token?service=registry.example.com&scope=",
            OciHttpResponse {
                status: 200,
                headers: HashMap::new(),
                body: br#"{"token":"lowercase-token"}"#.to_vec(),
            },
        );
        let token = fetch_bearer_token(
            &transport,
            "registry.example.com",
            r#"bearer realm="https://issuer.example/token""#,
            None,
        )
        .expect("lowercase bearer token");
        assert_eq!(token, "lowercase-token");

        let transport = FakeTransport::default();
        transport.add(
            "https://issuer.example/token?service=registry.example.com&scope=",
            OciHttpResponse {
                status: 200,
                headers: HashMap::new(),
                body: br#"{"token":"","access_token":"fallback-token"}"#.to_vec(),
            },
        );
        let token = fetch_bearer_token(
            &transport,
            "registry.example.com",
            r#"BEARER realm="https://issuer.example/token""#,
            None,
        )
        .expect("uppercase bearer access token fallback");
        assert_eq!(token, "fallback-token");
    }

    #[test]
    fn registry_auth_retry_sends_basic_to_token_service_then_bearer_to_manifest() {
        let mut env_guard = crate::test_support::process_env_guard();
        env_guard.set_var("DEVCONTAINERS_OCI_AUTH", "registry.example.com|user|secret");
        let transport = FakeTransport::default();
        transport.add(
            "https://registry.example.com/v2/acme/features/fake/manifests/1.0.0",
            OciHttpResponse {
                status: 401,
                headers: HashMap::from([(
                    "www-authenticate".to_string(),
                    r#"Bearer realm="https://issuer.example/token",service="registry.example.com""#
                        .to_string(),
                )]),
                body: Vec::new(),
            },
        );
        transport.add(
            "https://issuer.example/token?service=registry.example.com&scope=",
            OciHttpResponse {
                status: 200,
                headers: HashMap::new(),
                body: br#"{"token":"registry-token"}"#.to_vec(),
            },
        );
        transport.add(
            "https://registry.example.com/v2/acme/features/fake/manifests/1.0.0",
            OciHttpResponse {
                status: 200,
                headers: HashMap::new(),
                body: Vec::new(),
            },
        );

        let response = registry_get(
            &transport,
            "registry.example.com",
            "https://registry.example.com/v2/acme/features/fake/manifests/1.0.0",
            &[],
        )
        .expect("auth retry");

        assert_eq!(response.status, 200);
        assert_eq!(
            *transport.seen_authorization.lock().expect("seen"),
            vec![
                None,
                Some("Basic dXNlcjpzZWNyZXQ=".to_string()),
                Some("Bearer registry-token".to_string()),
            ]
        );
    }

    #[test]
    fn registry_auth_retries_basic_and_reports_malformed_bearer_challenges() {
        let mut env_guard = crate::test_support::process_env_guard();
        env_guard.set_var("DEVCONTAINERS_OCI_AUTH", "registry.example.com|user|secret");
        let registry_url = "https://registry.example.com/v2/acme/features/fake/manifests/latest";
        let transport = FakeTransport::default();
        transport.add(
            registry_url,
            OciHttpResponse {
                status: 401,
                headers: HashMap::from([(
                    "www-authenticate".to_string(),
                    "Basic realm=\"registry.example.com\"".to_string(),
                )]),
                body: Vec::new(),
            },
        );
        transport.add(
            registry_url,
            OciHttpResponse {
                status: 200,
                headers: HashMap::new(),
                body: Vec::new(),
            },
        );

        let response = registry_get(&transport, "registry.example.com", registry_url, &[])
            .expect("Basic auth retry");
        assert_eq!(response.status, 200);
        assert_eq!(
            *transport.seen_authorization.lock().expect("authorization"),
            vec![None, Some("Basic dXNlcjpzZWNyZXQ=".to_string())]
        );

        env_guard.remove_var("DEVCONTAINERS_OCI_AUTH");
        env_guard.set_var(
            "DOCKER_CONFIG",
            crate::test_support::unique_temp_dir("devcontainer-oci-no-basic-auth"),
        );
        let transport = FakeTransport::default();
        transport.add(
            registry_url,
            OciHttpResponse {
                status: 401,
                headers: HashMap::from([(
                    "www-authenticate".to_string(),
                    "Basic realm=\"registry.example.com\"".to_string(),
                )]),
                body: Vec::new(),
            },
        );
        let response = registry_get(&transport, "registry.example.com", registry_url, &[])
            .expect("Basic challenge without configured credentials");
        assert_eq!(response.status, 401);
        assert_eq!(
            *transport.seen_authorization.lock().expect("authorization"),
            vec![None]
        );

        let transport = FakeTransport::default();
        transport.add(
            registry_url,
            OciHttpResponse {
                status: 401,
                headers: HashMap::from([("www-authenticate".to_string(), "Bearer".to_string())]),
                body: Vec::new(),
            },
        );
        let error = registry_get(&transport, "registry.example.com", registry_url, &[])
            .expect_err("malformed Bearer challenge");
        assert!(error.contains("Unsupported OCI auth challenge"), "{error}");
    }

    #[test]
    fn validates_oci_auth_realm_policy_and_configured_mappings() {
        let mappings =
            parse_cross_origin_auth_hosts(&["REGISTRY.EXAMPLE:8443=AUTH.EXAMPLE:9443".to_string()])
                .expect("valid mapping");
        assert_eq!(
            mappings.get("registry.example:8443"),
            Some(&std::collections::HashSet::from([
                "auth.example:9443".to_string()
            ]))
        );

        for (realm, registry_url, expected) in [
            (
                "https://registry.example/token",
                "https://registry.example/v2/",
                true,
            ),
            (
                "https://REGISTRY.EXAMPLE/token",
                "https://registry.example/v2/",
                true,
            ),
            (
                "http://registry.example/token",
                "https://registry.example/v2/",
                false,
            ),
            (
                "http://localhost:5000/token",
                "https://localhost:5000/v2/",
                true,
            ),
            (
                "https://auth.docker.io/token",
                "https://registry-1.docker.io/v2/",
                true,
            ),
            (
                "https://gitlab.com/jwt/auth",
                "https://registry.gitlab.com/v2/",
                true,
            ),
            (
                "https://auth.docker.io.attacker.example/token",
                "https://registry-1.docker.io/v2/",
                false,
            ),
        ] {
            assert_eq!(
                is_allowed_token_service_realm(realm, registry_url, &[]),
                expected,
                "{realm} for {registry_url}"
            );
        }
        assert!(is_allowed_token_service_realm(
            "https://auth.example/token",
            "https://registry.example/v2/",
            &["registry.example=auth.example".to_string()],
        ));
        assert!(!is_allowed_token_service_realm(
            "not-a-url",
            "https://registry.example/v2/",
            &[],
        ));
        assert!(!is_allowed_token_service_realm(
            "ftp://auth.example/token",
            "https://registry.example/v2/",
            &[],
        ));
        assert!(!is_allowed_token_service_realm(
            "https://auth.example/token",
            "not-a-url",
            &[],
        ));

        let ipv6_mappings = parse_cross_origin_auth_hosts(&[
            "[::1]=[::1]".to_string(),
            "[::1]:443=[::2]:444".to_string(),
        ])
        .expect("IPv6 mappings");
        assert_eq!(
            ipv6_mappings.get("[::1]"),
            Some(&std::collections::HashSet::from([
                "[::1]".to_string(),
                "[::2]:444".to_string(),
            ]))
        );

        for invalid in [
            "auth.example",
            "=auth.example",
            "registry.example=",
            "https://registry.example=auth.example",
            "registry.example=https://auth.example",
            "registry.example/path=auth.example",
            "[::1=auth.example",
            "[]=auth.example",
            "[gg::1]=auth.example",
            "[::1]suffix=auth.example",
            "2001:db8::1=auth.example",
            "bad!host=auth.example",
            "registry.example:=auth.example",
            "registry.example:not-a-port=auth.example",
            "registry.example:99999=auth.example",
        ] {
            assert!(
                parse_cross_origin_auth_hosts(&[invalid.to_string()]).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn registry_origin_binding_preserves_scheme_ports_and_docker_aliases() {
        assert_eq!(registry_scheme("localhost"), "http");
        assert_eq!(registry_scheme("LOCALHOST:5000"), "http");
        assert_eq!(registry_scheme("127.0.0.1:5000"), "https");
        assert_eq!(registry_scheme("registry.example:5000"), "https");
        assert!(is_oci_registry_origin(
            "http://localhost:5000/v2/",
            "localhost:5000"
        ));
        assert!(!is_oci_registry_origin(
            "https://localhost:5000/v2/",
            "localhost:5000"
        ));
        assert!(!is_oci_registry_origin(
            "http://registry.example/v2/",
            "registry.example"
        ));
        assert!(is_oci_registry_origin(
            "https://registry.example:443/v2/",
            "registry.example"
        ));
        assert!(!is_oci_registry_origin(
            "https://registry.example:8443/v2/",
            "registry.example"
        ));

        for expected in [
            "registry-1.docker.io",
            "registry.docker.io",
            "docker.io",
            "index.docker.io",
        ] {
            for candidate in [
                "registry-1.docker.io",
                "registry.docker.io",
                "docker.io",
                "index.docker.io",
            ] {
                assert!(
                    is_oci_registry_origin(&format!("https://{candidate}/v2/"), expected),
                    "{candidate} should be an HTTPS alias for {expected}"
                );
            }
        }
        assert!(!is_oci_registry_origin(
            "http://registry-1.docker.io/v2/",
            "docker.io"
        ));
        assert!(!is_oci_registry_origin(
            "https://registry-1.docker.io:8443/v2/",
            "docker.io"
        ));
        assert!(!is_oci_registry_origin("not-a-url", "registry.example"));
        assert!(!is_oci_registry_origin(
            "https://registry.example/v2/",
            "bad registry"
        ));
    }

    #[test]
    fn hardened_redirected_challenge_does_not_reuse_registry_credentials() {
        let mut env_guard = crate::test_support::process_env_guard();
        env_guard.set_var(
            "DEVCONTAINERS_OCI_AUTH",
            "registry.example|user|registry-secret",
        );
        let transport = FakeTransport::default();
        let registry_url = "https://registry.example/v2/test/manifests/latest";
        transport.add_redirected(
            registry_url,
            "https://uploads.example/v2/test/manifests/latest",
            OciHttpResponse {
                status: 401,
                headers: HashMap::from([(
                    "www-authenticate".to_string(),
                    r#"Bearer realm="https://uploads.example/token",service="uploads.example",scope="repository:test:pull""#.to_string(),
                )]),
                body: Vec::new(),
            },
        );
        transport.add(
            "https://uploads.example/token?service=uploads.example&scope=repository%3Atest%3Apull",
            OciHttpResponse {
                status: 200,
                headers: HashMap::new(),
                body: br#"{"token":"upload-token"}"#.to_vec(),
            },
        );
        transport.add(
            registry_url,
            OciHttpResponse {
                status: 200,
                headers: HashMap::new(),
                body: Vec::new(),
            },
        );

        let options = crate::commands::common::OciAuthOptions {
            hardening: true,
            allowed_cross_origin_auth_hosts: Vec::new(),
        };
        crate::commands::common::with_oci_auth_options(options, || {
            let response = registry_get(&transport, "registry.example", registry_url, &[])
                .expect("anonymous redirected challenge");
            assert_eq!(response.status, 200);
            assert_eq!(
                crate::commands::common::oci_auth_diagnostics_json().expect("diagnostics")
                    ["registryRedirectWouldPreventCredentialForwarding"],
                true
            );
        });

        assert_eq!(
            *transport.seen_authorization.lock().expect("authorization"),
            vec![None, None, Some("Bearer upload-token".to_string()),]
        );
    }

    #[test]
    fn hardened_cross_origin_basic_challenge_never_receives_registry_credentials() {
        let mut env_guard = crate::test_support::process_env_guard();
        env_guard.set_var(
            "DEVCONTAINERS_OCI_AUTH",
            "registry.example|user|registry-secret",
        );
        let transport = FakeTransport::default();
        let upload_url = "https://uploads.example/upload";
        transport.add(
            upload_url,
            OciHttpResponse {
                status: 401,
                headers: HashMap::from([(
                    "www-authenticate".to_string(),
                    "Basic realm=\"uploads.example\"".to_string(),
                )]),
                body: Vec::new(),
            },
        );

        let options = crate::commands::common::OciAuthOptions {
            hardening: true,
            allowed_cross_origin_auth_hosts: Vec::new(),
        };
        let response = crate::commands::common::with_oci_auth_options(options, || {
            registry_get(
                &transport,
                "registry.example",
                upload_url,
                &[(
                    "authorization".to_string(),
                    "Bearer original-registry-token".to_string(),
                )],
            )
        })
        .expect("blocked Basic challenge response");

        assert_eq!(response.status, 401);
        assert_eq!(
            *transport.seen_authorization.lock().expect("authorization"),
            vec![None]
        );
    }

    #[test]
    fn legacy_auth_records_all_shadow_diagnostics() {
        let transport = FakeTransport::default();
        let registry_url = "https://registry.example/v2/test/manifests/latest";
        transport.add_redirected(
            registry_url,
            "https://challenge.example/v2/test/manifests/latest",
            OciHttpResponse {
                status: 401,
                headers: HashMap::from([(
                    "www-authenticate".to_string(),
                    r#"Bearer realm="http://localhost:5000/token",service="challenge.example",scope="repository:test:pull""#.to_string(),
                )]),
                body: Vec::new(),
            },
        );
        let token_url =
            "http://localhost:5000/token?service=challenge.example&scope=repository%3Atest%3Apull";
        transport.add_redirected(
            token_url,
            "http://localhost:5001/token",
            OciHttpResponse {
                status: 200,
                headers: HashMap::new(),
                body: br#"{"token":"shadow-token"}"#.to_vec(),
            },
        );
        transport.add(
            registry_url,
            OciHttpResponse {
                status: 200,
                headers: HashMap::new(),
                body: Vec::new(),
            },
        );

        crate::commands::common::with_oci_auth_options(
            crate::commands::common::OciAuthOptions::default(),
            || {
                let response = registry_get(&transport, "registry.example", registry_url, &[])
                    .expect("legacy shadow request");
                assert_eq!(response.status, 200);
                assert_eq!(
                    crate::commands::common::oci_auth_diagnostics_json(),
                    Some(json!({
                        "authLookupWouldBeBlocked": true,
                        "registryRedirectWouldPreventCredentialForwarding": true,
                        "authServerRedirect": true,
                    }))
                );
            },
        );
    }

    #[test]
    fn legacy_refresh_exchange_and_anonymous_retry_follow_redirects() {
        let transport = FakeTransport::default();
        let realm = "https://auth.example/token";
        let token_url =
            "https://auth.example/token?service=registry.example&scope=repository%3Atest%3Apull";
        transport.add_redirected(
            realm,
            "https://auth.example/refresh-redirect",
            OciHttpResponse {
                status: 403,
                headers: HashMap::new(),
                body: Vec::new(),
            },
        );
        transport.add_redirected(
            token_url,
            "https://auth.example/anonymous-redirect",
            OciHttpResponse {
                status: 200,
                headers: HashMap::new(),
                body: br#"{"token":"anonymous-token"}"#.to_vec(),
            },
        );

        let token = crate::commands::common::with_oci_auth_options(
            crate::commands::common::OciAuthOptions::default(),
            || {
                super::fetch_bearer_token_for_registry_url(
                    &transport,
                    "registry.example",
                    "https://registry.example/v2/",
                    r#"Bearer realm="https://auth.example/token",service="registry.example",scope="repository:test:pull""#,
                    None,
                    Some("refresh-token"),
                    true,
                )
            },
        )
        .expect("legacy anonymous token fallback");

        assert_eq!(token, "anonymous-token");
        assert_eq!(
            *transport.seen_methods.lock().expect("methods"),
            vec!["POST", "GET"]
        );
        assert_eq!(crate::commands::common::oci_auth_diagnostics_json(), None);
    }

    #[test]
    fn encodes_token_service_query_values_without_overwriting_existing_parameters() {
        assert_eq!(
            token_service_url(
                "https://registry.example/token?existing=value#fragment",
                "registry.example&injected=service#fragment",
                Some("repository:test:pull&injected=scope#fragment"),
            )
            .expect("token URL"),
            "https://registry.example/token?existing=value&service=registry.example%26injected%3Dservice%23fragment&scope=repository%3Atest%3Apull%26injected%3Dscope%23fragment"
        );
        assert_eq!(
            token_service_url(
                "https://registry.example/token?%73ervice=old&%73%63%6f%70%65=old&%73%63%6F%70%65=old&ser+vice=kept&bad%GG=kept",
                "registry example",
                None,
            )
            .expect("encoded query keys"),
            "https://registry.example/token?ser+vice=kept&bad%GG=kept&service=registry+example&scope="
        );
    }

    #[test]
    fn hardened_auth_rejects_untrusted_realms_and_token_redirects() {
        let options = crate::commands::common::OciAuthOptions {
            hardening: true,
            allowed_cross_origin_auth_hosts: Vec::new(),
        };
        crate::commands::common::with_oci_auth_options(options, || {
            let transport = FakeTransport::default();
            let error = fetch_bearer_token(
                &transport,
                "registry.example",
                r#"Bearer realm="https://attacker.example/token""#,
                Some("Basic secret"),
            )
            .expect_err("untrusted realm");
            assert!(error.contains("untrusted realm"), "{error}");
            assert!(transport
                .seen_authorization
                .lock()
                .expect("seen")
                .is_empty());
        });

        let options = crate::commands::common::OciAuthOptions {
            hardening: true,
            allowed_cross_origin_auth_hosts: vec!["registry.example=auth.example".to_string()],
        };
        crate::commands::common::with_oci_auth_options(options, || {
            let transport = FakeTransport::default();
            transport.add(
                "https://auth.example/token?service=registry.example&scope=",
                OciHttpResponse {
                    status: 302,
                    headers: HashMap::from([(
                        "location".to_string(),
                        "https://attacker.example/token".to_string(),
                    )]),
                    body: Vec::new(),
                },
            );
            let error = fetch_bearer_token(
                &transport,
                "registry.example",
                r#"Bearer realm="https://auth.example/token""#,
                Some("Basic secret"),
            )
            .expect_err("token redirect");
            assert!(error.contains("redirected"), "{error}");
            assert_eq!(
                *transport.seen_authorization.lock().expect("seen"),
                vec![Some("Basic secret".to_string())]
            );
        });
    }

    #[test]
    fn configured_registry_authorization_reads_env_and_docker_config_shapes() {
        let mut env_guard = crate::test_support::process_env_guard();
        let config_dir = crate::test_support::unique_temp_dir("devcontainer-oci-auth");
        fs::create_dir_all(&config_dir).expect("config dir");

        env_guard.set_var("DEVCONTAINERS_OCI_AUTH", "registry.example.com|user|token");
        assert_eq!(
            configured_basic_authorization("registry.example.com").as_deref(),
            Some("Basic dXNlcjp0b2tlbg==")
        );
        let transport = FakeTransport::default();
        transport.add(
            "https://registry.example.com/v2/",
            OciHttpResponse {
                status: 200,
                headers: HashMap::new(),
                body: Vec::new(),
            },
        );
        registry_get(
            &transport,
            "registry.example.com",
            "https://registry.example.com/v2/",
            &[],
        )
        .expect("authorized registry get");
        let last_authorization = {
            let seen = transport.seen_authorization.lock().expect("seen");
            seen.last().cloned().flatten()
        };
        assert_eq!(last_authorization.as_deref(), None);
        assert_eq!(super::env_oci_auth("other.example.com"), None);
        env_guard.set_var(
            "DEVCONTAINERS_OCI_AUTH",
            "registry.example.com|missing-token",
        );
        assert_eq!(super::env_oci_auth("registry.example.com"), None);
        env_guard.remove_var("DEVCONTAINERS_OCI_AUTH");

        env_guard.set_var("GITHUB_TOKEN", "github-token");
        assert_eq!(
            configured_basic_authorization("ghcr.io").as_deref(),
            Some("Basic eC1hY2Nlc3MtdG9rZW46Z2l0aHViLXRva2Vu")
        );
        assert_eq!(super::configured_refresh_token("ghcr.io"), None);
        let transport = FakeTransport::default();
        transport.add(
            "https://ghcr.io/v2/",
            OciHttpResponse {
                status: 200,
                headers: HashMap::new(),
                body: Vec::new(),
            },
        );
        registry_get(&transport, "ghcr.io", "https://ghcr.io/v2/", &[])
            .expect("ghcr github token auth");
        assert_eq!(
            transport
                .seen_authorization
                .lock()
                .expect("seen")
                .last()
                .and_then(|value| value.as_deref()),
            None
        );
        env_guard.remove_var("GITHUB_TOKEN");

        env_guard.set_var("DOCKER_CONFIG", &config_dir);
        fs::write(
            config_dir.join("config.json"),
            json!({
                "auths": {
                    "registry.example.com": {
                        "identitytoken": "identity-1"
                    }
                }
            })
            .to_string(),
        )
        .expect("identity config");
        assert_eq!(
            super::configured_authorization("registry.example.com").as_deref(),
            None
        );
        assert_eq!(
            super::configured_refresh_token("registry.example.com").as_deref(),
            Some("identity-1")
        );

        fs::write(
            config_dir.join("config.json"),
            json!({
                "auths": {
                    "https://registry.example.com": {
                        "auth": BASE64.encode("docker-user:docker-secret")
                    }
                }
            })
            .to_string(),
        )
        .expect("auth config");
        assert_eq!(
            configured_basic_authorization("registry.example.com").as_deref(),
            Some("Basic ZG9ja2VyLXVzZXI6ZG9ja2VyLXNlY3JldA==")
        );

        fs::write(
            config_dir.join("config.json"),
            json!({
                "auths": {
                    "https://registry.example.com/v1/": {
                        "username": "plain-user",
                        "password": "plain-secret"
                    }
                }
            })
            .to_string(),
        )
        .expect("plain config");
        let auth = docker_config_auth("registry.example.com").expect("docker config auth");
        assert_eq!(auth.username.as_deref(), Some("plain-user"));
        assert_eq!(auth.secret.as_deref(), Some("plain-secret"));
        assert_eq!(
            registry_config_keys("registry.example.com"),
            vec![
                "registry.example.com".to_string(),
                "https://registry.example.com".to_string(),
                "https://registry.example.com/v1/".to_string()
            ]
        );
        #[cfg(target_os = "macos")]
        {
            assert_eq!(platform_default_credential_helper(), Some("osxkeychain"));
        }
        #[cfg(target_os = "windows")]
        {
            assert_eq!(platform_default_credential_helper(), Some("wincred"));
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            assert_eq!(platform_default_credential_helper(), None);
            assert!(super::platform_default_credential_auth("registry.example.com").is_none());
        }

        let _ = fs::remove_dir_all(config_dir);
    }

    #[test]
    fn configured_registry_authorization_reads_credential_helpers_and_restores_env() {
        let mut env_guard = crate::test_support::process_env_guard();
        let config_dir = crate::test_support::unique_temp_dir("devcontainer-oci-helper-config");
        let bin_dir = config_dir.join("bin");
        fs::create_dir_all(&bin_dir).expect("bin dir");
        crate::test_support::write_executable_script(
            &bin_dir.join("docker-credential-fixture"),
            "#!/bin/sh\ncat >/dev/null\nprintf '{\"Username\":\"helper-user\",\"Secret\":\"helper-secret\"}'\n",
        );
        crate::test_support::write_executable_script(
            &bin_dir.join("docker-credential-fails"),
            "#!/bin/sh\ncat >/dev/null\nexit 1\n",
        );
        crate::test_support::write_executable_script(
            &bin_dir.join("docker-credential-token"),
            "#!/bin/sh\ncat >/dev/null\nprintf '{\"Username\":\"<token>\",\"Secret\":\"helper-refresh\"}'\n",
        );
        let _tools = TestToolDirGuard::new(&bin_dir);
        env_guard.set_var("DOCKER_CONFIG", &config_dir);

        fs::write(
            config_dir.join("config.json"),
            json!({
                "credHelpers": {
                    "registry.example.com": "fixture"
                }
            })
            .to_string(),
        )
        .expect("cred helper config");
        let auth = docker_config_auth("registry.example.com").expect("cred helper auth");
        assert_eq!(auth.username.as_deref(), Some("helper-user"));
        assert_eq!(auth.secret.as_deref(), Some("helper-secret"));

        fs::write(
            config_dir.join("config.json"),
            json!({
                "credsStore": "fixture"
            })
            .to_string(),
        )
        .expect("creds store config");
        let auth = docker_config_auth("registry.example.com").expect("creds store auth");
        assert_eq!(auth.username.as_deref(), Some("helper-user"));
        assert_eq!(auth.secret.as_deref(), Some("helper-secret"));
        assert!(credential_helper_auth("fails", "registry.example.com").is_none());
        let auth =
            credential_helper_auth("token", "registry.example.com").expect("token helper auth");
        assert_eq!(auth.refresh_token.as_deref(), Some("helper-refresh"));
        assert_eq!(auth.username, None);
        assert_eq!(auth.secret, None);

        for auth in [
            "not-base64".to_string(),
            BASE64.encode([0xff]),
            BASE64.encode("missing-colon"),
        ] {
            fs::write(
                config_dir.join("config.json"),
                json!({
                    "auths": {
                        "registry.example.com": {
                            "auth": auth
                        }
                    }
                })
                .to_string(),
            )
            .expect("invalid auth config");
            assert!(docker_config_auth("registry.example.com").is_none());
        }

        fs::write(
            config_dir.join("config.json"),
            json!({
                "auths": {
                    "registry.example.com": {
                        "identitytoken": "identity-only"
                    }
                }
            })
            .to_string(),
        )
        .expect("identity-only config");
        assert_eq!(configured_basic_authorization("registry.example.com"), None);

        env_guard.set_var("DEVCONTAINER_OCI_TEST_RESTORE", "restored");
        assert_eq!(
            env::var("DEVCONTAINER_OCI_TEST_RESTORE").as_deref(),
            Ok("restored")
        );
        env_guard.remove_var("DEVCONTAINER_OCI_TEST_RESTORE");
        assert!(env::var_os("DEVCONTAINER_OCI_TEST_RESTORE").is_none());
        let _ = fs::remove_dir_all(config_dir);
    }

    #[test]
    fn identity_token_is_exchanged_as_refresh_token_then_retried_anonymously() {
        let mut env_guard = crate::test_support::process_env_guard();
        let config_dir = crate::test_support::unique_temp_dir("devcontainer-oci-refresh-token");
        fs::create_dir_all(&config_dir).expect("config dir");
        env_guard.set_var("DOCKER_CONFIG", &config_dir);
        fs::write(
            config_dir.join("config.json"),
            json!({
                "auths": {
                    "registry.example.com": {
                        "identitytoken": "refresh secret&value"
                    }
                }
            })
            .to_string(),
        )
        .expect("docker config");

        let transport = FakeTransport::default();
        let registry_url = "https://registry.example.com/v2/acme/features/fake/manifests/latest";
        transport.add(
            registry_url,
            OciHttpResponse {
                status: 403,
                headers: HashMap::from([(
                    "www-authenticate".to_string(),
                    r#"Bearer realm="https://registry.example.com/token?existing=value",service="registry.example.com",scope="repository:acme/features/fake:pull""#.to_string(),
                )]),
                body: Vec::new(),
            },
        );
        transport.add(
            "https://registry.example.com/token?existing=value",
            OciHttpResponse {
                status: 403,
                headers: HashMap::new(),
                body: Vec::new(),
            },
        );
        transport.add(
            "https://registry.example.com/token?existing=value&service=registry.example.com&scope=repository%3Aacme%2Ffeatures%2Ffake%3Apull",
            OciHttpResponse {
                status: 200,
                headers: HashMap::new(),
                body: br#"{"access_token":"registry-token"}"#.to_vec(),
            },
        );
        transport.add(
            registry_url,
            OciHttpResponse {
                status: 200,
                headers: HashMap::new(),
                body: Vec::new(),
            },
        );

        let options = crate::commands::common::OciAuthOptions {
            hardening: true,
            allowed_cross_origin_auth_hosts: Vec::new(),
        };
        let response = crate::commands::common::with_oci_auth_options(options, || {
            registry_get(&transport, "registry.example.com", registry_url, &[])
        })
        .expect("refresh-token auth");

        assert_eq!(response.status, 200);
        assert_eq!(
            *transport.seen_methods.lock().expect("methods"),
            vec!["GET", "POST", "GET", "GET"]
        );
        let bodies = transport.seen_bodies.lock().expect("bodies");
        assert_eq!(
            String::from_utf8_lossy(&bodies[1]),
            "client_id=devcontainer&grant_type=refresh_token&service=registry.example.com&scope=repository%3Aacme%2Ffeatures%2Ffake%3Apull&refresh_token=refresh+secret%26value"
        );
        assert!(bodies[2].is_empty());
        let headers = transport.seen_headers.lock().expect("headers");
        assert_eq!(
            headers[1],
            vec![
                ("User-Agent".to_string(), "devcontainer".to_string()),
                (
                    "Content-Type".to_string(),
                    "application/x-www-form-urlencoded".to_string(),
                ),
            ]
        );
        assert_eq!(
            headers[2],
            vec![("User-Agent".to_string(), "devcontainer".to_string())]
        );
        assert_eq!(
            *transport.seen_authorization.lock().expect("authorization"),
            vec![None, None, None, Some("Bearer registry-token".to_string())]
        );

        let _ = fs::remove_dir_all(config_dir);
    }

    #[test]
    fn curl_transport_reports_process_failures_without_network() {
        let missing_bin_dir = crate::test_support::unique_temp_dir("devcontainer-oci-curl-missing");
        fs::create_dir_all(&missing_bin_dir).expect("missing bin dir");
        {
            let _tools = TestToolDirGuard::new(&missing_bin_dir);
            let error = CurlTransport
                .get("https://registry.example.com/v2/", &[])
                .expect_err("curl spawn failure");
            assert!(!error.is_empty());
            let error = CurlTransport
                .get_no_redirects("https://registry.example.com/token", &[])
                .expect_err("curl spawn failure without redirects");
            assert!(!error.is_empty());
        }
        let _ = fs::remove_dir_all(missing_bin_dir);

        let bin_dir = crate::test_support::unique_temp_dir("devcontainer-oci-curl-path");
        fs::create_dir_all(&bin_dir).expect("bin dir");
        crate::test_support::write_executable_script(
            &bin_dir.join("curl"),
            "#!/bin/sh\necho curl failed >&2\nexit 7\n",
        );
        let _tools = TestToolDirGuard::new(&bin_dir);

        let error = CurlTransport
            .get("https://registry.example.com/v2/", &[])
            .expect_err("curl failure");

        assert!(error.contains("curl failed"), "{error}");
        let _ = fs::remove_dir_all(bin_dir);
    }

    #[test]
    fn curl_transport_disables_curlrc_before_any_other_argument() {
        let bin_dir = crate::test_support::unique_temp_dir("devcontainer-oci-curlrc");
        fs::create_dir_all(&bin_dir).expect("bin dir");
        crate::test_support::write_executable_script(
            &bin_dir.join("curl"),
            r#"#!/bin/sh
test "$1" = "-q" || {
    echo "curlrc was not disabled first" >&2
    exit 77
}
shift
headers=
body=
while [ "$#" -gt 0 ]; do
    case "$1" in
        -D) headers="$2"; shift 2 ;;
        -o) body="$2"; shift 2 ;;
        -H|-w|--max-time) shift 2 ;;
        *) shift ;;
    esac
done
printf 'HTTP/1.1 200 OK\r\n\r\n' > "$headers"
: > "$body"
printf '200'
"#,
        );
        let _tools = TestToolDirGuard::new(&bin_dir);

        let response = CurlTransport
            .get_no_redirects("https://registry.example.com/token", &[])
            .expect("curlrc-independent response");

        assert_eq!(response.status, 200);
        let _ = fs::remove_dir_all(bin_dir);
    }

    #[test]
    fn curl_transport_preserves_effective_url_and_redirect_state() {
        let bin_dir = crate::test_support::unique_temp_dir("devcontainer-oci-curl-effective-url");
        fs::create_dir_all(&bin_dir).expect("bin dir");
        crate::test_support::write_executable_script(
            &bin_dir.join("curl"),
            r#"#!/bin/sh
headers=
body=
while [ "$#" -gt 0 ]; do
    case "$1" in
        -D) headers="$2"; shift 2 ;;
        -o) body="$2"; shift 2 ;;
        -H|-w|--max-time) shift 2 ;;
        *) shift ;;
    esac
done
printf 'HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Bearer realm="https://challenge.example/token"\r\n\r\n' > "$headers"
: > "$body"
printf '401\nhttps://challenge.example/v2/test/manifests/latest\n1'
"#,
        );
        let _tools = TestToolDirGuard::new(&bin_dir);

        let exchange = CurlTransport
            .get_exchange("https://registry.example/v2/test/manifests/latest", &[])
            .expect("curl exchange");

        assert_eq!(exchange.response.status, 401);
        assert_eq!(
            exchange.response_url,
            "https://challenge.example/v2/test/manifests/latest"
        );
        assert!(exchange.redirected);
        let _ = fs::remove_dir_all(bin_dir);
    }

    #[test]
    fn curl_transport_supports_post_modes_and_rejects_header_newlines() {
        let error = CurlTransport
            .get(
                "https://registry.example/v2/",
                &[(
                    "Authorization".to_string(),
                    "Bearer token\nleak".to_string(),
                )],
            )
            .expect_err("header newline");
        assert_eq!(error, "OCI HTTP headers must not contain newlines");

        let bin_dir = crate::test_support::unique_temp_dir("devcontainer-oci-curl-post");
        fs::create_dir_all(&bin_dir).expect("bin dir");
        crate::test_support::write_executable_script(
            &bin_dir.join("curl"),
            r#"#!/bin/sh
headers=
body=
request_headers=
request_body=
url=
redirects=0
while [ "$#" -gt 0 ]; do
    case "$1" in
        -D) headers="$2"; shift 2 ;;
        -o) body="$2"; shift 2 ;;
        -H) request_headers="${2#@}"; shift 2 ;;
        --data-binary) request_body="${2#@}"; shift 2 ;;
        -w|--max-time) shift 2 ;;
        -L) redirects=1; shift ;;
        -q|-sS) shift ;;
        *) url="$1"; shift ;;
    esac
done
if [ -n "$request_headers" ]; then
    test "$(cat "$request_headers")" = "Content-Type: application/x-www-form-urlencoded" || exit 71
fi
if [ -n "$request_body" ]; then
    test "$(cat "$request_body")" = "refresh_token=secret" || exit 72
fi
printf 'HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n' > "$headers"
printf '{"token":"curl-token"}' > "$body"
printf '200\n%s\n%s' "$url" "$redirects"
"#,
        );
        let _tools = TestToolDirGuard::new(&bin_dir);
        let headers = [(
            "Content-Type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        )];

        let exchange = CurlTransport
            .get_no_redirects_exchange("https://registry.example/token", &[])
            .expect("GET exchange without redirects");
        assert!(!exchange.redirected);
        assert_eq!(exchange.response_url, "https://registry.example/token");

        let exchange = CurlTransport
            .post_no_redirects_exchange(
                "https://registry.example/token",
                &headers,
                b"refresh_token=secret",
            )
            .expect("POST exchange without redirects");
        assert!(!exchange.redirected);
        assert_eq!(exchange.response.body, br#"{"token":"curl-token"}"#);

        let exchange = CurlTransport
            .post_exchange(
                "https://registry.example/token",
                &headers,
                b"refresh_token=secret",
            )
            .expect("POST exchange with redirects");
        assert!(exchange.redirected);
        assert_eq!(exchange.response.status, 200);
        let _ = fs::remove_dir_all(bin_dir);
    }

    #[test]
    fn localhost_registry_uses_plain_http_for_tags_manifests_and_blobs() {
        let reference = OciReference {
            original: "localhost:5000/acme/features/fake:1.0.0".to_string(),
            resource: "localhost:5000/acme/features/fake".to_string(),
            registry: "localhost:5000".to_string(),
            repository: "acme/features/fake".to_string(),
            tag: Some("1.0.0".to_string()),
            digest: None,
        };
        let transport = FakeTransport::default();
        transport.add(
            "http://localhost:5000/v2/acme/features/fake/tags/list",
            OciHttpResponse {
                status: 200,
                headers: HashMap::new(),
                body: br#"{"tags":["1.0.0"]}"#.to_vec(),
            },
        );
        assert_eq!(
            registry_tags(&reference, &transport).expect("localhost tags"),
            vec!["1.0.0"]
        );

        let layer = layer_bytes(false);
        let layer_digest = format!("sha256:{}", super::sha256_digest(&layer));
        let manifest = json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "layers": [{
                "mediaType": "application/vnd.devcontainers.layer.v1+tar",
                "digest": layer_digest,
                "size": layer.len(),
            }],
            "annotations": {
                "dev.containers.metadata": json!({"id":"fake","version":"1.0.0"}).to_string(),
            },
        });
        transport.add(
            "http://localhost:5000/v2/acme/features/fake/manifests/1.0.0",
            manifest_response(&manifest),
        );
        let artifact =
            registry_feature_artifact(&reference, &transport).expect("localhost manifest");
        transport.add(
            &format!("http://localhost:5000/v2/acme/features/fake/blobs/{layer_digest}"),
            OciHttpResponse {
                status: 200,
                headers: HashMap::new(),
                body: layer,
            },
        );
        let destination = crate::test_support::unique_temp_dir("devcontainer-localhost-oci-layer");
        materialize_feature_artifact_with_transport(&artifact, &destination, &transport)
            .expect("localhost blob");
        assert!(destination.join("install.sh").is_file());

        let _ = fs::remove_dir_all(destination);
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
    fn fixture_artifacts_cover_exact_synthetic_and_missing_metadata_entries() {
        let exact_reference =
            parse_oci_reference("ghcr.io/codspace/doesnotexist:0.1.2").expect("reference");
        let artifact = resolve_feature_artifact_for_reference(
            &exact_reference,
            None,
            &FakeTransport::default(),
        )
        .expect("synthetic artifact");
        assert_eq!(artifact.metadata["id"], "doesnotexist");
        assert_eq!(artifact.metadata["version"], "0.1.2");

        let unversioned_reference =
            parse_oci_reference("ghcr.io/codspace/doesnotexist:dev").expect("reference");
        let artifact = resolve_feature_artifact_for_reference(
            &unversioned_reference,
            None,
            &FakeTransport::default(),
        )
        .expect("synthetic unversioned artifact");
        assert_eq!(artifact.metadata["version"], "dev");

        let catalog_without_metadata =
            parse_oci_reference("ghcr.io/codspace/versioning/foo").expect("reference");
        assert!(fixture_feature_artifact(&catalog_without_metadata)
            .expect("fixture lookup")
            .is_none());
        let unqualified_unknown = parse_oci_reference("unknown-feature").expect("reference");
        let artifact = fixture_feature_artifact(&unqualified_unknown)
            .expect("unqualified fixture lookup")
            .expect("generic fixture artifact");
        assert_eq!(artifact.metadata["id"], "unknown-feature");
        assert_eq!(
            fixture_tags("ghcr.io/codspace/versioning/foo").expect("foo tags"),
            vec!["2.11.1", "0.3.1"]
        );
        assert_eq!(
            fixture_tags("ghcr.io/codspace/versioning/bar").expect("bar tags"),
            vec!["1.0.0"]
        );
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
        assert_eq!(tag_feature_ref["tag"], "1.2.1");

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

        let digest_only_artifact = OciFeatureArtifact {
            original_reference: "ghcr.io/acme/features/fake@sha256:abc".to_string(),
            resource: "ghcr.io/acme/features/fake".to_string(),
            registry: "ghcr.io".to_string(),
            repository: "acme/features/fake".to_string(),
            tag: None,
            reference_digest: Some("sha256:abc".to_string()),
            manifest_digest: "sha256:abc".to_string(),
            manifest: json!({}),
            metadata: json!({"id":"fake","version":"1.0.0"}),
            layer: OciFeatureLayer::Missing,
        };
        let digest_only_ref = feature_ref_json(&digest_only_artifact);
        assert!(digest_only_ref
            .as_object()
            .expect("featureRef object")
            .get("tag")
            .is_none());
        assert_eq!(digest_only_ref["digest"], "sha256:abc");

        let fallback_artifact = OciFeatureArtifact {
            original_reference: "ghcr.io/acme/features/fallback".to_string(),
            resource: "ghcr.io/acme/features/fallback".to_string(),
            registry: "ghcr.io".to_string(),
            repository: "acme/features/fallback".to_string(),
            tag: None,
            reference_digest: None,
            manifest_digest: "sha256:def".to_string(),
            manifest: json!({}),
            metadata: json!({}),
            layer: OciFeatureLayer::Missing,
        };
        let fallback_ref = feature_ref_json(&fallback_artifact);
        assert_eq!(fallback_ref["id"], "fallback");
        assert_eq!(fallback_ref["version"], "latest");
    }

    #[test]
    fn local_layout_resolution_supports_tags_selectors_and_digest_pins() {
        let workspace = crate::test_support::unique_temp_dir("devcontainer-oci-layout-test");
        let resource = "ghcr.io/acme/features/local-feature";
        let first_digest = write_local_layout_version(
            &workspace,
            resource,
            "1.0.0",
            json!({"id":"local-feature","version":"1.0.0"}),
            &layer_bytes(false),
        );
        let second_digest = write_local_layout_version(
            &workspace,
            resource,
            "1.2.0",
            json!({"id":"local-feature","version":"1.2.0"}),
            &layer_bytes(false),
        );
        write_local_layout_version(
            &workspace,
            resource,
            "2.0.0",
            json!({"id":"local-feature","version":"2.0.0"}),
            &layer_bytes(false),
        );
        let dev_digest = write_local_layout_version(
            &workspace,
            resource,
            "dev",
            json!({"id":"local-feature","version":"dev"}),
            &layer_bytes(false),
        );

        let exact = resolve_feature_artifact(
            "ghcr.io/acme/features/local-feature:1.0.0",
            Some(workspace.as_path()),
        )
        .expect("exact local artifact");
        assert_eq!(exact.tag.as_deref(), Some("1.0.0"));
        assert_eq!(exact.manifest_digest, format!("sha256:{first_digest}"));

        let selected =
            resolve_feature_artifact("ghcr.io/acme/features/local-feature:1", Some(&workspace))
                .expect("selector local artifact");
        assert_eq!(selected.tag.as_deref(), Some("1.2.0"));
        assert_eq!(selected.manifest_digest, format!("sha256:{second_digest}"));

        let dev =
            resolve_feature_artifact("ghcr.io/acme/features/local-feature:dev", Some(&workspace))
                .expect("dev local artifact");
        assert_eq!(dev.tag.as_deref(), Some("dev"));
        assert_eq!(dev.manifest_digest, format!("sha256:{dev_digest}"));

        let digest_pinned = resolve_feature_artifact(
            &format!("ghcr.io/acme/features/local-feature@sha256:{first_digest}"),
            Some(workspace.as_path()),
        )
        .expect("digest local artifact");
        let expected_reference_digest = format!("sha256:{first_digest}");
        assert_eq!(
            digest_pinned.reference_digest.as_deref(),
            Some(expected_reference_digest.as_str())
        );
        assert_eq!(digest_pinned.tag, None);

        let tags = list_feature_tags("ghcr.io/acme/features/local-feature", Some(&workspace))
            .expect("local tags");
        assert_eq!(tags, vec!["1.0.0", "1.2.0", "2.0.0", "dev"]);

        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn local_layout_resolution_ignores_missing_and_malformed_selectors() {
        let workspace = crate::test_support::unique_temp_dir("devcontainer-oci-layout-test");
        let resource = "ghcr.io/acme/features/local-feature";
        write_local_layout_version(
            &workspace,
            resource,
            "latest",
            json!({"id":"local-feature","version":"latest"}),
            &layer_bytes(false),
        );

        let latest =
            resolve_feature_artifact("ghcr.io/acme/features/local-feature", Some(&workspace))
                .expect("latest local artifact");
        assert_eq!(latest.tag.as_deref(), Some("latest"));

        let parsed = parse_oci_reference("ghcr.io/acme/features/local-feature:not-present")
            .expect("reference");
        let missing = super::local_layout_feature_artifact(&parsed, Some(workspace.as_path()))
            .expect("local layout lookup");
        assert!(missing.is_none());

        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn local_layout_manifest_digest_ignores_entries_without_usable_tags_or_digests() {
        let workspace = crate::test_support::unique_temp_dir("devcontainer-oci-layout-missing");
        let layout_dir = workspace
            .join(".devcontainer")
            .join("oci-layouts")
            .join("ghcr.io/acme/features/local-feature");
        fs::create_dir_all(layout_dir.join("blobs").join("sha256")).expect("layout blobs");
        fs::write(
            layout_dir.join("oci-layout"),
            "{\n  \"imageLayoutVersion\": \"1.0.0\"\n}\n",
        )
        .expect("layout marker");

        let write_index = |manifests: serde_json::Value| {
            fs::write(
                layout_dir.join("index.json"),
                serde_json::to_string_pretty(&json!({
                    "schemaVersion": 2,
                    "manifests": manifests,
                }))
                .expect("index json"),
            )
            .expect("index write");
        };

        write_index(json!([{
            "annotations": {
                "org.opencontainers.image.ref.name": "latest",
            }
        }]));
        let parsed = parse_oci_reference("ghcr.io/acme/features/local-feature").expect("reference");
        assert_eq!(
            local_layout_manifest_digest(&parsed, &layout_dir).expect("latest missing digest"),
            None
        );

        write_index(json!([{
            "digest": "sha256:abc",
            "annotations": {
                "org.opencontainers.image.ref.name": "1.0.0",
            }
        }]));
        assert_eq!(
            local_layout_manifest_digest(&parsed, &layout_dir).expect("latest absent"),
            None
        );

        write_index(json!([{
            "annotations": {
                "org.opencontainers.image.ref.name": "1.0.0",
            }
        }]));
        let parsed =
            parse_oci_reference("ghcr.io/acme/features/local-feature:1.0.0").expect("reference");
        assert_eq!(
            local_layout_manifest_digest(&parsed, &layout_dir).expect("exact missing digest"),
            None
        );

        write_index(json!([{
            "digest": "sha256:abc",
            "annotations": {
                "org.opencontainers.image.ref.name": "1.2.0",
            }
        }]));
        assert_eq!(
            local_layout_manifest_digest(&parsed, &layout_dir).expect("exact absent"),
            None
        );

        write_index(json!([{
            "annotations": {
                "org.opencontainers.image.ref.name": "dev",
            }
        }]));
        let parsed =
            parse_oci_reference("ghcr.io/acme/features/local-feature:dev").expect("reference");
        assert_eq!(
            local_layout_manifest_digest(&parsed, &layout_dir).expect("named missing digest"),
            None
        );

        write_index(json!([
            {},
            {
                "annotations": {
                    "org.opencontainers.image.ref.name": "1.0.0",
                }
            },
            {
                "digest": "sha256:abc",
                "annotations": {
                    "org.opencontainers.image.ref.name": "2.0.0",
                }
            }
        ]));
        let parsed =
            parse_oci_reference("ghcr.io/acme/features/local-feature:1").expect("reference");
        assert_eq!(
            local_layout_manifest_digest(&parsed, &layout_dir).expect("selector unusable entries"),
            None
        );

        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn local_layout_resolution_reports_invalid_manifest_and_missing_digest_entries() {
        let workspace = crate::test_support::unique_temp_dir("devcontainer-oci-layout-errors");
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
        let invalid_manifest = b"not-json";
        let invalid_digest = super::sha256_digest(invalid_manifest);
        fs::write(
            layout_dir
                .join("blobs")
                .join("sha256")
                .join(&invalid_digest),
            invalid_manifest,
        )
        .expect("invalid manifest blob");
        fs::write(
            layout_dir.join("index.json"),
            serde_json::to_string_pretty(&json!({
                "schemaVersion": 2,
                "manifests": [{
                    "digest": format!("sha256:{invalid_digest}"),
                    "annotations": {
                        "org.opencontainers.image.ref.name": "1.0.0",
                    }
                }]
            }))
            .expect("index json"),
        )
        .expect("index write");
        let parsed =
            parse_oci_reference("ghcr.io/acme/features/local-feature:1.0.0").expect("reference");
        let error = local_layout_feature_artifact(&parsed, Some(workspace.as_path()))
            .expect_err("invalid manifest json");
        assert!(error.contains("invalid JSON"), "{error}");

        fs::write(layout_dir.join("index.json"), "not-json").expect("index write");
        let parsed = parse_oci_reference("ghcr.io/acme/features/local-feature").expect("reference");
        let error = local_layout_feature_artifact(&parsed, Some(workspace.as_path()))
            .expect_err("invalid index json");
        assert!(!error.is_empty());

        fs::write(
            layout_dir.join("index.json"),
            serde_json::to_string_pretty(&json!({
                "schemaVersion": 2,
                "manifests": [{
                    "digest": "md5:unsupported",
                    "annotations": {
                        "org.opencontainers.image.ref.name": "latest",
                    }
                }]
            }))
            .expect("index json"),
        )
        .expect("index write");
        let parsed = parse_oci_reference("ghcr.io/acme/features/local-feature").expect("reference");
        let error = local_layout_feature_artifact(&parsed, Some(workspace.as_path()))
            .expect_err("unsupported manifest digest");
        assert!(error.contains("Unsupported OCI digest"), "{error}");

        fs::write(
            layout_dir.join("index.json"),
            serde_json::to_string_pretty(&json!({
                "schemaVersion": 2,
                "manifests": [{
                    "annotations": {
                        "org.opencontainers.image.ref.name": "latest",
                    }
                }]
            }))
            .expect("index json"),
        )
        .expect("index write");
        let parsed = parse_oci_reference("ghcr.io/acme/features/local-feature").expect("reference");
        let missing = local_layout_feature_artifact(&parsed, Some(workspace.as_path()))
            .expect("missing digest lookup");
        assert!(missing.is_none());

        let layer = json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "layers": [{
                "mediaType": "application/vnd.devcontainers.layer.v1+tar",
                "digest": "md5:unsupported",
            }],
            "annotations": {
                "dev.containers.metadata": json!({
                    "id": "local-feature",
                    "version": "bad-layer",
                }).to_string(),
            },
        });
        let layer_bytes = serde_json::to_vec_pretty(&layer).expect("manifest bytes");
        let layer_digest = super::sha256_digest(&layer_bytes);
        fs::write(
            layout_dir.join("blobs").join("sha256").join(&layer_digest),
            &layer_bytes,
        )
        .expect("bad layer manifest blob");
        fs::write(
            layout_dir.join("index.json"),
            serde_json::to_string_pretty(&json!({
                "schemaVersion": 2,
                "manifests": [{
                    "digest": format!("sha256:{layer_digest}"),
                    "annotations": {
                        "org.opencontainers.image.ref.name": "bad-layer",
                    }
                }]
            }))
            .expect("index json"),
        )
        .expect("index write");
        let parsed = parse_oci_reference("ghcr.io/acme/features/local-feature:bad-layer")
            .expect("reference");
        let error = local_layout_feature_artifact(&parsed, Some(workspace.as_path()))
            .expect_err("unsupported layer digest");
        assert!(
            error.contains("Unsupported OCI Feature layer digest"),
            "{error}"
        );

        let layer = feature_layer(
            &json!({
                "schemaVersion": 2,
                "layers": [{
                    "mediaType": "application/vnd.oci.image.layer.v1.tar",
                    "digest": "sha256:abc",
                }],
            }),
            None,
        )
        .expect("non-feature layer manifest");
        assert!(matches!(layer, OciFeatureLayer::Missing));

        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn fixture_tags_are_sorted_by_latest_semver() {
        assert_eq!(
            fixture_tags("ghcr.io/devcontainers/features/git").expect("fixture tags"),
            vec!["1.2.0", "1.1.5", "1.0.5", "1.0.4"]
        );
        assert_eq!(
            fixture_tags("ghcr.io/devcontainers/features/github-cli").expect("fixture tags"),
            vec!["1.0.9"]
        );
        assert_eq!(
            fixture_tags("ghcr.io/devcontainers/features/git-lfs").expect("fixture tags"),
            vec!["1.0.6"]
        );
        assert_eq!(
            fixture_tags("ghcr.io/codspace/dependson/a").expect("fixture tags"),
            vec!["2.0.1"]
        );
        assert_eq!(
            fixture_tags("ghcr.io/codspace/dependson/e").expect("fixture tags"),
            vec!["2.0.0", "1.0.0"]
        );
        assert_eq!(fixture_tags("ghcr.io/unknown/features/nope"), None);
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

        let direct_error =
            verify_manifest_digest(&reference, None, br#"{"schemaVersion":2}"#).expect_err("err");
        assert!(
            direct_error.contains("expected sha256:bad"),
            "{direct_error}"
        );
    }

    #[test]
    fn rejects_fallback_metadata_layer_digest_mismatch() {
        let transport = FakeTransport::default();
        let reference = OciReference {
            original: "ghcr.io/acme/features/fake:1.0.0".to_string(),
            resource: "ghcr.io/acme/features/fake".to_string(),
            registry: "ghcr.io".to_string(),
            repository: "acme/features/fake".to_string(),
            tag: Some("1.0.0".to_string()),
            digest: None,
        };
        let expected_layer = layer_bytes_with_manifest(
            false,
            br#"{"id":"fake","version":"1.0.0","dependsOn":["ghcr.io/acme/features/base"]}"#,
        );
        let expected_digest = format!("sha256:{}", super::sha256_digest(&expected_layer));
        let manifest = json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "layers": [{
                "mediaType": "application/vnd.devcontainers.layer.v1+tar",
                "digest": expected_digest,
            }],
        });
        transport.add(
            "https://ghcr.io/v2/acme/features/fake/manifests/1.0.0",
            manifest_response(&manifest),
        );
        let wrong_layer = layer_bytes_with_manifest(
            false,
            br#"{"id":"fake","version":"9.9.9","dependsOn":["ghcr.io/acme/features/untrusted"]}"#,
        );
        transport.add(
            &format!("https://ghcr.io/v2/acme/features/fake/blobs/{expected_digest}"),
            OciHttpResponse {
                status: 200,
                headers: HashMap::new(),
                body: wrong_layer,
            },
        );

        let error =
            resolve_feature_artifact_for_reference(&reference, None, &transport).expect_err("err");

        assert!(error.contains("Feature layer digest mismatch"), "{error}");
    }

    #[test]
    fn metadata_from_feature_layer_handles_local_missing_and_malformed_layers() {
        let destination = crate::test_support::unique_temp_dir("devcontainer-oci-metadata-test");
        fs::create_dir_all(&destination).expect("metadata temp dir");
        let reference = parse_oci_reference("ghcr.io/acme/features/fake:1.0.0").expect("reference");
        let local_bytes = layer_bytes_with_manifest(false, br#"{"id":"fake","version":"1.0.0"}"#);
        let local_digest = format!("sha256:{}", super::sha256_digest(&local_bytes));
        let local_path = destination.join("layer.tar");
        fs::write(&local_path, &local_bytes).expect("local layer");
        let metadata = metadata_from_feature_layer(
            &reference,
            &OciFeatureLayer::LocalPath {
                digest: local_digest,
                media_type: "application/vnd.devcontainers.layer.v1+tar".to_string(),
                path: local_path,
            },
            &FakeTransport::default(),
        )
        .expect("local metadata");
        assert_eq!(metadata["id"], "fake");

        let error = metadata_from_feature_layer(
            &reference,
            &OciFeatureLayer::Generated {
                install_script: "#!/bin/sh\n".to_string(),
            },
            &FakeTransport::default(),
        )
        .expect_err("generated layer metadata");
        assert!(error.contains("does not provide metadata"), "{error}");

        let error = metadata_from_feature_layer(
            &reference,
            &OciFeatureLayer::Missing,
            &FakeTransport::default(),
        )
        .expect_err("missing layer metadata");
        assert!(error.contains("does not provide metadata"), "{error}");

        let mut no_manifest_archive = Vec::new();
        {
            let mut builder = Builder::new(&mut no_manifest_archive);
            append_dir(&mut builder, ".");
            append_file(&mut builder, "install.sh", b"#!/bin/sh\n");
            builder.finish().expect("finish archive");
        }
        let error = feature_manifest_from_layer(
            &no_manifest_archive,
            "application/vnd.devcontainers.layer.v1+tar",
        )
        .expect_err("missing feature manifest");
        assert!(error.contains("does not contain"), "{error}");

        let invalid_manifest = layer_bytes_with_manifest(false, br#"{"id":"fake","version":"#);
        let error = feature_manifest_from_layer(
            &invalid_manifest,
            "application/vnd.devcontainers.layer.v1+tar",
        )
        .expect_err("invalid feature manifest");
        assert!(!error.is_empty());

        let error = feature_manifest_from_layer(
            b"not a tar archive",
            "application/vnd.devcontainers.layer.v1+tar",
        )
        .expect_err("invalid tar");
        assert!(!error.is_empty());
        let _ = fs::remove_dir_all(destination);
    }

    #[test]
    fn materialize_feature_artifact_handles_generated_missing_and_local_layers() {
        let destination = crate::test_support::unique_temp_dir("devcontainer-oci-materialize-test");
        let generated = OciFeatureArtifact {
            original_reference: "ghcr.io/acme/features/generated:1.0.0".to_string(),
            resource: "ghcr.io/acme/features/generated".to_string(),
            registry: "ghcr.io".to_string(),
            repository: "acme/features/generated".to_string(),
            tag: Some("1.0.0".to_string()),
            reference_digest: None,
            manifest_digest: "sha256:generated".to_string(),
            manifest: json!({}),
            metadata: json!({"id":"generated","version":"1.0.0"}),
            layer: OciFeatureLayer::Generated {
                install_script: "#!/bin/sh\nset -eu\n".to_string(),
            },
        };

        materialize_feature_artifact(&generated, &destination).expect("generated materialize");
        assert!(destination.join("devcontainer-feature.json").is_file());
        assert!(destination.join("install.sh").is_file());

        let missing = OciFeatureArtifact {
            layer: OciFeatureLayer::Missing,
            ..generated
        };
        let error =
            materialize_feature_artifact(&missing, &destination.join("missing")).expect_err("err");
        assert!(error.contains("does not include"), "{error}");

        let layer = layer_bytes(false);
        let layer_digest = format!("sha256:{}", super::sha256_digest(&layer));
        let layer_path = destination.join("layer.tar");
        fs::write(&layer_path, &layer).expect("layer");
        let local = OciFeatureArtifact {
            layer: OciFeatureLayer::LocalPath {
                digest: layer_digest,
                media_type: "application/vnd.devcontainers.layer.v1+tar".to_string(),
                path: layer_path,
            },
            ..missing
        };
        let local_destination = destination.join("local");
        materialize_feature_artifact(&local, &local_destination).expect("local materialize");
        assert!(local_destination.join("repo").join("data.txt").is_file());

        let _ = fs::remove_dir_all(destination);
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

    #[test]
    fn extract_feature_layer_handles_current_directory_and_directory_entries() {
        let mut archive = Vec::new();
        {
            let mut builder = Builder::new(&mut archive);
            append_dir(&mut builder, ".");
            append_dir(&mut builder, "nested");
            append_file(&mut builder, "nested/file.txt", b"data");
            builder.finish().expect("finish archive");
        }
        let destination = crate::test_support::unique_temp_dir("devcontainer-oci-dir-test");

        extract_feature_layer(
            &archive,
            "application/vnd.devcontainers.layer.v1+tar",
            &destination,
        )
        .expect("extract");

        assert!(destination.join("nested").is_dir());
        assert_eq!(
            fs::read_to_string(destination.join("nested").join("file.txt")).expect("nested file"),
            "data"
        );
        let _ = fs::remove_dir_all(destination);
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

    #[test]
    fn extract_feature_layer_rejects_unsafe_or_unsupported_entries() {
        let destination = crate::test_support::unique_temp_dir("devcontainer-oci-unsafe-test");
        let mut unsupported_archive = Vec::new();
        {
            let mut builder = Builder::new(&mut unsupported_archive);
            let mut header = Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            header.set_cksum();
            builder
                .append_data(&mut header, "linked", &b""[..])
                .expect("append symlink");
            builder.finish().expect("finish archive");
        }
        let error = extract_feature_layer(
            &unsupported_archive,
            "application/vnd.devcontainers.layer.v1+tar",
            &destination,
        )
        .expect_err("unsupported entry");
        assert!(error.contains("unsupported archive entry"), "{error}");

        assert_eq!(
            safe_archive_path(Path::new("./nested/file")).expect("safe"),
            Path::new("nested/file")
        );
        assert!(safe_archive_path(Path::new("/absolute")).is_err());
        let _ = fs::remove_dir_all(destination);
    }

    #[test]
    fn auth_and_header_helpers_parse_registry_shapes() {
        let challenge = challenge_parameters(
            r#"realm="https://example.com/token",service="registry",scope="repository:acme/features:pull,push",note="escaped \"quote\", comma",path="C:\\tmp""#,
        );
        assert_eq!(
            challenge.get("scope").map(String::as_str),
            Some("repository:acme/features:pull,push")
        );
        assert_eq!(
            challenge.get("note").map(String::as_str),
            Some("escaped \"quote\", comma")
        );
        assert_eq!(challenge.get("path").map(String::as_str), Some("C:\\tmp"));
        assert_eq!(
            challenge_parameters("realm=https://example.com/token")
                .get("realm")
                .map(String::as_str),
            Some("https://example.com/token")
        );
        assert_eq!(
            super::challenge_parameter_value(r#""trailing\""#),
            "trailing\\"
        );
        assert_eq!(
            parse_http_headers("HTTP/1.1 401 Unauthorized\r\nx-old: ignored\r\n\r\nHTTP/1.1 200 OK\r\nDocker-Content-Digest: sha256:abc\r\nContent-Type: application/json\r\n\r\n")
                .get("docker-content-digest")
                .map(String::as_str),
            Some("sha256:abc")
        );
    }

    #[test]
    fn semver_selectors_and_comparison_helpers_match_expected_order() {
        assert!(VersionSelector::parse("1").expect("major").matches("1.2.3"));
        assert!(VersionSelector::parse("1.2")
            .expect("minor")
            .matches("1.2.3"));
        assert!(!VersionSelector::parse("1.2")
            .expect("minor")
            .matches("1.3.0"));
        assert!(VersionSelector::parse("1.2.3").is_none());
        assert_eq!(exact_semver("1.2.3").expect("exact").major, 1);
        assert_eq!(exact_semver("1.2"), None);
        assert_eq!(
            compare_versions_asc("1.2.0", "1.10.0"),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_versions_desc("1.2.0", "1.10.0"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_versions_asc("dev", "latest"),
            std::cmp::Ordering::Less
        );
    }
}
