//! Control-manifest helpers for disallowed Features and future advisory support.

use std::env;
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

    fs::read_to_string(&manifest_path)
        .map_err(io_error_to_string)
        .and_then(|raw| crate::config::parse_jsonc_value(&raw))
        .map(|parsed| sanitize_control_manifest(&parsed))
}

fn control_manifest_path(args: &[String]) -> PathBuf {
    let user_data_folder = match common::runtime_options(args).user_data_folder {
        Some(path) => PathBuf::from(path),
        None => default_user_data_folder(),
    };
    user_data_folder.join("control-manifest.json")
}

fn default_user_data_folder() -> PathBuf {
    default_user_data_folder_impl()
}

#[cfg(target_os = "linux")]
fn default_user_data_folder_impl() -> PathBuf {
    let username = env::var("USER").unwrap_or("unknown".to_string());
    env::temp_dir().join(format!("devcontainercli-{username}"))
}

#[cfg(not(target_os = "linux"))]
fn default_user_data_folder_impl() -> PathBuf {
    env::temp_dir().join("devcontainercli")
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
            .filter_map(disallowed_feature_from_value)
            .collect(),
        feature_advisories: entries
            .get("featureAdvisories")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(feature_advisory_from_value)
            .collect(),
    }
}

fn disallowed_feature_from_value(entry: &Value) -> Option<DisallowedFeature> {
    entry.as_object().and_then(|object| {
        json_string_field(object, "featureIdPrefix").map(|feature_id_prefix| DisallowedFeature {
            feature_id_prefix,
            documentation_url: json_string_field(object, "documentationURL"),
        })
    })
}

fn feature_advisory_from_value(entry: &Value) -> Option<FeatureAdvisory> {
    entry.as_object().and_then(|object| {
        json_string_field(object, "featureId")
            .zip(json_string_field(object, "introducedInVersion"))
            .zip(json_string_field(object, "fixedInVersion"))
            .zip(json_string_field(object, "description"))
            .map(
                |(((feature_id, introduced_in_version), fixed_in_version), description)| {
                    FeatureAdvisory {
                        feature_id,
                        introduced_in_version,
                        fixed_in_version,
                        description,
                        documentation_url: json_string_field(object, "documentationURL"),
                    }
                },
            )
    })
}

fn json_string_field(object: &Map<String, Value>, field: &str) -> Option<String> {
    object
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
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
    const FEATURE_ID_SEPARATORS: &[u8] = b"/:@";

    feature_id.starts_with(prefix)
        && feature_id
            .as_bytes()
            .get(prefix.len())
            .is_none_or(|separator| FEATURE_ID_SEPARATORS.contains(separator))
}

fn feature_version_is_affected(
    feature_version: &str,
    introduced_in_version: &str,
    fixed_in_version: &str,
) -> bool {
    parse_version(feature_version)
        .zip(parse_version(introduced_in_version))
        .zip(parse_version(fixed_in_version))
        .is_some_and(
            |((feature_version, introduced_in_version), fixed_in_version)| {
                feature_version >= introduced_in_version && feature_version < fixed_in_version
            },
        )
}

fn parse_version(input: &str) -> Option<(u64, u64, u64)> {
    let mut parts = input.split('.');
    let major = parts.next().and_then(parse_version_part);
    let minor = parts.next().map_or(Some(0), parse_version_part);
    let patch = parts.next().map_or(Some(0), parse_version_part);
    if parts.next().is_some() {
        return None;
    }
    major
        .zip(minor)
        .zip(patch)
        .map(|((major, minor), patch)| (major, minor, patch))
}

fn parse_version_part(input: &str) -> Option<u64> {
    input.parse().ok()
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

fn io_error_to_string(error: std::io::Error) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Map, Value};

    use super::{
        default_user_data_folder, ensure_no_disallowed_features,
        feature_advisories_for_oci_features, feature_matches_prefix, feature_version_is_affected,
        parse_version, sanitize_control_manifest,
    };
    use crate::commands::common::{test_env_defaults, DEVCONTAINER_USER_DATA_FOLDER};
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
    fn ensure_no_disallowed_features_accepts_empty_feature_sets_without_manifest_lookup() {
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
    fn ensure_no_disallowed_features_reports_unreadable_control_manifest_content() {
        let root = unique_temp_dir("devcontainer-control-manifest-test");
        let user_data = root.join("user-data");
        std::fs::create_dir_all(&user_data).expect("user data dir");
        std::fs::write(user_data.join("control-manifest.json"), [0xff]).expect("control manifest");
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
        .expect_err("invalid UTF-8 manifest should report read error");

        assert!(
            error.to_ascii_lowercase().contains("utf"),
            "unexpected error: {error}"
        );
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
    fn ensure_no_disallowed_features_uses_env_user_data_folder_default() {
        let root = unique_temp_dir("devcontainer-control-manifest-test");
        let user_data = root.join("user-data");
        write_test_control_manifest(&user_data);
        let user_data_path = user_data.display().to_string();
        let _env = test_env_defaults(&[(DEVCONTAINER_USER_DATA_FOLDER, user_data_path.as_str())]);
        let features = Map::from_iter([(
            "ghcr.io/devcontainers/features/problematic-feature:1".to_string(),
            Value::Object(Map::new()),
        )]);

        let error = ensure_no_disallowed_features(&[], &features)
            .expect_err("env user-data control manifest");

        assert!(error.contains("problematic-feature:1"), "{error}");
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
        assert_eq!(sanitize_control_manifest(&json!(null)), Default::default());

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
    fn version_helpers_reject_malformed_advisory_versions() {
        assert!(!feature_version_is_affected("bad", "1.0.0", "2.0.0"));
        assert!(!feature_version_is_affected("1.0.0", "bad", "2.0.0"));
        assert!(!feature_version_is_affected("1.0.0", "1.0.0", "bad"));
        assert_eq!(parse_version("1.2.3.4"), None);
        #[cfg(target_os = "linux")]
        assert!(default_user_data_folder()
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.starts_with("devcontainercli-")));
        #[cfg(not(target_os = "linux"))]
        assert!(default_user_data_folder().ends_with("devcontainercli"));
    }
}
