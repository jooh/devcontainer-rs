//! Template collection discovery, rendering, and write helpers.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use super::registry::{
    embedded_template_source_dir, local_oci_artifact, normalize_collection_reference,
    published_template_manifest_with_workspace,
};
use crate::commands::common;
use crate::process_runner::{self, ProcessLogLevel, ProcessRequest};

const DEFAULT_PUBLISHED_TEMPLATE_BASE_IMAGE: &str = "docker.io/library/debian:bookworm-slim";
static NEXT_TEMPLATE_TMP_ID: AtomicU64 = AtomicU64::new(0);

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn apply_template_target(
    template_root: &Path,
    workspace_root: &Path,
) -> Result<Value, String> {
    apply_template_target_with_options(template_root, workspace_root, &[], None)
}

fn apply_template_target_with_options(
    template_root: &Path,
    workspace_root: &Path,
    omit_paths: &[String],
    tmp_dir: Option<&Path>,
) -> Result<Value, String> {
    let manifest = common::parse_manifest(template_root, "devcontainer-template.json")?;
    let source_root = prepare_template_source_root(&template_root.join("src"), tmp_dir)?;
    copy_embedded_template_contents(&source_root, workspace_root, &Map::new(), omit_paths)?;
    Ok(json!({
        "outcome": "success",
        "id": manifest.get("id").cloned().unwrap_or_else(|| Value::String("unknown".to_string())),
        "appliedTo": workspace_root,
    }))
}

pub(super) fn build_template_metadata_payload(
    template_path: &str,
    workspace_folder: Option<&Path>,
) -> Result<Value, String> {
    let manifest = if template_path.starts_with("ghcr.io/") {
        published_template_manifest_with_workspace(template_path, workspace_folder)
            .ok_or_else(|| format!("Unknown published template: {template_path}"))?
    } else {
        common::parse_manifest(Path::new(template_path), "devcontainer-template.json")?
    };
    Ok(json!({
        "id": manifest.get("id").cloned().unwrap_or_else(|| Value::String("unknown".to_string())),
        "name": manifest.get("name").cloned().unwrap_or_else(|| Value::String("unknown".to_string())),
        "description": manifest.get("description").cloned().unwrap_or_else(|| Value::String(String::new())),
    }))
}

pub(super) fn run_template_apply(args: &[String]) -> Result<Value, String> {
    let omit_paths = common::parse_json_string_array_option(args, "--omit-paths")?;
    let tmp_dir = common::parse_option_value(args, "--tmp-dir").map(PathBuf::from);
    let template_id = common::parse_option_value(args, "--template-id");
    if let Some(template_id) = template_id {
        let workspace = common::parse_option_value(args, "--workspace-folder")
            .map(PathBuf::from)
            .or_else(|| env::current_dir().ok())
            .ok_or_else(|| "Unable to determine workspace folder".to_string())?;
        return apply_catalog_template_with_options(
            &template_id,
            &workspace,
            args,
            &omit_paths,
            tmp_dir.as_deref(),
        );
    }

    let target = args
        .first()
        .ok_or_else(|| "templates apply requires <target>".to_string())?;
    let workspace = common::parse_option_value(args, "--workspace-folder")
        .map(PathBuf::from)
        .or_else(|| env::current_dir().ok())
        .ok_or_else(|| "Unable to determine workspace folder".to_string())?;
    apply_template_target_with_options(
        Path::new(target),
        &workspace,
        &omit_paths,
        tmp_dir.as_deref(),
    )
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn apply_catalog_template(
    template_id: &str,
    workspace_root: &Path,
    args: &[String],
) -> Result<Value, String> {
    apply_catalog_template_with_options(template_id, workspace_root, args, &[], None)
}

fn apply_catalog_template_with_options(
    template_id: &str,
    workspace_root: &Path,
    args: &[String],
    omit_paths: &[String],
    tmp_dir: Option<&Path>,
) -> Result<Value, String> {
    let manifest = published_template_manifest_with_workspace(template_id, Some(workspace_root))
        .ok_or_else(|| format!("Unknown published template: {template_id}"))?;
    let template_args = common::parse_option_value(args, "--template-args")
        .map(|value| crate::config::parse_jsonc_value(&value))
        .transpose()?
        .unwrap_or_else(|| json!({}));
    let extra_features = common::parse_option_value(args, "--features")
        .map(|value| crate::config::parse_jsonc_value(&value))
        .transpose()?
        .unwrap_or_else(|| json!([]));

    if let Some(source_root) =
        extract_local_published_template_source_root(template_id, workspace_root, tmp_dir)?
    {
        return apply_embedded_published_template(
            &manifest,
            &source_root,
            workspace_root,
            &template_args,
            extra_features,
            omit_paths,
            None,
        );
    }

    let normalized_template_id = normalize_collection_reference(template_id);
    if normalized_template_id != "ghcr.io/devcontainers/templates/docker-from-docker" {
        if let Some(template_root) = embedded_template_source_dir(&normalized_template_id) {
            return apply_embedded_published_template(
                &manifest,
                &template_root,
                workspace_root,
                &template_args,
                extra_features,
                omit_paths,
                tmp_dir,
            );
        }
        return apply_generic_published_template(&manifest, workspace_root, extra_features);
    }

    let mut features = Map::new();
    features.insert(
        "ghcr.io/devcontainers/features/common-utils:1".to_string(),
        json!({
            "installZsh": template_args.get("installZsh").cloned().unwrap_or_else(|| Value::String("true".to_string())),
            "upgradePackages": template_args.get("upgradePackages").cloned().unwrap_or_else(|| Value::String("false".to_string())),
        }),
    );
    features.insert(
        "ghcr.io/devcontainers/features/docker-from-docker:1".to_string(),
        json!({
            "version": template_args.get("dockerVersion").cloned().unwrap_or_else(|| Value::String("latest".to_string())),
            "moby": template_args.get("moby").cloned().unwrap_or_else(|| Value::String("true".to_string())),
            "enableNonRootDocker": template_args.get("enableNonRootDocker").cloned().unwrap_or_else(|| Value::String("true".to_string())),
        }),
    );
    if let Some(extra_features) = extra_features.as_array() {
        for feature in extra_features {
            let Some(id) = feature.get("id").and_then(Value::as_str) else {
                continue;
            };
            features.insert(
                id.to_string(),
                feature.get("options").cloned().unwrap_or_else(|| json!({})),
            );
        }
    }

    let devcontainer = json!({
        "name": manifest.get("name").cloned().unwrap_or_else(|| Value::String("Docker from Docker".to_string())),
        "image": DEFAULT_PUBLISHED_TEMPLATE_BASE_IMAGE,
        "features": features,
    });
    let config_dir = workspace_root.join(".devcontainer");
    fs::create_dir_all(&config_dir).map_err(|error| error.to_string())?;
    fs::write(
        config_dir.join("devcontainer.json"),
        serde_json::to_string_pretty(&devcontainer).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;

    Ok(json!({
        "files": ["./.devcontainer/devcontainer.json"],
    }))
}

fn extract_local_published_template_source_root(
    template_id: &str,
    workspace_root: &Path,
    tmp_dir: Option<&Path>,
) -> Result<Option<PathBuf>, String> {
    let Some(artifact) = local_oci_artifact(template_id, Some(workspace_root)) else {
        return Ok(None);
    };
    let Some(layer_path) = artifact.layer_path else {
        return Err(format!(
            "Published template OCI manifest is missing a layer: {template_id}"
        ));
    };

    let extraction_root = tmp_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir)
        .join(unique_template_tmp_name());
    fs::create_dir_all(&extraction_root).map_err(|error| error.to_string())?;
    let result = process_runner::run_process(&ProcessRequest {
        program: "tar".to_string(),
        args: vec![
            "-xzf".to_string(),
            layer_path.display().to_string(),
            "-C".to_string(),
            extraction_root.display().to_string(),
        ],
        cwd: None,
        env: std::collections::HashMap::new(),
        log_level: ProcessLogLevel::Info,
    })
    .map_err(|error| error.to_string())?;
    if result.status_code != 0 {
        return Err(result.stderr);
    }

    let source_root = if extraction_root.join("src").is_dir() {
        extraction_root.join("src")
    } else {
        extraction_root
    };
    Ok(Some(source_root))
}

fn apply_embedded_published_template(
    manifest: &Value,
    template_root: &Path,
    workspace_root: &Path,
    template_args: &Value,
    extra_features: Value,
    omit_paths: &[String],
    tmp_dir: Option<&Path>,
) -> Result<Value, String> {
    let template_options = template_option_values(manifest, template_args);
    let source_root = prepare_template_source_root(template_root, tmp_dir)?;
    copy_embedded_template_contents(&source_root, workspace_root, &template_options, omit_paths)?;
    merge_extra_features_into_template(workspace_root, extra_features)?;
    Ok(json!({
        "outcome": "success",
        "id": manifest.get("id").cloned().unwrap_or_else(|| Value::String("unknown".to_string())),
        "appliedTo": workspace_root,
    }))
}

fn apply_generic_published_template(
    manifest: &Value,
    workspace_root: &Path,
    extra_features: Value,
) -> Result<Value, String> {
    let mut devcontainer = Map::new();
    devcontainer.insert(
        "name".to_string(),
        manifest
            .get("name")
            .cloned()
            .unwrap_or_else(|| Value::String("Published Template".to_string())),
    );
    devcontainer.insert(
        "image".to_string(),
        Value::String(DEFAULT_PUBLISHED_TEMPLATE_BASE_IMAGE.to_string()),
    );

    let mut features = Map::new();
    if let Some(extra_features) = extra_features.as_array() {
        for feature in extra_features {
            let Some(id) = feature.get("id").and_then(Value::as_str) else {
                continue;
            };
            features.insert(
                id.to_string(),
                feature.get("options").cloned().unwrap_or_else(|| json!({})),
            );
        }
    }
    if !features.is_empty() {
        devcontainer.insert("features".to_string(), Value::Object(features));
    }

    let config_dir = workspace_root.join(".devcontainer");
    fs::create_dir_all(&config_dir).map_err(|error| error.to_string())?;
    fs::write(
        config_dir.join("devcontainer.json"),
        serde_json::to_string_pretty(&Value::Object(devcontainer))
            .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;

    Ok(json!({
        "files": ["./.devcontainer/devcontainer.json"],
    }))
}

fn template_option_values(manifest: &Value, template_args: &Value) -> Map<String, Value> {
    let mut options = manifest
        .get("options")
        .and_then(Value::as_object)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|(name, definition)| {
                    definition
                        .get("default")
                        .cloned()
                        .map(|value| (name.clone(), value))
                })
                .collect::<Map<String, Value>>()
        })
        .unwrap_or_default();
    if let Some(template_args) = template_args.as_object() {
        for (name, value) in template_args {
            options.insert(name.clone(), value.clone());
        }
    }
    options
}

fn copy_embedded_template_contents(
    template_root: &Path,
    workspace_root: &Path,
    template_options: &Map<String, Value>,
    omit_paths: &[String],
) -> Result<(), String> {
    fs::create_dir_all(workspace_root).map_err(|error| error.to_string())?;
    for entry in fs::read_dir(template_root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry.file_name() == "devcontainer-template.json" {
            continue;
        }
        let relative_path = PathBuf::from(entry.file_name());
        copy_embedded_template_entry(
            &entry.path(),
            &workspace_root.join(entry.file_name()),
            template_options,
            &relative_path,
            omit_paths,
        )?;
    }
    Ok(())
}

fn copy_embedded_template_entry(
    source: &Path,
    destination: &Path,
    template_options: &Map<String, Value>,
    relative_path: &Path,
    omit_paths: &[String],
) -> Result<(), String> {
    if template_path_is_omitted(relative_path, omit_paths) {
        return Ok(());
    }
    if source.is_dir() {
        fs::create_dir_all(destination).map_err(|error| error.to_string())?;
        for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let child_relative_path = relative_path.join(entry.file_name());
            copy_embedded_template_entry(
                &entry.path(),
                &destination.join(entry.file_name()),
                template_options,
                &child_relative_path,
                omit_paths,
            )?;
        }
        return Ok(());
    }

    let bytes = fs::read(source).map_err(|error| error.to_string())?;
    if let Ok(text) = String::from_utf8(bytes) {
        let substituted = substitute_template_options(&text, template_options);
        fs::write(destination, substituted).map_err(|error| error.to_string())?;
    } else {
        fs::copy(source, destination).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn prepare_template_source_root(
    source_root: &Path,
    tmp_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    let Some(tmp_dir) = tmp_dir else {
        return Ok(source_root.to_path_buf());
    };
    fs::create_dir_all(tmp_dir).map_err(|error| error.to_string())?;
    let scratch_root = tmp_dir.join(unique_template_tmp_name());
    common::copy_directory_recursive(source_root, &scratch_root)?;
    Ok(scratch_root)
}

fn template_path_is_omitted(relative_path: &Path, omit_paths: &[String]) -> bool {
    let relative = relative_path.to_string_lossy().replace('\\', "/");
    omit_paths.iter().any(|pattern| {
        if let Some(prefix) = pattern.strip_suffix("/*") {
            relative == prefix || relative.starts_with(&format!("{prefix}/"))
        } else {
            relative == *pattern
        }
    })
}

fn unique_template_tmp_name() -> String {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time went backwards")
        .as_nanos();
    let unique_id = NEXT_TEMPLATE_TMP_ID.fetch_add(1, Ordering::Relaxed);
    format!(
        "devcontainer-template-{}-{suffix}-{unique_id}",
        std::process::id()
    )
}

fn substitute_template_options(contents: &str, template_options: &Map<String, Value>) -> String {
    let mut substituted = String::new();
    let mut remaining = contents;
    while let Some(start) = remaining.find("${templateOption:") {
        substituted.push_str(&remaining[..start]);
        let placeholder = &remaining[start + "${templateOption:".len()..];
        let Some(end) = placeholder.find('}') else {
            substituted.push_str(&remaining[start..]);
            return substituted;
        };
        let name = &placeholder[..end];
        if let Some(value) = template_options.get(name) {
            substituted.push_str(&template_option_string(value));
        } else {
            substituted.push_str(&remaining[start..start + "${templateOption:".len() + end + 1]);
        }
        remaining = &placeholder[end + 1..];
    }
    substituted.push_str(remaining);
    substituted
}

fn template_option_string(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}

fn merge_extra_features_into_template(
    workspace_root: &Path,
    extra_features: Value,
) -> Result<(), String> {
    let Some(extra_features) = extra_features
        .as_array()
        .filter(|features| !features.is_empty())
    else {
        return Ok(());
    };
    let config_path = applied_template_config_path(workspace_root)
        .ok_or_else(|| "Applied template is missing a dev container config".to_string())?;
    let raw = fs::read_to_string(&config_path).map_err(|error| error.to_string())?;
    let mut config = crate::config::parse_jsonc_value(&raw)?;
    let config_object = config
        .as_object_mut()
        .ok_or_else(|| "Applied template config must be a JSON object".to_string())?;
    let features = config_object
        .entry("features".to_string())
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| "Applied template features must be a JSON object".to_string())?;
    for feature in extra_features {
        let Some(id) = feature.get("id").and_then(Value::as_str) else {
            continue;
        };
        features.insert(
            id.to_string(),
            feature.get("options").cloned().unwrap_or_else(|| json!({})),
        );
    }
    fs::write(
        config_path,
        serde_json::to_string_pretty(&config).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn applied_template_config_path(workspace_root: &Path) -> Option<PathBuf> {
    [
        workspace_root
            .join(".devcontainer")
            .join("devcontainer.json"),
        workspace_root.join(".devcontainer.json"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::{
        applied_template_config_path, apply_generic_published_template,
        merge_extra_features_into_template, substitute_template_options, template_option_string,
        template_option_values, template_path_is_omitted,
    };

    #[test]
    fn template_option_values_merge_defaults_and_overrides() {
        let manifest = json!({
            "options": {
                "channel": { "type": "string", "default": "stable" },
                "enabled": { "type": "boolean", "default": true },
                "missingDefault": { "type": "string" }
            }
        });
        let options = template_option_values(
            &manifest,
            &json!({
                "channel": "nightly",
                "count": 3,
            }),
        );

        assert_eq!(options["channel"], "nightly");
        assert_eq!(options["enabled"], true);
        assert_eq!(options["count"], 3);
        assert!(!options.contains_key("missingDefault"));
    }

    #[test]
    fn substitute_template_options_preserves_unknown_and_unclosed_placeholders() {
        let options = template_option_values(
            &json!({
                "options": {
                    "channel": { "type": "string", "default": "stable" },
                    "enabled": { "type": "boolean", "default": true }
                }
            }),
            &json!({}),
        );

        assert_eq!(
            substitute_template_options(
                "image:${templateOption:channel} enabled=${templateOption:enabled}",
                &options,
            ),
            "image:stable enabled=true"
        );
        assert_eq!(
            substitute_template_options("${templateOption:missing}", &options),
            "${templateOption:missing}"
        );
        assert_eq!(
            substitute_template_options("before ${templateOption:channel", &options),
            "before ${templateOption:channel"
        );
        assert_eq!(template_option_string(&json!(["a", "b"])), "[\"a\",\"b\"]");
    }

    #[test]
    fn omit_path_patterns_match_exact_files_and_directory_prefixes() {
        assert!(template_path_is_omitted(
            std::path::Path::new(".github/workflows/ci.yml"),
            &[".github/*".to_string()]
        ));
        assert!(template_path_is_omitted(
            std::path::Path::new("README.md"),
            &["README.md".to_string()]
        ));
        assert!(!template_path_is_omitted(
            std::path::Path::new("docs/README.md"),
            &["README.md".to_string()]
        ));
    }

    #[test]
    fn generic_published_template_writes_config_and_merges_extra_features() {
        let workspace = crate::test_support::unique_temp_dir("devcontainer-template-test");
        let payload = apply_generic_published_template(
            &json!({
                "id": "custom-template",
                "name": "Custom Template"
            }),
            &workspace,
            json!([
                { "id": "ghcr.io/devcontainers/features/git:1", "options": { "ppa": true } },
                { "name": "missing-id" }
            ]),
        )
        .expect("apply generic template");

        assert_eq!(
            payload["files"],
            json!(["./.devcontainer/devcontainer.json"])
        );
        let config_path = workspace.join(".devcontainer").join("devcontainer.json");
        let config: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&config_path).expect("config"))
                .expect("config json");
        assert_eq!(config["name"], "Custom Template");
        assert_eq!(
            config["features"]["ghcr.io/devcontainers/features/git:1"]["ppa"],
            true
        );
        assert_eq!(
            applied_template_config_path(&workspace).as_deref(),
            Some(config_path.as_path())
        );

        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn merge_extra_features_reports_missing_or_invalid_template_configs() {
        let workspace = crate::test_support::unique_temp_dir("devcontainer-template-test");
        fs::create_dir_all(&workspace).expect("workspace");

        let error = merge_extra_features_into_template(
            &workspace,
            json!([{ "id": "ghcr.io/devcontainers/features/git:1" }]),
        )
        .expect_err("missing config");
        assert!(error.contains("missing a dev container config"), "{error}");

        fs::write(workspace.join(".devcontainer.json"), "[]").expect("config");
        let error = merge_extra_features_into_template(
            &workspace,
            json!([{ "id": "ghcr.io/devcontainers/features/git:1" }]),
        )
        .expect_err("invalid config");
        assert!(error.contains("must be a JSON object"), "{error}");

        fs::write(
            workspace.join(".devcontainer.json"),
            json!({"features": []}).to_string(),
        )
        .expect("config");
        let error = merge_extra_features_into_template(
            &workspace,
            json!([{ "id": "ghcr.io/devcontainers/features/git:1" }]),
        )
        .expect_err("invalid features");
        assert!(error.contains("features must be a JSON object"), "{error}");

        merge_extra_features_into_template(&workspace, json!([])).expect("empty extras");
        let _ = fs::remove_dir_all(workspace);
    }
}
