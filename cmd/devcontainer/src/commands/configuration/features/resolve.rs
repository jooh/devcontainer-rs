//! Feature declaration parsing, dependency ordering, and source resolution helpers.

#[cfg(test)]
use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::commands::collections::oci;
use crate::commands::collections::registry::{
    collection_reference_version, collection_slug, direct_tarball_feature_manifest,
    normalize_collection_reference, published_feature_manifest,
};
use crate::commands::common;
use crate::process_runner::{self, ProcessLogLevel, ProcessRequest};

use super::super::{catalog::exact_catalog_entry, Lockfile, LockfileEntry};
use super::control::{ensure_no_disallowed_features, feature_advisories_for_oci_features};
use super::metadata::feature_metadata_entry;
use super::options::{feature_object, feature_option_values_from_manifest, feature_options};
use super::types::{
    FeatureInstallation, FeatureInstallationSource, FeatureRequest, FeatureSource, FeatureSpec,
    ResolvedFeatureSummary, ResolvedFeatureSupport, ResolvedLockfileFeature,
};

#[derive(Clone)]
struct FeatureNode {
    spec: FeatureSpec,
    depends_on: Vec<FeatureDependency>,
    installs_after: Vec<FeatureDependency>,
    round_priority: usize,
}

#[derive(Clone)]
struct FeatureDependency {
    request: FeatureRequest,
    spec: FeatureSpec,
}

pub(crate) fn resolve_feature_support(
    args: &[String],
    workspace_folder: &Path,
    config_file: &Path,
    configuration: &Value,
) -> Result<Option<ResolvedFeatureSupport>, String> {
    let lockfile = super::super::upgrade::lockfile_for_resolution(args, config_file)?;
    resolve_feature_support_with_lockfile(
        args,
        workspace_folder,
        config_file,
        configuration,
        lockfile.as_ref(),
    )
}

pub(in crate::commands::configuration) fn resolve_feature_support_without_lockfile(
    args: &[String],
    workspace_folder: &Path,
    config_file: &Path,
    configuration: &Value,
) -> Result<Option<ResolvedFeatureSupport>, String> {
    resolve_feature_support_with_lockfile(args, workspace_folder, config_file, configuration, None)
}

fn resolve_feature_support_with_lockfile(
    args: &[String],
    workspace_folder: &Path,
    config_file: &Path,
    configuration: &Value,
    lockfile: Option<&Lockfile>,
) -> Result<Option<ResolvedFeatureSupport>, String> {
    let declared = declared_features(args, configuration)?;
    if declared.is_empty() {
        return Ok(None);
    }
    ensure_no_disallowed_features(args, &declared)?;

    let config_root = config_file.parent().unwrap_or(workspace_folder);
    let root_requests = declared
        .iter()
        .map(|(user_feature_id, options)| FeatureRequest {
            user_feature_id: user_feature_id.clone(),
            options: options.clone(),
        })
        .collect::<Vec<_>>();
    let graph = build_dependency_graph(
        root_requests,
        configuration,
        config_root,
        workspace_folder,
        lockfile,
    )?;
    let ordered_nodes = compute_feature_install_order(graph)?;

    let mut feature_sets = Vec::new();
    let mut advisory_inputs = Vec::new();
    let mut metadata_entries = Vec::new();
    let mut installations = Vec::new();
    let mut ordered_features = Vec::new();
    let mut ordered_feature_ids = Vec::new();
    let mut lockfile_features = Vec::new();

    for node in ordered_nodes {
        let spec = node.spec;
        feature_sets.push(json!({
            "features": [feature_object(&spec.manifest, &spec.options, &spec.value)],
            "internalVersion": "2",
            "sourceInformation": spec.source_information,
        }));
        if spec
            .metadata_entry
            .as_object()
            .is_some_and(|entries| !entries.is_empty())
        {
            metadata_entries.push(spec.metadata_entry);
        }
        if matches!(spec.source, FeatureSource::Oci { .. }) {
            if let Some(version) = spec.manifest.get("version").and_then(Value::as_str) {
                advisory_inputs.push((
                    normalize_collection_reference(&spec.user_feature_id),
                    version.to_string(),
                ));
            }
        }
        ordered_feature_ids.push(spec.user_feature_id.clone());
        ordered_features.push(ResolvedFeatureSummary {
            id: spec.install_order_id.clone(),
            options: spec.value.clone(),
        });
        if let Some(lockfile_feature) = spec.lockfile_feature.clone() {
            lockfile_features.push(lockfile_feature);
        }
        installations.push(spec.installation);
    }
    let feature_advisories = feature_advisories_for_oci_features(args, &advisory_inputs)?;

    Ok(Some(ResolvedFeatureSupport {
        features_configuration: json!({
            "featureSets": feature_sets,
        }),
        feature_advisories,
        metadata_entries,
        installations,
        ordered_features,
        ordered_feature_ids,
        lockfile_features,
    }))
}

fn declared_features(args: &[String], configuration: &Value) -> Result<Map<String, Value>, String> {
    let mut declared = configuration
        .get("features")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(raw_additional) = common::parse_option_value(args, "--additional-features") {
        let additional = crate::config::parse_jsonc_value(&raw_additional)?;
        let Some(additional) = additional.as_object() else {
            return Err("--additional-features must be a JSON object".to_string());
        };
        for (key, value) in additional {
            declared.insert(key.clone(), value.clone());
        }
    }
    Ok(declared)
}

fn build_dependency_graph(
    root_requests: Vec<FeatureRequest>,
    configuration: &Value,
    config_root: &Path,
    workspace_folder: &Path,
    lockfile: Option<&Lockfile>,
) -> Result<Vec<FeatureNode>, String> {
    let mut worklist = VecDeque::from(root_requests);
    let mut resolved = Vec::new();

    while let Some(request) = worklist.pop_front() {
        let node = resolve_feature_node(&request, config_root, workspace_folder, lockfile)?;
        if resolved.iter().any(|existing| nodes_equal(existing, &node)) {
            continue;
        }
        for dependency in &node.depends_on {
            worklist.push_back(dependency.request.clone());
        }
        resolved.push(node);
    }

    apply_override_feature_install_order(
        &mut resolved,
        configuration,
        config_root,
        workspace_folder,
        lockfile,
    )?;
    Ok(resolved)
}

fn resolve_feature_node(
    request: &FeatureRequest,
    config_root: &Path,
    workspace_folder: &Path,
    lockfile: Option<&Lockfile>,
) -> Result<FeatureNode, String> {
    let spec = resolve_feature_spec(
        &request.user_feature_id,
        &request.options,
        config_root,
        workspace_folder,
        lockfile,
    )?;
    let depends_on = spec
        .depends_on
        .iter()
        .map(|dependency| {
            resolve_feature_dependency(dependency, config_root, workspace_folder, lockfile)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let installs_after = spec
        .installs_after
        .iter()
        .map(|dependency| {
            resolve_feature_dependency(dependency, config_root, workspace_folder, lockfile)
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(FeatureNode {
        spec,
        depends_on,
        installs_after,
        round_priority: 0,
    })
}

fn resolve_feature_dependency(
    request: &FeatureRequest,
    config_root: &Path,
    workspace_folder: &Path,
    lockfile: Option<&Lockfile>,
) -> Result<FeatureDependency, String> {
    let spec = resolve_feature_spec(
        &request.user_feature_id,
        &request.options,
        config_root,
        workspace_folder,
        lockfile,
    )?;
    Ok(FeatureDependency {
        request: request.clone(),
        spec,
    })
}

fn apply_override_feature_install_order(
    worklist: &mut [FeatureNode],
    configuration: &Value,
    config_root: &Path,
    workspace_folder: &Path,
    lockfile: Option<&Lockfile>,
) -> Result<(), String> {
    let Some(overrides) = configuration
        .get("overrideFeatureInstallOrder")
        .and_then(Value::as_array)
    else {
        return Ok(());
    };

    let override_ids = overrides
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    let override_count = override_ids.len();
    for (index, override_id) in override_ids.into_iter().enumerate().rev() {
        let priority = override_count - index;
        let request = FeatureRequest {
            user_feature_id: override_id.to_string(),
            options: json!({}),
        };
        let dependency =
            resolve_feature_dependency(&request, config_root, workspace_folder, lockfile)?;
        for node in worklist.iter_mut() {
            if node_satisfies_soft_dependency(node, &dependency) {
                node.round_priority = node.round_priority.max(priority);
            }
        }
    }

    Ok(())
}

fn compute_feature_install_order(
    mut worklist: Vec<FeatureNode>,
) -> Result<Vec<FeatureNode>, String> {
    let snapshot = worklist.clone();
    for node in &mut worklist {
        node.installs_after.retain(|dependency| {
            snapshot
                .iter()
                .any(|candidate| node_satisfies_soft_dependency(candidate, dependency))
        });
    }

    let mut installation_order = Vec::new();
    while !worklist.is_empty() {
        let mut round = worklist
            .iter()
            .filter(|node| {
                node.depends_on.iter().all(|dependency| {
                    installation_order
                        .iter()
                        .any(|installed| node_matches_dependency(installed, dependency))
                }) && node.installs_after.iter().all(|dependency| {
                    installation_order
                        .iter()
                        .any(|installed| node_satisfies_soft_dependency(installed, dependency))
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        if round.is_empty() {
            return Err(format!(
                "Circular feature dependency detected: {}",
                worklist
                    .iter()
                    .map(|node| node.spec.user_feature_id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        let max_priority = round
            .iter()
            .map(|node| node.round_priority)
            .max()
            .unwrap_or(0);
        round.retain(|node| node.round_priority == max_priority);
        worklist.retain(|node| !round.iter().any(|candidate| nodes_equal(candidate, node)));
        round.sort_by(compare_nodes);
        installation_order.extend(round);
    }

    Ok(installation_order)
}

fn nodes_equal(left: &FeatureNode, right: &FeatureNode) -> bool {
    compare_specs(&left.spec, &right.spec) == Ordering::Equal
}

fn node_matches_dependency(node: &FeatureNode, dependency: &FeatureDependency) -> bool {
    compare_specs(&node.spec, &dependency.spec) == Ordering::Equal
}

fn node_satisfies_soft_dependency(node: &FeatureNode, dependency: &FeatureDependency) -> bool {
    match (&node.spec.source, &dependency.spec.source) {
        (
            FeatureSource::Oci { resource, .. },
            FeatureSource::Oci {
                resource: dependency_resource,
                ..
            },
        ) => {
            if resource == dependency_resource {
                return true;
            }
            let Some((prefix, _)) = dependency_resource.rsplit_once('/') else {
                return false;
            };
            dependency
                .spec
                .aliases
                .iter()
                .any(|alias| format!("{prefix}/{}", alias.to_ascii_lowercase()) == *resource)
        }
        (
            FeatureSource::Local { resolved_path },
            FeatureSource::Local {
                resolved_path: dependency_path,
            },
        ) => resolved_path == dependency_path,
        (
            FeatureSource::DirectTarball { uri },
            FeatureSource::DirectTarball {
                uri: dependency_uri,
            },
        ) => uri == dependency_uri,
        (
            FeatureSource::GithubRepo { id_without_version },
            FeatureSource::GithubRepo {
                id_without_version: dependency_id,
            },
        ) => id_without_version == dependency_id,
        _ => false,
    }
}

fn compare_nodes(left: &FeatureNode, right: &FeatureNode) -> Ordering {
    compare_specs(&left.spec, &right.spec)
}

fn compare_specs(left: &FeatureSpec, right: &FeatureSpec) -> Ordering {
    match (&left.source, &right.source) {
        (
            FeatureSource::Oci {
                resource,
                tag,
                digest,
            },
            FeatureSource::Oci {
                resource: right_resource,
                tag: right_tag,
                digest: right_digest,
            },
        ) => resource
            .cmp(right_resource)
            .then_with(|| match (tag, right_tag) {
                (Some(left), Some(right)) if left != right => left.cmp(right),
                _ => Ordering::Equal,
            })
            .then_with(|| compare_options(&left.value, &right.value))
            .then_with(|| digest.cmp(right_digest)),
        (
            FeatureSource::Local { resolved_path },
            FeatureSource::Local {
                resolved_path: right_path,
            },
        ) => resolved_path
            .cmp(right_path)
            .then_with(|| compare_options(&left.value, &right.value)),
        (FeatureSource::DirectTarball { uri }, FeatureSource::DirectTarball { uri: right_uri }) => {
            uri.cmp(right_uri)
                .then_with(|| compare_options(&left.value, &right.value))
        }
        (
            FeatureSource::GithubRepo { id_without_version },
            FeatureSource::GithubRepo {
                id_without_version: right_id,
            },
        ) => id_without_version
            .cmp(right_id)
            .then_with(|| compare_options(&left.value, &right.value)),
        _ => left
            .user_feature_id
            .cmp(&right.user_feature_id)
            .then_with(|| source_type(&left.source).cmp(source_type(&right.source))),
    }
}

fn source_type(source: &FeatureSource) -> &'static str {
    match source {
        FeatureSource::Local { .. } => "file-path",
        FeatureSource::Oci { .. } => "oci",
        FeatureSource::DirectTarball { .. } => "direct-tarball",
        FeatureSource::GithubRepo { .. } => "github-repo",
    }
}

fn compare_options(left: &Value, right: &Value) -> Ordering {
    match (left, right) {
        (Value::String(left), Value::String(right)) => left.cmp(right),
        (Value::Bool(left), Value::Bool(right)) => left.cmp(right),
        (Value::Object(left), Value::Object(right)) => {
            left.len().cmp(&right.len()).then_with(|| {
                left.iter()
                    .zip(right.iter())
                    .map(|((left_key, left_value), (right_key, right_value))| {
                        left_key
                            .cmp(right_key)
                            .then_with(|| compare_options(left_value, right_value))
                    })
                    .find(|ordering| *ordering != Ordering::Equal)
                    .unwrap_or(Ordering::Equal)
            })
        }
        (Value::Number(left), Value::Number(right)) => left.to_string().cmp(&right.to_string()),
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Array(left), Value::Array(right)) => left.len().cmp(&right.len()).then_with(|| {
            left.iter()
                .zip(right.iter())
                .map(|(left_value, right_value)| compare_options(left_value, right_value))
                .find(|ordering| *ordering != Ordering::Equal)
                .unwrap_or(Ordering::Equal)
        }),
        _ => value_type_name(left).cmp(value_type_name(right)),
    }
}

fn value_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn resolve_feature_spec(
    feature_id: &str,
    value: &Value,
    config_root: &Path,
    workspace_folder: &Path,
    lockfile: Option<&Lockfile>,
) -> Result<FeatureSpec, String> {
    let locked_entry = lockfile.and_then(|value| value.features.get(feature_id));
    let (manifest, source_information, installation, source, install_order_id) =
        if is_local_feature_reference(feature_id) {
            let feature_dir = resolve_local_feature_path(config_root, feature_id);
            let resolved_path = fs_path_string(&feature_dir);
            let manifest = common::parse_manifest(&feature_dir, "devcontainer-feature.json")?;
            let source_information = json!({
                "type": "file-path",
                "resolvedFilePath": resolved_path,
                "userFeatureId": feature_id,
            });
            let source = FeatureSource::Local {
                resolved_path: source_information_string(&source_information, "resolvedFilePath"),
            };
            let installation = FeatureInstallation {
                source: FeatureInstallationSource::Local(feature_dir),
                env: feature_option_values_from_manifest(&manifest, value),
            };
            (
                manifest,
                source_information,
                installation,
                source,
                feature_id.to_string(),
            )
        } else if is_direct_tarball_reference(feature_id) {
            let manifest = match direct_tarball_feature_manifest(feature_id) {
                Some(manifest) => manifest,
                None => generic_feature_manifest(
                    &collection_slug(feature_id).unwrap_or("tarball-feature".to_string()),
                    collection_reference_version(feature_id),
                ),
            };
            let source_information = json!({
                "type": "direct-tarball",
                "tarballUri": feature_id,
                "userFeatureId": feature_id,
            });
            let installation = FeatureInstallation {
                source: FeatureInstallationSource::DirectTarball(feature_id.to_string()),
                env: feature_option_values_from_manifest(&manifest, value),
            };
            (
                manifest,
                source_information,
                installation,
                FeatureSource::DirectTarball {
                    uri: feature_id.to_string(),
                },
                feature_id.to_string(),
            )
        } else if is_github_repo_feature_reference(feature_id) {
            let id_without_version = github_repo_id_without_version(feature_id);
            let manifest = match published_feature_manifest(feature_id) {
                Some(manifest) => manifest,
                None => generic_feature_manifest(
                    &collection_slug(&id_without_version).unwrap_or(id_without_version.clone()),
                    collection_reference_version(feature_id),
                ),
            };
            let source_information = json!({
                "type": "github-repo",
                "userFeatureId": feature_id,
                "userFeatureIdWithoutVersion": id_without_version,
            });
            let source = FeatureSource::GithubRepo {
                id_without_version: source_information_string(
                    &source_information,
                    "userFeatureIdWithoutVersion",
                ),
            };
            let installation = FeatureInstallation {
                source: FeatureInstallationSource::GithubRepo(feature_id.to_string()),
                env: feature_option_values_from_manifest(&manifest, value),
            };
            (
                manifest,
                source_information,
                installation,
                source,
                feature_id.to_string(),
            )
        } else {
            let locked_digest = locked_entry
                .map(|entry| entry.integrity.as_str())
                .filter(|integrity| !integrity.is_empty());
            let artifact = if let Some(digest) = locked_digest {
                oci::resolve_feature_artifact_with_digest(
                    feature_id,
                    digest,
                    Some(workspace_folder),
                )?
            } else {
                oci::resolve_feature_artifact(feature_id, Some(workspace_folder))?
            };
            let manifest = artifact.metadata.clone();
            let resource = artifact.resource.clone();
            let tag = artifact.tag.clone();
            let digest = artifact.manifest_digest.clone();
            let source_information = json!({
                "type": "oci",
                "userFeatureId": feature_id,
                "userFeatureIdWithoutVersion": resource,
                "featureRef": oci::feature_ref_json(&artifact),
                "manifestDigest": digest.clone(),
                "manifest": artifact.manifest.clone(),
            });
            let installation = FeatureInstallation {
                source: FeatureInstallationSource::Published(Box::new(artifact.clone())),
                env: feature_option_values_from_manifest(&manifest, value),
            };
            let install_order_id = oci::canonical_feature_id(&artifact);
            (
                manifest,
                source_information,
                installation,
                FeatureSource::Oci {
                    resource,
                    tag,
                    digest,
                },
                install_order_id,
            )
        };

    let lockfile_feature = resolved_lockfile_feature(
        feature_id,
        &manifest,
        &source,
        workspace_folder,
        locked_entry,
    )?;
    let options = feature_options(&manifest, value);
    let metadata_entry = feature_metadata_entry(&manifest);
    let aliases = feature_aliases(&manifest);
    let depends_on = feature_depends_on(&manifest);
    let installs_after = feature_installs_after(&manifest);

    Ok(FeatureSpec {
        user_feature_id: feature_id.to_string(),
        manifest,
        options,
        value: value.clone(),
        source_information,
        metadata_entry,
        installation,
        install_order_id,
        source,
        aliases,
        depends_on,
        installs_after,
        lockfile_feature,
    })
}

fn resolved_lockfile_feature(
    feature_id: &str,
    manifest: &Value,
    source: &FeatureSource,
    workspace_folder: &Path,
    locked_entry: Option<&LockfileEntry>,
) -> Result<Option<ResolvedLockfileFeature>, String> {
    match source {
        FeatureSource::Oci {
            resource,
            tag,
            digest,
        } => Ok(Some(ResolvedLockfileFeature {
            user_feature_id: feature_id.to_string(),
            version: manifest_version(manifest, tag.as_deref()),
            resolved: format!("{resource}@{digest}"),
            integrity: digest.clone(),
            depends_on: manifest_depends_on_entries(manifest),
        })),
        FeatureSource::DirectTarball { uri } => {
            let verified_integrity = locked_entry
                .map(|entry| verify_direct_tarball_lockfile_integrity(uri, entry))
                .transpose()?
                .flatten();
            if let Some(entry) = exact_catalog_entry(uri, Some(workspace_folder)) {
                return Ok(Some(ResolvedLockfileFeature {
                    user_feature_id: feature_id.to_string(),
                    version: entry.version,
                    resolved: entry.resolved,
                    integrity: verified_integrity.unwrap_or(entry.integrity),
                    depends_on: entry.depends_on,
                }));
            }
            Ok(Some(ResolvedLockfileFeature {
                user_feature_id: feature_id.to_string(),
                version: manifest_version(manifest, None),
                resolved: uri.clone(),
                integrity: match verified_integrity {
                    Some(integrity) => integrity,
                    None => direct_tarball_archive_integrity(uri)?,
                },
                depends_on: manifest_depends_on_entries(manifest),
            }))
        }
        FeatureSource::Local { .. } | FeatureSource::GithubRepo { .. } => Ok(None),
    }
}

fn verify_direct_tarball_lockfile_integrity(
    uri: &str,
    locked_entry: &LockfileEntry,
) -> Result<Option<String>, String> {
    if locked_entry.integrity.is_empty() {
        return Ok(None);
    }
    let actual_integrity = direct_tarball_archive_integrity(uri)?;
    if actual_integrity == locked_entry.integrity {
        return Ok(Some(actual_integrity));
    }
    Err(format!(
        "Digest did not match for {uri}. Expected {}, got {actual_integrity}.",
        locked_entry.integrity
    ))
}

fn manifest_version(manifest: &Value, fallback: Option<&str>) -> String {
    manifest
        .get("version")
        .and_then(Value::as_str)
        .or(fallback)
        .unwrap_or("latest")
        .to_string()
}

fn manifest_depends_on_entries(manifest: &Value) -> Option<Vec<String>> {
    let depends_on = manifest.get("dependsOn")?;
    let entries = if let Some(object) = depends_on.as_object() {
        object.keys().cloned().collect::<Vec<_>>()
    } else if let Some(array) = depends_on.as_array() {
        array
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    if entries.is_empty() {
        None
    } else {
        Some(entries)
    }
}

fn direct_tarball_archive_integrity(uri: &str) -> Result<String, String> {
    let temp = TempDownloadedTarball::new();
    let result = process_runner::run_process(&ProcessRequest {
        program: curl_program(),
        args: vec![
            "-fsSL".to_string(),
            "--max-time".to_string(),
            "30".to_string(),
            "-o".to_string(),
            temp.path.display().to_string(),
            uri.to_string(),
        ],
        cwd: None,
        env: HashMap::new(),
        log_level: ProcessLogLevel::Info,
    })
    .map_err(|error| error.to_string())?;
    if result.status_code != 0 {
        let stderr = result.stderr.trim();
        return if stderr.is_empty() {
            Err(format!(
                "Failed to fetch direct tarball {uri}: curl exited with status {}",
                result.status_code
            ))
        } else {
            Err(format!("Failed to fetch direct tarball {uri}: {stderr}"))
        };
    }
    let bytes = fs::read(&temp.path).map_err(|error| error.to_string())?;
    Ok(sha256_integrity(&bytes))
}

#[cfg(test)]
thread_local! {
    static TEST_CURL_PROGRAM: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

#[cfg(test)]
fn replace_test_curl_program(program: Option<PathBuf>) -> Option<PathBuf> {
    TEST_CURL_PROGRAM.with(|cell| cell.replace(program))
}

fn curl_program() -> String {
    #[cfg(test)]
    if let Some(program) = TEST_CURL_PROGRAM.with(|cell| cell.borrow().clone()) {
        return program.display().to_string();
    }

    "curl".to_string()
}

fn sha256_integrity(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{:x}", hasher.finalize())
}

struct TempDownloadedTarball {
    path: PathBuf,
}

impl TempDownloadedTarball {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nonce = COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        Self {
            path: env::temp_dir().join(format!(
                "devcontainer-direct-tarball-{}-{now}-{nonce}.tgz",
                std::process::id()
            )),
        }
    }
}

impl Drop for TempDownloadedTarball {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn feature_depends_on(manifest: &Value) -> Vec<FeatureRequest> {
    manifest
        .get("dependsOn")
        .and_then(Value::as_object)
        .map(|entries| {
            entries
                .iter()
                .map(|(user_feature_id, options)| FeatureRequest {
                    user_feature_id: user_feature_id.clone(),
                    options: options.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn feature_installs_after(manifest: &Value) -> Vec<FeatureRequest> {
    manifest
        .get("installsAfter")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(Value::as_str)
                .map(|user_feature_id| FeatureRequest {
                    user_feature_id: user_feature_id.to_string(),
                    options: json!({}),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn feature_aliases(manifest: &Value) -> Vec<String> {
    let mut aliases = Vec::new();
    if let Some(current_id) = manifest
        .get("currentId")
        .or_else(|| manifest.get("id"))
        .and_then(Value::as_str)
    {
        aliases.push(current_id.to_string());
    }
    if let Some(legacy_ids) = manifest.get("legacyIds").and_then(Value::as_array) {
        aliases.extend(
            legacy_ids
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string),
        );
    }
    aliases
}

fn is_local_feature_reference(feature_id: &str) -> bool {
    feature_id.starts_with('.') || feature_id.starts_with('/') || feature_id.starts_with("file://")
}

fn is_direct_tarball_reference(feature_id: &str) -> bool {
    feature_id.starts_with("http://") || feature_id.starts_with("https://")
}

fn is_github_repo_feature_reference(feature_id: &str) -> bool {
    !is_registry_qualified_oci_reference(feature_id)
        && !is_direct_tarball_reference(feature_id)
        && feature_id.contains('/')
}

fn is_registry_qualified_oci_reference(feature_id: &str) -> bool {
    let normalized = normalize_collection_reference(feature_id);
    let Some((registry, _)) = normalized.split_once('/') else {
        return false;
    };
    registry.contains('.') || registry.contains(':') || registry == "localhost"
}

fn resolve_local_feature_path(config_root: &Path, feature_id: &str) -> PathBuf {
    if let Some(path) = feature_id.strip_prefix("file://") {
        return PathBuf::from(path);
    }
    let path = PathBuf::from(feature_id);
    if path.is_absolute() {
        path
    } else {
        config_root.join(path)
    }
}

fn fs_path_string(path: &Path) -> String {
    match path.canonicalize() {
        Ok(path) => path,
        Err(_) => path.to_path_buf(),
    }
    .display()
    .to_string()
}

fn source_information_string(source_information: &Value, key: &str) -> String {
    source_information
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn github_repo_id_without_version(feature_id: &str) -> String {
    let last_slash = feature_id.rfind('/').unwrap_or(0);
    feature_id
        .find('@')
        .filter(|index| *index > last_slash)
        .map(|index| feature_id[..index].to_string())
        .unwrap_or(feature_id.to_string())
}

fn generic_feature_manifest(id: &str, version: String) -> Value {
    json!({
        "id": id,
        "name": id
            .split('-')
            .filter(|segment| !segment.is_empty())
            .map(|segment| {
                let mut chars = segment.chars();
                let first = chars.next().expect("non-empty segment after filter");
                format!("{}{}", first.to_ascii_uppercase(), chars.as_str())
            })
            .collect::<Vec<_>>()
            .join(" "),
        "version": version,
        "options": {}
    })
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};

    use serde_json::json;
    use sha2::Digest;

    use super::{
        compare_options, compare_specs, compute_feature_install_order, declared_features,
        direct_tarball_archive_integrity, feature_aliases, feature_depends_on,
        feature_installs_after, generic_feature_manifest, github_repo_id_without_version,
        is_direct_tarball_reference, is_github_repo_feature_reference, is_local_feature_reference,
        is_registry_qualified_oci_reference, manifest_depends_on_entries,
        node_satisfies_soft_dependency, resolve_feature_spec, resolve_feature_support,
        resolve_local_feature_path, sha256_integrity, value_type_name,
        verify_direct_tarball_lockfile_integrity, FeatureDependency, FeatureInstallation,
        FeatureInstallationSource, FeatureNode, FeatureRequest, FeatureSource, FeatureSpec,
        Lockfile, LockfileEntry, TempDownloadedTarball,
    };
    use crate::test_support::{process_env_lock, write_executable_script};

    fn spec(
        id: &str,
        source: FeatureSource,
        value: serde_json::Value,
        aliases: &[&str],
    ) -> FeatureSpec {
        FeatureSpec {
            user_feature_id: id.to_string(),
            manifest: json!({
                "id": id,
                "version": "1.0.0"
            }),
            options: value.clone(),
            value,
            source_information: json!({}),
            metadata_entry: json!({}),
            installation: FeatureInstallation {
                source: FeatureInstallationSource::Local(PathBuf::from("/unused")),
                env: Vec::new(),
            },
            install_order_id: id.to_string(),
            source,
            aliases: aliases.iter().map(|alias| alias.to_string()).collect(),
            depends_on: Vec::new(),
            installs_after: Vec::new(),
            lockfile_feature: None,
        }
    }

    fn dependency(spec: &FeatureSpec) -> FeatureDependency {
        FeatureDependency {
            request: FeatureRequest {
                user_feature_id: spec.user_feature_id.clone(),
                options: spec.value.clone(),
            },
            spec: spec.clone(),
        }
    }

    fn node(
        spec: FeatureSpec,
        depends_on: Vec<FeatureDependency>,
        installs_after: Vec<FeatureDependency>,
        round_priority: usize,
    ) -> FeatureNode {
        FeatureNode {
            spec,
            depends_on,
            installs_after,
            round_priority,
        }
    }

    fn write_local_feature(feature_dir: &Path) {
        fs::create_dir_all(feature_dir).expect("feature dir");
        fs::write(
            feature_dir.join("devcontainer-feature.json"),
            r#"{
  "id": "demo",
  "version": "1.0.0",
  "legacyIds": ["legacy-demo"],
  "dependsOn": {
    "ghcr.io/devcontainers/features/common-utils": {
      "installZsh": "false"
    }
  },
  "installsAfter": [
    "ghcr.io/devcontainers/features/git"
  ],
  "options": {
    "flag": {
      "type": "boolean",
      "default": false
    }
  }
}"#,
        )
        .expect("manifest");
    }

    fn write_feature_manifest(feature_dir: &Path, manifest: &serde_json::Value) {
        fs::create_dir_all(feature_dir).expect("feature dir");
        fs::write(
            feature_dir.join("devcontainer-feature.json"),
            serde_json::to_string_pretty(manifest).expect("manifest json"),
        )
        .expect("manifest");
    }

    fn with_fake_curl<R>(script: &str, run: impl FnOnce() -> R) -> R {
        let _guard = process_env_lock();
        let bin_dir = crate::test_support::unique_temp_dir("devcontainer-resolve-curl");
        fs::create_dir_all(&bin_dir).expect("bin dir");
        write_executable_script(&bin_dir.join("curl"), script);
        let _curl = TestCurlProgramGuard::new(bin_dir.join("curl"));

        let result = run();

        let _ = fs::remove_dir_all(bin_dir);
        result
    }

    struct TestCurlProgramGuard {
        previous: Option<PathBuf>,
    }

    impl TestCurlProgramGuard {
        fn new(program: PathBuf) -> Self {
            Self {
                previous: super::replace_test_curl_program(Some(program)),
            }
        }
    }

    impl Drop for TestCurlProgramGuard {
        fn drop(&mut self) {
            super::replace_test_curl_program(self.previous.take());
        }
    }

    #[test]
    fn declared_features_merges_additional_features_and_rejects_non_objects() {
        let declared = declared_features(
            &[
                "--additional-features".to_string(),
                r#"{"ghcr.io/devcontainers/features/git":{"version":"latest"}}"#.to_string(),
            ],
            &json!({
                "features": {
                    "demo": {}
                }
            }),
        )
        .expect("declared features");

        assert!(declared.contains_key("demo"));
        assert!(declared.contains_key("ghcr.io/devcontainers/features/git"));
        assert_eq!(
            declared_features(
                &["--additional-features".to_string(), "[]".to_string()],
                &json!({})
            )
            .unwrap_err(),
            "--additional-features must be a JSON object"
        );
    }

    #[test]
    fn resolve_feature_support_orders_local_dependencies_and_overrides() {
        let workspace = crate::test_support::unique_temp_dir("devcontainer-resolve-support");
        let config_root = workspace.join(".devcontainer");
        let features_root = config_root.join("features");
        write_feature_manifest(
            &features_root.join("base"),
            &json!({
                "id": "base",
                "version": "1.0.0",
                "options": {}
            }),
        );
        write_feature_manifest(
            &features_root.join("dep"),
            &json!({
                "id": "dep",
                "version": "1.0.0",
                "dependsOn": {
                    "./features/base": {}
                },
                "installsAfter": [
                    "./features/base"
                ],
                "options": {}
            }),
        );
        let configuration = json!({
            "features": {
                "./features/base": {},
                "./features/dep": {}
            },
            "overrideFeatureInstallOrder": [
                "./features/dep"
            ]
        });
        let config_file = config_root.join("devcontainer.json");
        fs::write(&config_file, configuration.to_string()).expect("config");

        let support = resolve_feature_support(&[], &workspace, &config_file, &configuration)
            .expect("resolved")
            .expect("feature support");

        assert_eq!(
            support.ordered_feature_ids,
            vec!["./features/base".to_string(), "./features/dep".to_string()]
        );
        assert_eq!(
            support.features_configuration["featureSets"]
                .as_array()
                .expect("feature sets")
                .len(),
            2
        );
        assert!(support.lockfile_features.is_empty());
        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn resolve_feature_support_reports_resolution_errors_from_roots_dependencies_and_overrides() {
        let workspace = crate::test_support::unique_temp_dir("devcontainer-resolve-errors");
        let config_root = workspace.join(".devcontainer");
        let features_root = config_root.join("features");
        write_feature_manifest(
            &features_root.join("base"),
            &json!({
                "id": "base",
                "version": "1.0.0",
                "options": {}
            }),
        );
        write_feature_manifest(
            &features_root.join("depends-on-missing"),
            &json!({
                "id": "depends-on-missing",
                "version": "1.0.0",
                "dependsOn": {
                    "./features/missing-dependency": {}
                },
                "options": {}
            }),
        );
        let config_file = config_root.join("devcontainer.json");

        let missing_root = json!({
            "features": {
                "./features/missing-root": {}
            }
        });
        assert!(
            resolve_feature_support(&[], &workspace, &config_file, &missing_root)
                .expect_err("missing root")
                .contains("No such file")
        );

        let missing_dependency = json!({
            "features": {
                "./features/depends-on-missing": {}
            }
        });
        assert!(
            resolve_feature_support(&[], &workspace, &config_file, &missing_dependency)
                .expect_err("missing dependency")
                .contains("No such file")
        );

        let missing_override = json!({
            "features": {
                "./features/base": {}
            },
            "overrideFeatureInstallOrder": [
                "./features/missing-override"
            ]
        });
        assert!(
            resolve_feature_support(&[], &workspace, &config_file, &missing_override)
                .expect_err("missing override")
                .contains("No such file")
        );
        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn compute_feature_install_order_respects_dependencies_priorities_and_cycles() {
        let base = spec(
            "base",
            FeatureSource::Local {
                resolved_path: "/features/base".to_string(),
            },
            json!({}),
            &[],
        );
        let dependent = spec(
            "dependent",
            FeatureSource::Local {
                resolved_path: "/features/dependent".to_string(),
            },
            json!({}),
            &[],
        );
        let soft = spec(
            "soft",
            FeatureSource::Local {
                resolved_path: "/features/soft".to_string(),
            },
            json!({}),
            &[],
        );
        let ordered = compute_feature_install_order(vec![
            node(dependent.clone(), vec![dependency(&base)], Vec::new(), 10),
            node(base.clone(), Vec::new(), Vec::new(), 0),
            node(soft.clone(), Vec::new(), vec![dependency(&base)], 5),
        ])
        .expect("order");

        assert_eq!(
            ordered
                .iter()
                .map(|node| node.spec.user_feature_id.as_str())
                .collect::<Vec<_>>(),
            vec!["base", "dependent", "soft"]
        );

        let cycle_result = compute_feature_install_order(vec![
            node(base.clone(), vec![dependency(&dependent)], Vec::new(), 0),
            node(dependent.clone(), vec![dependency(&base)], Vec::new(), 0),
        ]);
        assert!(cycle_result.is_err());
        let cycle_error = cycle_result.err().expect("cycle error");
        assert!(cycle_error.contains("Circular feature dependency detected"));
    }

    #[test]
    fn soft_dependency_matching_covers_source_kinds_and_aliases() {
        let oci_dependency = spec(
            "oci-current",
            FeatureSource::Oci {
                resource: "ghcr.io/acme/features/current".to_string(),
                tag: None,
                digest: "sha256:current".to_string(),
            },
            json!({}),
            &["legacy"],
        );
        let oci_alias = spec(
            "oci-legacy",
            FeatureSource::Oci {
                resource: "ghcr.io/acme/features/legacy".to_string(),
                tag: Some("1".to_string()),
                digest: "sha256:legacy".to_string(),
            },
            json!({}),
            &[],
        );
        let oci_same = spec(
            "oci-current-same",
            FeatureSource::Oci {
                resource: "ghcr.io/acme/features/current".to_string(),
                tag: Some("1".to_string()),
                digest: "sha256:same".to_string(),
            },
            json!({}),
            &[],
        );
        let oci_without_slash = spec(
            "oci-without-slash",
            FeatureSource::Oci {
                resource: "noslash".to_string(),
                tag: None,
                digest: "sha256:noslash".to_string(),
            },
            json!({}),
            &[],
        );
        let local = spec(
            "local",
            FeatureSource::Local {
                resolved_path: "/features/local".to_string(),
            },
            json!({}),
            &[],
        );
        let direct = spec(
            "direct",
            FeatureSource::DirectTarball {
                uri: "https://example.com/feature.tgz".to_string(),
            },
            json!({}),
            &[],
        );
        let github = spec(
            "github",
            FeatureSource::GithubRepo {
                id_without_version: "owner/repo".to_string(),
            },
            json!({}),
            &[],
        );

        assert!(node_satisfies_soft_dependency(
            &node(oci_alias, Vec::new(), Vec::new(), 0),
            &dependency(&oci_dependency)
        ));
        assert!(node_satisfies_soft_dependency(
            &node(oci_same, Vec::new(), Vec::new(), 0),
            &dependency(&oci_dependency)
        ));
        assert!(!node_satisfies_soft_dependency(
            &node(oci_dependency.clone(), Vec::new(), Vec::new(), 0),
            &dependency(&oci_without_slash)
        ));
        assert!(node_satisfies_soft_dependency(
            &node(local.clone(), Vec::new(), Vec::new(), 0),
            &dependency(&local)
        ));
        assert!(node_satisfies_soft_dependency(
            &node(direct.clone(), Vec::new(), Vec::new(), 0),
            &dependency(&direct)
        ));
        assert!(node_satisfies_soft_dependency(
            &node(github.clone(), Vec::new(), Vec::new(), 0),
            &dependency(&github)
        ));
        assert!(!node_satisfies_soft_dependency(
            &node(local, Vec::new(), Vec::new(), 0),
            &dependency(&github)
        ));
    }

    #[test]
    fn comparison_helpers_cover_sources_and_json_option_types() {
        let oci_a = spec(
            "oci-a",
            FeatureSource::Oci {
                resource: "ghcr.io/acme/features/a".to_string(),
                tag: Some("1".to_string()),
                digest: "sha256:a".to_string(),
            },
            json!({"enabled": true}),
            &[],
        );
        let oci_b = spec(
            "oci-b",
            FeatureSource::Oci {
                resource: "ghcr.io/acme/features/b".to_string(),
                tag: Some("2".to_string()),
                digest: "sha256:b".to_string(),
            },
            json!({"enabled": false}),
            &[],
        );
        let direct_a = spec(
            "direct-a",
            FeatureSource::DirectTarball {
                uri: "https://example.com/a.tgz".to_string(),
            },
            json!({}),
            &[],
        );
        let direct_b = spec(
            "direct-b",
            FeatureSource::DirectTarball {
                uri: "https://example.com/b.tgz".to_string(),
            },
            json!({}),
            &[],
        );
        let github_a = spec(
            "github-a",
            FeatureSource::GithubRepo {
                id_without_version: "owner/a".to_string(),
            },
            json!({}),
            &[],
        );
        let github_b = spec(
            "github-b",
            FeatureSource::GithubRepo {
                id_without_version: "owner/b".to_string(),
            },
            json!({}),
            &[],
        );
        let direct_same_id = spec(
            "same-id",
            FeatureSource::DirectTarball {
                uri: "https://example.com/same.tgz".to_string(),
            },
            json!({}),
            &[],
        );
        let local_same_id = spec(
            "same-id",
            FeatureSource::Local {
                resolved_path: "/features/same".to_string(),
            },
            json!({}),
            &[],
        );
        let oci_same_id = spec(
            "shared-id",
            FeatureSource::Oci {
                resource: "ghcr.io/acme/features/shared".to_string(),
                tag: Some("1".to_string()),
                digest: "sha256:shared".to_string(),
            },
            json!({}),
            &[],
        );
        let github_same_id = spec(
            "shared-id",
            FeatureSource::GithubRepo {
                id_without_version: "owner/shared".to_string(),
            },
            json!({}),
            &[],
        );

        assert_eq!(compare_specs(&oci_a, &oci_b), Ordering::Less);
        assert_eq!(compare_specs(&direct_a, &direct_b), Ordering::Less);
        assert_eq!(compare_specs(&github_a, &github_b), Ordering::Less);
        assert_eq!(compare_specs(&direct_a, &github_a), Ordering::Less);
        assert_eq!(
            compare_specs(&direct_same_id, &local_same_id),
            Ordering::Less
        );
        assert_eq!(compare_specs(&github_same_id, &oci_same_id), Ordering::Less);
        assert_eq!(compare_options(&json!("a"), &json!("b")), Ordering::Less);
        assert_eq!(compare_options(&json!(false), &json!(true)), Ordering::Less);
        assert_eq!(
            compare_options(&json!({"a": 1}), &json!({"a": 2})),
            Ordering::Less
        );
        assert_eq!(compare_options(&json!(1), &json!(2)), Ordering::Less);
        assert_eq!(compare_options(&json!(null), &json!(null)), Ordering::Equal);
        assert_eq!(compare_options(&json!([1]), &json!([1, 2])), Ordering::Less);
        assert_eq!(compare_options(&json!([1]), &json!([2])), Ordering::Less);
        assert_ne!(
            compare_options(&json!(null), &json!(false)),
            Ordering::Equal
        );
        assert_eq!(value_type_name(&json!(0)), "number");
        assert_eq!(value_type_name(&json!("value")), "string");
        assert_eq!(value_type_name(&json!([])), "array");
        assert_eq!(value_type_name(&json!({})), "object");
    }

    #[test]
    fn resolve_feature_spec_materializes_local_direct_tarball_and_github_sources() {
        let workspace = crate::test_support::unique_temp_dir("devcontainer-resolve-feature");
        let config_root = workspace.join(".devcontainer");
        let local_feature = config_root.join("features").join("demo");
        write_local_feature(&local_feature);
        let local = resolve_feature_spec(
            "./features/demo",
            &json!({
                "flag": true
            }),
            &config_root,
            &workspace,
            None,
        )
        .expect("local spec");
        let direct_uri = "https://github.com/codspace/tgz-features-with-dependson/releases/download/0.0.2/devcontainer-feature-A.tgz";
        let direct = resolve_feature_spec(direct_uri, &json!({}), &config_root, &workspace, None)
            .expect("direct spec");
        let github = resolve_feature_spec(
            "devcontainers/features/src/demo-feature@1.2.3",
            &json!({}),
            &config_root,
            &workspace,
            None,
        )
        .expect("github spec");

        assert!(matches!(local.source, FeatureSource::Local { .. }));
        assert!(local.aliases.contains(&"demo".to_string()));
        assert!(local.aliases.contains(&"legacy-demo".to_string()));
        assert_eq!(
            local.depends_on[0].user_feature_id,
            "ghcr.io/devcontainers/features/common-utils"
        );
        assert_eq!(
            local.installs_after[0].user_feature_id,
            "ghcr.io/devcontainers/features/git"
        );
        assert!(local.lockfile_feature.is_none());
        assert!(matches!(direct.source, FeatureSource::DirectTarball { .. }));
        assert_eq!(
            direct
                .lockfile_feature
                .as_ref()
                .expect("direct lockfile")
                .version,
            "2.0.1"
        );
        assert!(matches!(github.source, FeatureSource::GithubRepo { .. }));
        assert_eq!(github.manifest["version"], "1.2.3");
        assert!(github.lockfile_feature.is_none());
        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn resolve_feature_spec_covers_generic_sources_locked_digest_and_tarball_integrity() {
        let workspace = crate::test_support::unique_temp_dir("devcontainer-resolve-generic");
        let config_root = workspace.join(".devcontainer");
        fs::create_dir_all(&config_root).expect("config root");
        let curl_success = r#"#!/bin/sh
out=
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-o" ]; then
    shift
    out="$1"
    break
  fi
  shift
done
printf fixture > "$out"
"#;

        with_fake_curl(curl_success, || {
            let direct_uri = "https://example.com/devcontainer-feature-unknown.tgz";
            let direct =
                resolve_feature_spec(direct_uri, &json!({}), &config_root, &workspace, None)
                    .expect("generic direct tarball");
            assert_eq!(direct.manifest["id"], "devcontainer-feature-unknown.tgz");
            assert_eq!(
                direct
                    .lockfile_feature
                    .as_ref()
                    .expect("direct lockfile")
                    .integrity,
                sha256_integrity(b"fixture")
            );
            let direct_lockfile = Lockfile {
                features: BTreeMap::from([(
                    direct_uri.to_string(),
                    LockfileEntry {
                        version: "latest".to_string(),
                        resolved: direct_uri.to_string(),
                        integrity: sha256_integrity(b"fixture"),
                        depends_on: None,
                    },
                )]),
            };
            let locked_direct = resolve_feature_spec(
                direct_uri,
                &json!({}),
                &config_root,
                &workspace,
                Some(&direct_lockfile),
            )
            .expect("locked generic direct tarball");
            assert_eq!(
                locked_direct
                    .lockfile_feature
                    .as_ref()
                    .expect("locked direct lockfile")
                    .integrity,
                sha256_integrity(b"fixture")
            );
        });

        let github = resolve_feature_spec(
            "owner/repo/path/unknown-feature@2.0.0",
            &json!({}),
            &config_root,
            &workspace,
            None,
        )
        .expect("generic github");
        assert_eq!(github.manifest["id"], "unknown-feature");
        assert_eq!(github.manifest["version"], "2.0.0");

        let feature_id = "ghcr.io/devcontainers/features/git:1.0.4";
        let digest = "sha256:0bb490abcc0a3fb23937d29e2c18a225b51c5584edc0d9eb4131569a980f60b6";
        let lockfile = Lockfile {
            features: BTreeMap::from([(
                feature_id.to_string(),
                LockfileEntry {
                    version: "1.0.4".to_string(),
                    resolved: format!("ghcr.io/devcontainers/features/git@{digest}"),
                    integrity: digest.to_string(),
                    depends_on: None,
                },
            )]),
        };
        let oci = resolve_feature_spec(
            feature_id,
            &json!({}),
            &config_root,
            &workspace,
            Some(&lockfile),
        )
        .expect("locked oci");
        assert_eq!(oci.manifest["version"], "1.0.4");
        assert_eq!(
            oci.lockfile_feature
                .as_ref()
                .expect("oci lockfile")
                .integrity,
            digest
        );

        let bad_lockfile = Lockfile {
            features: BTreeMap::from([(
                feature_id.to_string(),
                LockfileEntry {
                    version: "1.0.4".to_string(),
                    resolved: "ghcr.io/devcontainers/features/git@sha256:bad".to_string(),
                    integrity: "sha256:bad".to_string(),
                    depends_on: None,
                },
            )]),
        };
        let result = resolve_feature_spec(
            feature_id,
            &json!({}),
            &config_root,
            &workspace,
            Some(&bad_lockfile),
        );
        assert!(result.is_err());
        let error = result.err().expect("bad locked digest");
        assert!(error.contains("digest mismatch"), "{error}");
        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn direct_tarball_integrity_reports_success_and_failures() {
        let curl_success = r#"#!/bin/sh
out=
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-o" ]; then
    shift
    out="$1"
    break
  fi
  shift
done
printf fixture > "$out"
"#;
        with_fake_curl(curl_success, || {
            assert_eq!(
                direct_tarball_archive_integrity("https://example.com/archive.tgz")
                    .expect("integrity"),
                sha256_integrity(b"fixture")
            );
            let matched = LockfileEntry {
                version: "latest".to_string(),
                resolved: "https://example.com/archive.tgz".to_string(),
                integrity: sha256_integrity(b"fixture"),
                depends_on: None,
            };
            assert_eq!(
                verify_direct_tarball_lockfile_integrity(
                    "https://example.com/archive.tgz",
                    &matched,
                )
                .expect("digest match"),
                Some(sha256_integrity(b"fixture"))
            );
            let mismatched = LockfileEntry {
                version: "latest".to_string(),
                resolved: "https://example.com/archive.tgz".to_string(),
                integrity: "sha256:wrong".to_string(),
                depends_on: None,
            };
            let error = verify_direct_tarball_lockfile_integrity(
                "https://example.com/archive.tgz",
                &mismatched,
            )
            .expect_err("digest mismatch");
            assert!(error.contains("Digest did not match"), "{error}");

            let workspace =
                crate::test_support::unique_temp_dir("devcontainer-resolve-direct-lock-error");
            let config_root = workspace.join(".devcontainer");
            fs::create_dir_all(&config_root).expect("config root");
            let direct_uri = "https://example.com/devcontainer-feature-unknown.tgz";
            let lockfile = Lockfile {
                features: BTreeMap::from([(
                    direct_uri.to_string(),
                    LockfileEntry {
                        version: "latest".to_string(),
                        resolved: direct_uri.to_string(),
                        integrity: "sha256:wrong".to_string(),
                        depends_on: None,
                    },
                )]),
            };
            let result = resolve_feature_spec(
                direct_uri,
                &json!({}),
                &config_root,
                &workspace,
                Some(&lockfile),
            );
            assert!(result.is_err());
            let error = result.err().expect("direct tarball lockfile mismatch");
            assert!(error.contains("Digest did not match"), "{error}");
            let _ = fs::remove_dir_all(workspace);
        });

        with_fake_curl("#!/bin/sh\nexit 7\n", || {
            let error = direct_tarball_archive_integrity("https://example.com/archive.tgz")
                .expect_err("curl failure");
            assert!(error.contains("curl exited with status 7"), "{error}");
        });

        with_fake_curl("#!/bin/sh\necho broken >&2\nexit 7\n", || {
            let error = direct_tarball_archive_integrity("https://example.com/archive.tgz")
                .expect_err("curl stderr failure");
            assert!(error.contains("broken"), "{error}");
        });
    }

    #[test]
    fn manifest_reference_and_integrity_helpers_cover_edge_cases() {
        assert_eq!(
            manifest_depends_on_entries(&json!({
                "dependsOn": {
                    "a": {},
                    "b": {}
                }
            })),
            Some(vec!["a".to_string(), "b".to_string()])
        );
        assert_eq!(
            manifest_depends_on_entries(&json!({
                "dependsOn": ["a", 1, "b"]
            })),
            Some(vec!["a".to_string(), "b".to_string()])
        );
        assert_eq!(
            manifest_depends_on_entries(&json!({
                "dependsOn": true
            })),
            None
        );
        assert_eq!(
            feature_depends_on(&json!({
                "dependsOn": {
                    "a": {
                        "value": true
                    }
                }
            }))[0]
                .options,
            json!({
                "value": true
            })
        );
        assert_eq!(
            feature_installs_after(&json!({
                "installsAfter": ["a", 1, "b"]
            }))
            .iter()
            .map(|request| request.user_feature_id.as_str())
            .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        assert_eq!(
            feature_aliases(&json!({
                "currentId": "current",
                "legacyIds": ["old", 1]
            })),
            vec!["current".to_string(), "old".to_string()]
        );
        assert!(is_local_feature_reference("file:///tmp/feature"));
        assert!(is_direct_tarball_reference(
            "https://example.com/feature.tgz"
        ));
        assert!(is_github_repo_feature_reference("owner/repo"));
        assert!(!is_registry_qualified_oci_reference("owner/repo"));
        assert!(is_registry_qualified_oci_reference(
            "localhost/features/demo"
        ));
        assert_eq!(
            resolve_local_feature_path(Path::new("/config"), "file:///tmp/feature"),
            PathBuf::from("/tmp/feature")
        );
        assert_eq!(
            resolve_local_feature_path(Path::new("/config"), "/abs/feature"),
            PathBuf::from("/abs/feature")
        );
        assert_eq!(
            github_repo_id_without_version("owner/repo@1.2.3"),
            "owner/repo"
        );
        assert_eq!(
            generic_feature_manifest("demo-feature", "1.0.0".to_string())["name"],
            "Demo Feature"
        );
        assert_eq!(
            sha256_integrity(b"demo"),
            format!("sha256:{:x}", sha2::Sha256::digest(b"demo"))
        );
        let empty_lock = LockfileEntry {
            version: "1.0.0".to_string(),
            resolved: "https://example.com/feature.tgz".to_string(),
            integrity: String::new(),
            depends_on: None,
        };
        assert_eq!(
            verify_direct_tarball_lockfile_integrity(
                "https://example.com/feature.tgz",
                &empty_lock
            )
            .expect("empty integrity"),
            None
        );
        let temp_path = {
            let temp = TempDownloadedTarball::new();
            fs::write(&temp.path, "temporary").expect("temp write");
            temp.path.clone()
        };
        assert!(!temp_path.exists());
    }
}
