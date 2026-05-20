//! Control-manifest helpers for disallowed Features and future advisory support.

use std::fs;
use std::path::PathBuf;

use serde_json::{Map, Value};

use crate::commands::common;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct DevContainerControlManifest {
    disallowed_features: Vec<DisallowedFeature>,
    #[allow(dead_code)]
    feature_advisories: Vec<FeatureAdvisory>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DisallowedFeature {
    feature_id_prefix: String,
    documentation_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)]
struct FeatureAdvisory {
    feature_id: String,
    introduced_in_version: String,
    fixed_in_version: String,
    description: String,
    documentation_url: Option<String>,
}

pub(crate) fn ensure_no_disallowed_features(
    args: &[String],
    declared_features: &Map<String, Value>,
) -> Result<(), String> {
    if declared_features.is_empty() {
        return Ok(());
    }

    let control_manifest = control_manifest(args)?;
    for feature_id in declared_features.keys() {
        if let Some(entry) = find_disallowed_feature_entry(&control_manifest, feature_id) {
            let mut message = format!(
                "Cannot use the '{feature_id}' Feature since it was reported to be problematic. Please remove this Feature from your configuration and rebuild any dev container using it before continuing."
            );
            if let Some(documentation_url) = &entry.documentation_url {
                message.push_str(&format!(" See {documentation_url} to learn more."));
            }
            return Err(message);
        }
    }

    Ok(())
}

pub(crate) fn feature_advisories_for_oci_features(
    args: &[String],
    features: &[(String, String)],
) -> Result<Vec<Value>, String> {
    if features.is_empty() {
        return Ok(Vec::new());
    }

    let control_manifest = control_manifest(args)?;
    let mut matches = Vec::new();
    for (feature_id, version) in features {
        let advisories = control_manifest
            .feature_advisories
            .iter()
            .filter(|entry| {
                entry.feature_id == *feature_id
                    && feature_version_is_affected(
                        version,
                        &entry.introduced_in_version,
                        &entry.fixed_in_version,
                    )
            })
            .map(feature_advisory_json)
            .collect::<Vec<_>>();
        if advisories.is_empty() {
            continue;
        }

        matches.push(serde_json::json!({
            "feature": {
                "id": feature_id,
                "version": version,
            },
            "advisories": advisories,
        }));
    }

    Ok(matches)
}

fn control_manifest(args: &[String]) -> Result<DevContainerControlManifest, String> {
    let manifest_path = control_manifest_path(args);
    if !manifest_path.is_file() {
        return Ok(DevContainerControlManifest::default());
    }

    let raw = fs::read_to_string(&manifest_path).map_err(|error| error.to_string())?;
    let parsed = crate::config::parse_jsonc_value(&raw)?;
    Ok(sanitize_control_manifest(&parsed))
}

fn control_manifest_path(args: &[String]) -> PathBuf {
    common::parse_option_value(args, "--user-data-folder")
        .map(PathBuf::from)
        .unwrap_or_else(default_user_data_folder)
        .join("control-manifest.json")
}

#[cfg(target_os = "linux")]
fn default_user_data_folder() -> PathBuf {
    let username = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());
    std::env::temp_dir().join(format!("devcontainercli-{username}"))
}

#[cfg(not(target_os = "linux"))]
fn default_user_data_folder() -> PathBuf {
    std::env::temp_dir().join("devcontainercli")
}

fn sanitize_control_manifest(value: &Value) -> DevContainerControlManifest {
    let Some(entries) = value.as_object() else {
        return DevContainerControlManifest::default();
    };

    DevContainerControlManifest {
        disallowed_features: entries
            .get("disallowedFeatures")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                let object = entry.as_object()?;
                let feature_id_prefix = object.get("featureIdPrefix")?.as_str()?.to_string();
                Some(DisallowedFeature {
                    feature_id_prefix,
                    documentation_url: object
                        .get("documentationURL")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                })
            })
            .collect(),
        feature_advisories: entries
            .get("featureAdvisories")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                let object = entry.as_object()?;
                Some(FeatureAdvisory {
                    feature_id: object.get("featureId")?.as_str()?.to_string(),
                    introduced_in_version: object.get("introducedInVersion")?.as_str()?.to_string(),
                    fixed_in_version: object.get("fixedInVersion")?.as_str()?.to_string(),
                    description: object.get("description")?.as_str()?.to_string(),
                    documentation_url: object
                        .get("documentationURL")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                })
            })
            .collect(),
    }
}

fn find_disallowed_feature_entry<'a>(
    control_manifest: &'a DevContainerControlManifest,
    feature_id: &str,
) -> Option<&'a DisallowedFeature> {
    control_manifest
        .disallowed_features
        .iter()
        .find(|entry| feature_matches_prefix(feature_id, &entry.feature_id_prefix))
}

fn feature_matches_prefix(feature_id: &str, prefix: &str) -> bool {
    feature_id.starts_with(prefix)
        && (feature_id.len() == prefix.len()
            || matches!(
                feature_id.as_bytes().get(prefix.len()).copied(),
                Some(b'/') | Some(b':') | Some(b'@')
            ))
}

fn feature_version_is_affected(
    feature_version: &str,
    introduced_in_version: &str,
    fixed_in_version: &str,
) -> bool {
    let Some(feature_version) = parse_version(feature_version) else {
        return false;
    };
    let Some(introduced_in_version) = parse_version(introduced_in_version) else {
        return false;
    };
    let Some(fixed_in_version) = parse_version(fixed_in_version) else {
        return false;
    };

    feature_version >= introduced_in_version && feature_version < fixed_in_version
}

fn parse_version(input: &str) -> Option<(u64, u64, u64)> {
    let mut parts = input.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

fn feature_advisory_json(advisory: &FeatureAdvisory) -> Value {
    serde_json::json!({
        "featureId": advisory.feature_id,
        "introducedInVersion": advisory.introduced_in_version,
        "fixedInVersion": advisory.fixed_in_version,
        "description": advisory.description,
        "documentationURL": advisory.documentation_url,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Map, Value};

    use super::{
        ensure_no_disallowed_features, feature_advisories_for_oci_features, feature_matches_prefix,
        feature_version_is_affected, parse_version, sanitize_control_manifest,
    };
    use crate::test_support::{unique_temp_dir, write_test_control_manifest};

    #[test]
    fn disallowed_feature_matching_accepts_exact_ids_and_supported_separators() {
        assert!(feature_matches_prefix(
            "example.io/test/node",
            "example.io/test/node"
        ));
        assert!(feature_matches_prefix(
            "example.io/test/node:1",
            "example.io/test/node"
        ));
        assert!(feature_matches_prefix(
            "example.io/test/node/js",
            "example.io/test/node"
        ));
        assert!(feature_matches_prefix(
            "example.io/test/node@sha256:abc",
            "example.io/test/node"
        ));
        assert!(!feature_matches_prefix(
            "example.io/test/nodej",
            "example.io/test/node"
        ));
        assert!(!feature_matches_prefix(
            "example.io/test/node.js",
            "example.io/test/node"
        ));
    }

    #[test]
    fn ensure_no_disallowed_features_accepts_allowed_feature_sets() {
        let root = unique_temp_dir("devcontainer-control-manifest-test");
        let user_data = root.join("user-data");
        std::fs::create_dir_all(&user_data).expect("user data dir");
        let features = Map::from_iter([(
            "ghcr.io/devcontainers/features/git:1".to_string(),
            Value::Object(Map::new()),
        )]);

        ensure_no_disallowed_features(
            &[
                "--user-data-folder".to_string(),
                user_data.display().to_string(),
            ],
            &features,
        )
        .expect("allowed features");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ensure_no_disallowed_features_returns_early_without_declared_features() {
        ensure_no_disallowed_features(&[], &Map::new()).expect("empty feature set");
    }

    #[test]
    fn ensure_no_disallowed_features_defaults_to_an_empty_manifest() {
        let root = unique_temp_dir("devcontainer-control-manifest-test");
        let user_data = root.join("user-data");
        std::fs::create_dir_all(&user_data).expect("user data dir");
        let features = Map::from_iter([(
            "ghcr.io/devcontainers/features/problematic-feature:1".to_string(),
            Value::Object(Map::new()),
        )]);

        ensure_no_disallowed_features(
            &[
                "--user-data-folder".to_string(),
                user_data.display().to_string(),
            ],
            &features,
        )
        .expect("empty default control manifest");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ensure_no_disallowed_features_defaults_to_empty_when_manifest_file_is_missing() {
        let root = unique_temp_dir("devcontainer-control-manifest-test");
        let user_data = root.join("user-data");
        std::fs::create_dir_all(&user_data).expect("user data dir");
        let features = Map::from_iter([(
            "ghcr.io/devcontainers/features/problematic-feature:1".to_string(),
            Value::Object(Map::new()),
        )]);

        ensure_no_disallowed_features(
            &[
                "--user-data-folder".to_string(),
                user_data.display().to_string(),
            ],
            &features,
        )
        .expect("missing control manifest should behave as empty");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ensure_no_disallowed_features_supports_user_data_folder_override() {
        let root = unique_temp_dir("devcontainer-control-manifest-test");
        let user_data = root.join("user-data");
        write_test_control_manifest(&user_data);
        let features = Map::from_iter([(
            "ghcr.io/devcontainers/features/problematic-feature:1".to_string(),
            Value::Object(Map::new()),
        )]);

        let error = ensure_no_disallowed_features(
            &[
                "--user-data-folder".to_string(),
                user_data.display().to_string(),
            ],
            &features,
        )
        .expect_err("user-data control manifest");

        assert!(error.contains("problematic-feature:1"), "{error}");
        assert!(error.contains("https://containers.dev/"), "{error}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn feature_advisories_are_empty_without_a_control_manifest() {
        let root = unique_temp_dir("devcontainer-control-manifest-test");
        let user_data = root.join("user-data");
        std::fs::create_dir_all(&user_data).expect("user data dir");
        let advisories = feature_advisories_for_oci_features(
            &[
                "--user-data-folder".to_string(),
                user_data.display().to_string(),
            ],
            &[(
                "ghcr.io/devcontainers/features/feature-with-advisory".to_string(),
                "1.0.9".to_string(),
            )],
        )
        .expect("advisories");

        assert!(advisories.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn feature_advisories_match_versions_in_range() {
        let root = unique_temp_dir("devcontainer-control-manifest-test");
        let user_data = root.join("user-data");
        write_test_control_manifest(&user_data);
        let args = vec![
            "--user-data-folder".to_string(),
            user_data.display().to_string(),
        ];
        let advisories = feature_advisories_for_oci_features(
            &args,
            &[(
                "ghcr.io/devcontainers/features/feature-with-advisory".to_string(),
                "1.0.9".to_string(),
            )],
        )
        .expect("advisories");

        assert_eq!(advisories.len(), 1);
        assert_eq!(
            advisories[0]["feature"],
            json!({
                "id": "ghcr.io/devcontainers/features/feature-with-advisory",
                "version": "1.0.9"
            })
        );
        assert_eq!(
            advisories[0]["advisories"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(
            advisories[0]["advisories"][0]["introducedInVersion"],
            "1.0.7"
        );
        assert_eq!(advisories[0]["advisories"][0]["fixedInVersion"], "1.1.10");

        let advisories = feature_advisories_for_oci_features(
            &args,
            &[(
                "ghcr.io/devcontainers/features/feature-with-advisory".to_string(),
                "1.1.5".to_string(),
            )],
        )
        .expect("advisories");

        assert_eq!(advisories.len(), 1);
        assert_eq!(advisories[0]["feature"]["version"], "1.1.5");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn feature_advisories_skip_versions_outside_range() {
        let root = unique_temp_dir("devcontainer-control-manifest-test");
        let user_data = root.join("user-data");
        write_test_control_manifest(&user_data);
        let args = vec![
            "--user-data-folder".to_string(),
            user_data.display().to_string(),
        ];
        let advisories = feature_advisories_for_oci_features(
            &args,
            &[(
                "ghcr.io/devcontainers/features/feature-with-advisory".to_string(),
                "1.0.6".to_string(),
            )],
        )
        .expect("advisories");
        assert!(advisories.is_empty());

        let advisories = feature_advisories_for_oci_features(
            &args,
            &[(
                "ghcr.io/devcontainers/features/feature-with-advisory".to_string(),
                "1.1.10".to_string(),
            )],
        )
        .expect("advisories");
        assert!(advisories.is_empty());

        let advisories = feature_advisories_for_oci_features(
            &args,
            &[(
                "ghcr.io/devcontainers/features/other-feature".to_string(),
                "1.0.9".to_string(),
            )],
        )
        .expect("advisories");
        assert!(advisories.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sanitize_control_manifest_filters_invalid_entries() {
        let manifest = sanitize_control_manifest(&json!({
            "disallowedFeatures": [
                { "featureIdPrefix": "example.io/test/node" },
                { "featureIdPrefix": 3 }
            ],
            "featureAdvisories": [
                {
                    "featureId": "example.io/test/node",
                    "introducedInVersion": "1.0.0",
                    "fixedInVersion": "1.1.0",
                    "description": "desc"
                },
                {
                    "featureId": "example.io/test/invalid"
                }
            ]
        }));

        assert_eq!(manifest.disallowed_features.len(), 1);
        assert_eq!(manifest.feature_advisories.len(), 1);
    }

    #[test]
    fn sanitize_control_manifest_defaults_for_non_object_values() {
        let manifest = sanitize_control_manifest(&json!("not-object"));

        assert!(manifest.disallowed_features.is_empty());
        assert!(manifest.feature_advisories.is_empty());
    }

    #[test]
    fn feature_version_matching_rejects_invalid_versions() {
        assert!(!feature_version_is_affected("bad", "1.0.0", "2.0.0"));
        assert!(!feature_version_is_affected("1.0.0", "bad", "2.0.0"));
        assert!(!feature_version_is_affected("1.0.0", "1.0.0", "bad"));
        assert_eq!(parse_version("1.2.3.4"), None);
    }
}
