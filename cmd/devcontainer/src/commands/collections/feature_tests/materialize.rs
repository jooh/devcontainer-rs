//! Feature test materialization and build context helpers.

use std::fs;
use std::path::Component;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};

use super::{BaseImageSource, FeatureInstallation, FeatureInstallationSource, FeatureTestOptions};
use crate::commands::common;

static NEXT_FEATURE_TEST_ID: AtomicU64 = AtomicU64::new(0);

pub(super) fn scenario_base_image(
    options: &FeatureTestOptions,
    scenario_dir: &str,
    config: &Value,
    workspace_dir: &Path,
) -> Result<BaseImageSource, String> {
    if let Some(image) = config.get("image").and_then(Value::as_str) {
        return Ok(BaseImageSource::Image(image.to_string()));
    }

    let Some(build) = config.get("build").and_then(Value::as_object) else {
        return Ok(BaseImageSource::Image(options.base_image.clone()));
    };
    let config_root = scenario_config_root(workspace_dir, scenario_dir);
    let dockerfile = build
        .get("dockerfile")
        .or_else(|| build.get("dockerFile"))
        .and_then(Value::as_str)
        .unwrap_or("Dockerfile");
    let context = build.get("context").and_then(Value::as_str).unwrap_or(".");
    Ok(BaseImageSource::Build {
        dockerfile_path: resolve_relative_path(&config_root, dockerfile),
        context_path: resolve_relative_path(&config_root, context),
    })
}

pub(super) fn scenario_feature_installations(
    project_folder: &Path,
    default_feature: Option<&str>,
    config: &Value,
) -> Result<Vec<FeatureInstallation>, String> {
    let features = if let Some(features) = config.get("features").and_then(Value::as_object) {
        features.clone()
    } else if let Some(default_feature) = default_feature {
        let mut features = Map::new();
        features.insert(default_feature.to_string(), Value::Object(Map::new()));
        features
    } else {
        return Err("Scenario is missing features".to_string());
    };

    let mut installations = Vec::with_capacity(features.len());
    for (feature_id, value) in &features {
        if feature_id.starts_with('.') {
            return Err(format!(
                "Unsupported relative feature in test scenario: {feature_id}"
            ));
        }
        if feature_id.contains('/') {
            installations.push(published_feature_installation(feature_id, value)?);
            continue;
        }
        installations.push(feature_installation(
            &project_folder.join("src").join(feature_id),
            value,
        )?);
    }
    Ok(installations)
}

pub(super) fn feature_installation(
    feature_dir: &Path,
    value: &Value,
) -> Result<FeatureInstallation, String> {
    if !feature_dir.is_dir() {
        return Err(format!(
            "Feature source directory not found at {}",
            feature_dir.display()
        ));
    }
    Ok(FeatureInstallation {
        source: FeatureInstallationSource::Local(feature_dir.to_path_buf()),
        env: feature_option_values(feature_dir, value)?,
    })
}

fn published_feature_installation(
    feature_id: &str,
    value: &Value,
) -> Result<FeatureInstallation, String> {
    let manifest = super::super::registry::published_feature_manifest(feature_id)
        .ok_or_else(|| format!("Unknown published feature: {feature_id}"))?;
    Ok(FeatureInstallation {
        source: FeatureInstallationSource::Published(feature_id.to_string()),
        env: feature_option_values_from_manifest(&manifest, value),
    })
}

pub(super) fn feature_option_values(
    feature_dir: &Path,
    value: &Value,
) -> Result<Vec<(String, String)>, String> {
    let manifest = common::parse_manifest(feature_dir, "devcontainer-feature.json")?;
    Ok(feature_option_values_from_manifest(&manifest, value))
}

fn feature_option_values_from_manifest(manifest: &Value, value: &Value) -> Vec<(String, String)> {
    let defaults = manifest
        .get("options")
        .and_then(Value::as_object)
        .map(|options| {
            options
                .iter()
                .filter_map(|(key, option)| {
                    option.get("default").map(|default| {
                        (
                            common::feature_option_env_name(key),
                            json_value_to_env(default),
                        )
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let overrides = value
        .as_object()
        .map(|options| {
            options
                .iter()
                .map(|(key, option)| {
                    (
                        common::feature_option_env_name(key),
                        json_value_to_env(option),
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let mut merged = Map::new();
    for (key, value) in defaults.into_iter().chain(overrides) {
        merged.insert(key, Value::String(value));
    }
    merged
        .into_iter()
        .filter_map(|(key, value)| value.as_str().map(|text| (key, text.to_string())))
        .collect()
}

pub(super) fn alternate_feature_option_values(
    feature_dir: &Path,
    permit_randomization: bool,
) -> Result<Vec<(String, String)>, String> {
    let manifest = common::parse_manifest(feature_dir, "devcontainer-feature.json")?;
    let Some(options) = manifest.get("options").and_then(Value::as_object) else {
        return Ok(Vec::new());
    };

    let mut values = Vec::new();
    for (key, option) in options {
        let env_name = common::feature_option_env_name(key);
        let default = option.get("default");
        let value = match option.get("type").and_then(Value::as_str) {
            Some("boolean") => {
                let default = default.and_then(Value::as_bool).unwrap_or(false);
                (!default).to_string()
            }
            Some("string") => {
                if let Some(candidates) = option
                    .get("proposals")
                    .or_else(|| option.get("enum"))
                    .and_then(Value::as_array)
                {
                    let default = default.map(json_value_to_env);
                    choose_alternate_string_candidate(
                        candidates,
                        default.as_deref(),
                        permit_randomization,
                    )
                    .or(default)
                    .unwrap_or_default()
                } else {
                    default.map(json_value_to_env).unwrap_or_default()
                }
            }
            _ => default.map(json_value_to_env).unwrap_or_default(),
        };
        if !value.is_empty() {
            values.push((env_name, value));
        }
    }
    Ok(values)
}

fn json_value_to_env(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(boolean) => boolean.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.clone(),
        _ => value.to_string(),
    }
}

fn choose_alternate_string_candidate(
    candidates: &[Value],
    default: Option<&str>,
    permit_randomization: bool,
) -> Option<String> {
    let values = candidates.iter().map(json_value_to_env).collect::<Vec<_>>();
    if values.is_empty() {
        return None;
    }

    let default_index =
        default.and_then(|default| values.iter().position(|value| value == default));
    let alternate_indexes = values
        .iter()
        .enumerate()
        .filter_map(|(index, _)| (Some(index) != default_index).then_some(index))
        .collect::<Vec<_>>();
    if alternate_indexes.is_empty() {
        return values.first().cloned();
    }

    let selected_index = if permit_randomization {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time went backwards")
            .as_nanos() as usize;
        alternate_indexes[seed % alternate_indexes.len()]
    } else {
        alternate_indexes[0]
    };
    values.get(selected_index).cloned()
}

pub(super) fn write_feature_test_dockerfile(
    build_context_dir: &Path,
    base_image: &str,
    installations: &[FeatureInstallation],
) -> Result<PathBuf, String> {
    let dockerfile_path = build_context_dir.join("Dockerfile");
    let mut dockerfile = format!("FROM {base_image}\n");
    for (index, installation) in installations.iter().enumerate() {
        let feature_name = feature_installation_name(installation);
        let destination = format!("feature-{index}-{feature_name}");
        let copied_feature_dir = build_context_dir.join(&destination);
        materialize_feature_installation(installation, &copied_feature_dir)?;
        let install_path = format!("/tmp/devcontainer-features/{destination}");
        dockerfile.push_str(&format!("COPY {destination} {install_path}\n"));
        let env_assignments = installation
            .env
            .iter()
            .map(|(key, value)| format!("{key}={}", shell_single_quote(value)))
            .collect::<Vec<_>>()
            .join(" ");
        let command = if env_assignments.is_empty() {
            "chmod +x install.sh && ./install.sh".to_string()
        } else {
            format!("chmod +x install.sh && {env_assignments} ./install.sh")
        };
        dockerfile.push_str(&format!(
            "RUN cd {install_path} && /bin/sh -lc {}\n",
            shell_single_quote(&command)
        ));
    }
    fs::write(&dockerfile_path, dockerfile).map_err(|error| error.to_string())?;
    Ok(dockerfile_path)
}

fn feature_installation_name(installation: &FeatureInstallation) -> String {
    match &installation.source {
        FeatureInstallationSource::Local(path) => path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("feature")
            .to_string(),
        FeatureInstallationSource::Published(feature_id) => {
            super::super::registry::collection_slug(feature_id)
                .unwrap_or_else(|| "published-feature".to_string())
        }
    }
}

fn materialize_feature_installation(
    installation: &FeatureInstallation,
    destination: &Path,
) -> Result<(), String> {
    match &installation.source {
        FeatureInstallationSource::Local(path) => materialize_local_feature(path, destination),
        FeatureInstallationSource::Published(feature_id) => {
            materialize_published_feature(feature_id, destination)
        }
    }
}

fn materialize_local_feature(source: &Path, destination: &Path) -> Result<(), String> {
    common::copy_directory_recursive(source, destination)?;
    ensure_feature_install_script(destination)
}

fn materialize_published_feature(feature_id: &str, destination: &Path) -> Result<(), String> {
    let manifest = super::super::registry::published_feature_manifest(feature_id)
        .ok_or_else(|| format!("Unknown published feature: {feature_id}"))?;
    fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    fs::write(
        destination.join("devcontainer-feature.json"),
        serde_json::to_string_pretty(&manifest).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    fs::write(
        destination.join("install.sh"),
        super::super::registry::published_feature_install_script(feature_id),
    )
    .map_err(|error| error.to_string())?;
    ensure_feature_install_script(destination)
}

fn ensure_feature_install_script(destination: &Path) -> Result<(), String> {
    let install_path = destination.join("install.sh");
    if install_path.is_file() {
        return Ok(());
    }
    fs::write(&install_path, "#!/bin/sh\nset -eu\n").map_err(|error| error.to_string())
}

pub(super) fn unique_feature_test_name(prefix: &str) -> String {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time went backwards")
        .as_nanos();
    let unique_id = NEXT_FEATURE_TEST_ID.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{}-{suffix}-{unique_id}", std::process::id())
}

pub(super) fn unique_feature_test_dir() -> PathBuf {
    std::env::temp_dir().join(unique_feature_test_name(
        "devcontainer-feature-test-workspace",
    ))
}

fn resolve_relative_path(root: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

fn scenario_config_root(workspace_dir: &Path, scenario_dir: &str) -> PathBuf {
    let path = Path::new(scenario_dir);
    if path
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        let candidate = workspace_dir.join(path);
        if candidate.is_dir() {
            return candidate;
        }
    }
    workspace_dir.to_path_buf()
}

pub(super) fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use serde_json::json;

    use super::{
        alternate_feature_option_values, choose_alternate_string_candidate, feature_option_values,
        scenario_base_image, scenario_feature_installations, shell_single_quote,
        unique_feature_test_dir, write_feature_test_dockerfile, BaseImageSource,
        FeatureInstallation, FeatureInstallationSource, FeatureTestOptions,
    };

    fn test_options(project_folder: &Path) -> FeatureTestOptions {
        FeatureTestOptions {
            project_folder: project_folder.to_path_buf(),
            base_image: "example/base:latest".to_string(),
            remote_user: None,
            preserve_test_containers: false,
            permit_randomization: false,
            quiet: true,
        }
    }

    fn write_feature_manifest(feature_dir: &Path) {
        fs::create_dir_all(feature_dir).expect("feature dir");
        fs::write(
            feature_dir.join("devcontainer-feature.json"),
            r#"{
  "id": "demo",
  "version": "1.0.0",
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

    #[test]
    fn choose_alternate_string_candidate_prefers_first_non_default_without_randomization() {
        let selected = choose_alternate_string_candidate(
            &json!(["blue", "green", "red"])
                .as_array()
                .expect("array")
                .clone(),
            Some("blue"),
            false,
        );

        assert_eq!(selected.as_deref(), Some("green"));
    }

    #[test]
    fn choose_alternate_string_candidate_returns_non_default_with_randomization() {
        let selected = choose_alternate_string_candidate(
            &json!(["blue", "green", "red"])
                .as_array()
                .expect("array")
                .clone(),
            Some("blue"),
            true,
        )
        .expect("selection");

        assert!(selected == "green" || selected == "red");
    }

    #[test]
    fn choose_alternate_string_candidate_handles_empty_and_single_candidate_lists() {
        let empty = choose_alternate_string_candidate(&[], None, false);
        let single = choose_alternate_string_candidate(
            &json!(["only"]).as_array().expect("array").clone(),
            Some("only"),
            false,
        );

        assert_eq!(empty, None);
        assert_eq!(single.as_deref(), Some("only"));
    }

    #[test]
    fn alternate_feature_option_values_uses_first_non_default_by_default() {
        let feature_dir = unique_feature_test_dir();
        fs::create_dir_all(&feature_dir).expect("feature dir");
        fs::write(
            feature_dir.join("devcontainer-feature.json"),
            r#"{
  "id": "demo",
  "version": "1.0.0",
  "options": {
    "color": {
      "type": "string",
      "enum": ["blue", "green", "red"],
      "default": "blue"
    }
  }
}"#,
        )
        .expect("manifest");

        let values = alternate_feature_option_values(&feature_dir, false).expect("values");

        assert_eq!(values, vec![("COLOR".to_string(), "green".to_string())]);
        let _ = fs::remove_dir_all(feature_dir);
    }

    #[test]
    fn alternate_feature_option_values_returns_empty_without_manifest_options() {
        let feature_dir = unique_feature_test_dir();
        fs::create_dir_all(&feature_dir).expect("feature dir");
        fs::write(
            feature_dir.join("devcontainer-feature.json"),
            r#"{"id":"demo","version":"1.0.0"}"#,
        )
        .expect("manifest");

        let values = alternate_feature_option_values(&feature_dir, false).expect("values");

        assert!(values.is_empty());
        let _ = fs::remove_dir_all(feature_dir);
    }

    #[test]
    fn alternate_feature_option_values_handles_boolean_and_json_defaults() {
        let feature_dir = unique_feature_test_dir();
        fs::create_dir_all(&feature_dir).expect("feature dir");
        fs::write(
            feature_dir.join("devcontainer-feature.json"),
            r#"{
  "id": "demo",
  "version": "1.0.0",
  "options": {
    "array": {
      "default": ["a", "b"]
    },
    "disabled": {
      "type": "boolean",
      "default": true
    },
    "enabled": {
      "type": "boolean",
      "default": false
    },
    "emptyEnum": {
      "type": "string",
      "enum": []
    },
    "nullDefault": {
      "default": null
    },
    "number": {
      "default": 7
    },
    "object": {
      "default": { "nested": true }
    },
    "single": {
      "type": "string",
      "enum": ["only"],
      "default": "only"
    },
    "stringDefault": {
      "type": "string",
      "default": "plain"
    }
  }
}"#,
        )
        .expect("manifest");

        let values = alternate_feature_option_values(&feature_dir, false).expect("values");

        assert!(values.contains(&("ARRAY".to_string(), r#"["a","b"]"#.to_string())));
        assert!(values.contains(&("DISABLED".to_string(), "false".to_string())));
        assert!(values.contains(&("ENABLED".to_string(), "true".to_string())));
        assert!(values.contains(&("NUMBER".to_string(), "7".to_string())));
        assert!(values.contains(&("OBJECT".to_string(), r#"{"nested":true}"#.to_string())));
        assert!(values.contains(&("SINGLE".to_string(), "only".to_string())));
        assert!(values.contains(&("STRINGDEFAULT".to_string(), "plain".to_string())));
        assert!(!values.iter().any(|(key, _)| key == "EMPTYENUM"));
        assert!(!values.iter().any(|(key, _)| key == "NULLDEFAULT"));
        let _ = fs::remove_dir_all(feature_dir);
    }

    #[test]
    fn feature_test_option_env_names_match_upstream_safe_id_cases() {
        let feature_dir = unique_feature_test_dir();
        fs::create_dir_all(&feature_dir).expect("feature dir");
        fs::write(
            feature_dir.join("devcontainer-feature.json"),
            r#"{
  "id": "demo",
  "version": "1.0.0",
  "options": {
    "1name": {
      "type": "string",
      "default": "default-value"
    },
    "option-name": {
      "type": "string",
      "default": "default-option"
    }
  }
}"#,
        )
        .expect("manifest");

        let values = feature_option_values(
            &feature_dir,
            &json!({
                "1name": "override-value"
            }),
        )
        .expect("values");

        assert!(values.contains(&("_NAME".to_string(), "override-value".to_string())));
        assert!(values.contains(&("OPTION_NAME".to_string(), "default-option".to_string())));
        assert!(!values.iter().any(|(key, _)| key == "1NAME"));
        let _ = fs::remove_dir_all(feature_dir);
    }

    #[test]
    fn scenario_base_image_resolves_image_default_and_build_paths() {
        let workspace = unique_feature_test_dir();
        let scenario_dir = workspace.join("scenarios").join("basic");
        fs::create_dir_all(&scenario_dir).expect("scenario dir");
        let absolute_dockerfile = workspace.join("absolute.Dockerfile");
        let absolute_context = workspace.join("absolute-context");
        let options = test_options(&workspace);
        let explicit = scenario_base_image(
            &options,
            "scenarios/basic",
            &json!({
                "image": "ubuntu:24.04"
            }),
            &workspace,
        )
        .expect("explicit image");
        let default = scenario_base_image(&options, "scenarios/basic", &json!({}), &workspace)
            .expect("default image");
        let build = scenario_base_image(
            &options,
            "scenarios/basic",
            &json!({
                "build": {
                    "dockerFile": "Dockerfile.feature",
                    "context": ".."
                }
            }),
            &workspace,
        )
        .expect("build image");
        let escaped = scenario_base_image(
            &options,
            "../outside",
            &json!({
                "build": {}
            }),
            &workspace,
        )
        .expect("escaped scenario");
        let absolute_build = scenario_base_image(
            &options,
            "scenarios/basic",
            &json!({
                "build": {
                    "dockerfile": absolute_dockerfile,
                    "context": absolute_context
                }
            }),
            &workspace,
        )
        .expect("absolute build paths");

        assert_eq!(explicit, BaseImageSource::Image("ubuntu:24.04".to_string()));
        assert_eq!(
            default,
            BaseImageSource::Image("example/base:latest".to_string())
        );
        assert_eq!(
            build,
            BaseImageSource::Build {
                dockerfile_path: scenario_dir.join("Dockerfile.feature"),
                context_path: scenario_dir.join("..")
            }
        );
        assert_eq!(
            escaped,
            BaseImageSource::Build {
                dockerfile_path: workspace.join("Dockerfile"),
                context_path: workspace.join(".")
            }
        );
        assert_eq!(
            absolute_build,
            BaseImageSource::Build {
                dockerfile_path: absolute_dockerfile,
                context_path: absolute_context
            }
        );
        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn scenario_feature_installations_resolve_default_local_and_published_features() {
        let project = unique_feature_test_dir();
        write_feature_manifest(&project.join("src").join("demo"));

        let default = scenario_feature_installations(&project, Some("demo"), &json!({}))
            .expect("default feature");
        let configured = scenario_feature_installations(
            &project,
            None,
            &json!({
                "features": {
                    "demo": {
                        "flag": true
                    },
                    "ghcr.io/devcontainers/features/common-utils:2": {
                        "installZsh": "false"
                    }
                }
            }),
        )
        .expect("configured features");

        assert_eq!(default.len(), 1);
        assert!(matches!(
            default[0].source,
            FeatureInstallationSource::Local(_)
        ));
        assert!(default[0]
            .env
            .contains(&("FLAG".to_string(), "false".to_string())));
        assert_eq!(configured.len(), 2);
        assert!(configured.iter().any(|installation| matches!(
            installation.source,
            FeatureInstallationSource::Local(_)
        )));
        assert!(configured.iter().any(|installation| matches!(
            installation.source,
            FeatureInstallationSource::Published(_)
        )));
        assert!(configured.iter().any(|installation| installation
            .env
            .contains(&("INSTALLZSH".to_string(), "false".to_string()))));
        let _ = fs::remove_dir_all(project);
    }

    #[test]
    fn scenario_feature_installations_report_missing_and_unsupported_sources() {
        let project = unique_feature_test_dir();
        let missing_features =
            scenario_feature_installations(&project, None, &json!({})).unwrap_err();
        let relative_feature = scenario_feature_installations(
            &project,
            None,
            &json!({
                "features": {
                    "./demo": {}
                }
            }),
        )
        .unwrap_err();
        let missing_local = scenario_feature_installations(
            &project,
            None,
            &json!({
                "features": {
                    "demo": {}
                }
            }),
        )
        .unwrap_err();
        let unknown_published = scenario_feature_installations(
            &project,
            None,
            &json!({
                "features": {
                    "ghcr.io/example/notfeatures/missing": {}
                }
            }),
        )
        .unwrap_err();

        assert_eq!(missing_features, "Scenario is missing features");
        assert_eq!(
            relative_feature,
            "Unsupported relative feature in test scenario: ./demo"
        );
        assert!(missing_local.contains("Feature source directory not found at"));
        assert_eq!(
            unknown_published,
            "Unknown published feature: ghcr.io/example/notfeatures/missing"
        );
    }

    #[test]
    fn write_feature_test_dockerfile_materializes_local_and_published_features() {
        let workspace = unique_feature_test_dir();
        let build_context = workspace.join("build");
        let local_feature = workspace.join("local-feature");
        fs::create_dir_all(&build_context).expect("build context");
        write_feature_manifest(&local_feature);
        let dockerfile_path = write_feature_test_dockerfile(
            &build_context,
            "debian:bookworm-slim",
            &[
                FeatureInstallation {
                    source: FeatureInstallationSource::Local(local_feature),
                    env: vec![("GREETING".to_string(), "it's fine".to_string())],
                },
                FeatureInstallation {
                    source: FeatureInstallationSource::Published(
                        "ghcr.io/devcontainers/features/common-utils:2".to_string(),
                    ),
                    env: Vec::new(),
                },
            ],
        )
        .expect("dockerfile");

        let dockerfile = fs::read_to_string(dockerfile_path).expect("dockerfile contents");
        assert!(dockerfile.starts_with("FROM debian:bookworm-slim\n"));
        assert!(dockerfile.contains("COPY feature-0-local-feature"));
        assert!(dockerfile.contains("COPY feature-1-common-utils"));
        assert!(dockerfile.contains("GREETING="));
        assert!(build_context
            .join("feature-0-local-feature")
            .join("install.sh")
            .is_file());
        assert!(build_context
            .join("feature-1-common-utils")
            .join("devcontainer-feature.json")
            .is_file());
        assert_eq!(shell_single_quote("it's fine"), "'it'\"'\"'s fine'");
        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn write_feature_test_dockerfile_reports_unknown_published_feature() {
        let workspace = unique_feature_test_dir();
        fs::create_dir_all(&workspace).expect("workspace");
        let error = write_feature_test_dockerfile(
            &workspace,
            "debian:bookworm-slim",
            &[FeatureInstallation {
                source: FeatureInstallationSource::Published(
                    "ghcr.io/example/notfeatures/missing".to_string(),
                ),
                env: Vec::new(),
            }],
        )
        .unwrap_err();

        assert_eq!(
            error,
            "Unknown published feature: ghcr.io/example/notfeatures/missing"
        );
        let _ = fs::remove_dir_all(workspace);
    }
}
