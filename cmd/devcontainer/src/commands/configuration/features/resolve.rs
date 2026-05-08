//! Feature declaration parsing, dependency ordering, and source resolution helpers.

use std::cmp::Ordering;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::commands::collections::oci;
use crate::commands::collections::registry::{
    collection_reference_version, collection_slug, direct_tarball_feature_manifest,
    normalize_collection_reference, published_feature_manifest,
};
use crate::commands::common;

use super::control::{ensure_no_disallowed_features, feature_advisories_for_oci_features};
use super::metadata::feature_metadata_entry;
use super::options::{feature_object, feature_option_values_from_manifest, feature_options};
use super::types::{
    FeatureInstallation, FeatureInstallationSource, FeatureRequest, FeatureSource, FeatureSpec,
    ResolvedFeatureSummary, ResolvedFeatureSupport,
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
    let graph =
        build_dependency_graph(root_requests, configuration, config_root, workspace_folder)?;
    let ordered_nodes = compute_feature_install_order(graph)?;

    let mut feature_sets = Vec::new();
    let mut advisory_inputs = Vec::new();
    let mut metadata_entries = Vec::new();
    let mut installations = Vec::new();
    let mut ordered_features = Vec::new();
    let mut ordered_feature_ids = Vec::new();

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
        let additional = additional
            .as_object()
            .ok_or_else(|| "--additional-features must be a JSON object".to_string())?;
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
) -> Result<Vec<FeatureNode>, String> {
    let mut worklist = VecDeque::from(root_requests);
    let mut resolved = Vec::new();

    while let Some(request) = worklist.pop_front() {
        let node = resolve_feature_node(&request, config_root, workspace_folder)?;
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
    )?;
    Ok(resolved)
}

fn resolve_feature_node(
    request: &FeatureRequest,
    config_root: &Path,
    workspace_folder: &Path,
) -> Result<FeatureNode, String> {
    let spec = resolve_feature_spec(
        &request.user_feature_id,
        &request.options,
        config_root,
        workspace_folder,
    )?;
    let depends_on = spec
        .depends_on
        .iter()
        .map(|dependency| resolve_feature_dependency(dependency, config_root, workspace_folder))
        .collect::<Result<Vec<_>, _>>()?;
    let installs_after = spec
        .installs_after
        .iter()
        .map(|dependency| resolve_feature_dependency(dependency, config_root, workspace_folder))
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
) -> Result<FeatureDependency, String> {
    let spec = resolve_feature_spec(
        &request.user_feature_id,
        &request.options,
        config_root,
        workspace_folder,
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
        let dependency = resolve_feature_dependency(&request, config_root, workspace_folder)?;
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
    let left_type = source_type(&left.source);
    let right_type = source_type(&right.source);
    if left_type != right_type {
        return left
            .user_feature_id
            .cmp(&right.user_feature_id)
            .then_with(|| left_type.cmp(right_type));
    }

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
        _ => Ordering::Equal,
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
) -> Result<FeatureSpec, String> {
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
            let manifest = direct_tarball_feature_manifest(feature_id).unwrap_or_else(|| {
                generic_feature_manifest(
                    &collection_slug(feature_id).unwrap_or_else(|| "tarball-feature".to_string()),
                    collection_reference_version(feature_id),
                )
            });
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
            let manifest = published_feature_manifest(feature_id).unwrap_or_else(|| {
                generic_feature_manifest(
                    &collection_slug(&id_without_version)
                        .unwrap_or_else(|| id_without_version.clone()),
                    collection_reference_version(feature_id),
                )
            });
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
            let artifact = oci::resolve_feature_artifact(feature_id, Some(workspace_folder))?;
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
    })
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
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
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
        .unwrap_or_else(|| feature_id.to_string())
}

fn generic_feature_manifest(id: &str, version: String) -> Value {
    json!({
        "id": id,
        "name": id
            .split('-')
            .filter(|segment| !segment.is_empty())
            .map(|segment| {
                let mut chars = segment.chars();
                match chars.next() {
                    Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
        "version": version,
        "options": {}
    })
}
