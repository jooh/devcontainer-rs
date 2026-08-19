//! Template collection discovery, rendering, and write helpers.

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::read::GzDecoder;
use serde_json::{json, Map, Value};
use tar::Archive;

use super::registry::{
    embedded_template_source_dir, local_oci_artifact, normalize_collection_reference,
    published_template_manifest_with_workspace,
};
use crate::commands::common;

const DEFAULT_PUBLISHED_TEMPLATE_BASE_IMAGE: &str = "docker.io/library/debian:bookworm-slim";
static NEXT_TEMPLATE_TMP_ID: AtomicU64 = AtomicU64::new(0);

fn io_error_to_string(error: io::Error) -> String {
    error.to_string()
}

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
        "id": manifest_value_or_string(&manifest, "id", "unknown"),
        "appliedTo": workspace_root,
    }))
}

pub(super) fn build_template_metadata_payload(
    template_path: &str,
    workspace_folder: Option<&Path>,
) -> Result<Value, String> {
    let manifest = if template_path.starts_with("ghcr.io/") {
        match published_template_manifest_with_workspace(template_path, workspace_folder) {
            Some(manifest) => manifest,
            None => return Err(format!("Unknown published template: {template_path}")),
        }
    } else {
        common::parse_manifest(Path::new(template_path), "devcontainer-template.json")?
    };
    Ok(json!({
        "id": manifest_value_or_string(&manifest, "id", "unknown"),
        "name": manifest_value_or_string(&manifest, "name", "unknown"),
        "description": manifest_value_or_string(&manifest, "description", ""),
    }))
}

pub(super) fn run_template_apply(args: &[String]) -> Result<Value, String> {
    let omit_paths = common::parse_json_string_array_option(args, "--omit-paths")?;
    let tmp_dir = common::parse_option_value(args, "--tmp-dir").map(PathBuf::from);
    let template_id = common::parse_option_value(args, "--template-id");
    if let Some(template_id) = template_id {
        let workspace = template_workspace_folder(args)?;
        return apply_catalog_template_with_options(
            &template_id,
            &workspace,
            args,
            &omit_paths,
            tmp_dir.as_deref(),
        );
    }

    let positionals = crate::cli::command_positionals("templates apply", args);
    let target = match positionals.first() {
        Some(target) => target,
        None => return Err("templates apply requires <target>".to_string()),
    };
    let workspace = template_workspace_folder(args)?;
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
    let manifest =
        match published_template_manifest_with_workspace(template_id, Some(workspace_root)) {
            Some(manifest) => manifest,
            None => return Err(format!("Unknown published template: {template_id}")),
        };
    let template_args = parse_json_option_or_default(args, "--template-args", json!({}))?;
    let extra_features = parse_json_option_or_default(args, "--features", json!([]))?;

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
            "installZsh": template_value_or_string(&template_args, "installZsh", "true"),
            "upgradePackages": template_value_or_string(&template_args, "upgradePackages", "false"),
        }),
    );
    features.insert(
        "ghcr.io/devcontainers/features/docker-from-docker:1".to_string(),
        json!({
            "version": template_value_or_string(&template_args, "dockerVersion", "latest"),
            "moby": template_value_or_string(&template_args, "moby", "true"),
            "enableNonRootDocker": template_value_or_string(&template_args, "enableNonRootDocker", "true"),
        }),
    );
    if let Some(extra_features) = extra_features.as_array() {
        for feature in extra_features {
            let Some(id) = feature.get("id").and_then(Value::as_str) else {
                continue;
            };
            features.insert(
                id.to_string(),
                template_value_or_empty_object(feature, "options"),
            );
        }
    }

    let devcontainer = json!({
        "name": manifest_value_or_string(&manifest, "name", "Docker from Docker"),
        "image": DEFAULT_PUBLISHED_TEMPLATE_BASE_IMAGE,
        "features": features,
    });
    let config_dir = workspace_root.join(".devcontainer");
    fs::create_dir_all(&config_dir).map_err(io_error_to_string)?;
    fs::write(
        config_dir.join("devcontainer.json"),
        serde_json::to_string_pretty(&devcontainer).expect("serializing JSON value cannot fail"),
    )
    .map_err(io_error_to_string)?;

    Ok(json!({
        "files": ["./.devcontainer/devcontainer.json"],
    }))
}

fn template_workspace_folder(args: &[String]) -> Result<PathBuf, String> {
    template_workspace_folder_with_current_dir(args, env::current_dir)
}

fn template_workspace_folder_with_current_dir(
    args: &[String],
    current_dir: impl FnOnce() -> io::Result<PathBuf>,
) -> Result<PathBuf, String> {
    if let Some(workspace) = common::parse_option_value(args, "--workspace-folder") {
        return Ok(PathBuf::from(workspace));
    }
    match current_dir() {
        Ok(current_dir) => Ok(current_dir),
        Err(_) => Err("Unable to determine workspace folder".to_string()),
    }
}

fn parse_json_option_or_default(
    args: &[String],
    name: &str,
    default: Value,
) -> Result<Value, String> {
    match common::parse_option_value(args, name) {
        Some(value) => crate::config::parse_jsonc_value(&value),
        None => Ok(default),
    }
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

    let extraction_parent = match tmp_dir {
        Some(tmp_dir) => tmp_dir.to_path_buf(),
        None => std::env::temp_dir(),
    };
    let extraction_root = extraction_parent.join(unique_template_tmp_name());
    fs::create_dir_all(&extraction_root).map_err(io_error_to_string)?;
    extract_template_layer(&layer_path, &extraction_root)?;

    let source_root = if extraction_root.join("src").is_dir() {
        extraction_root.join("src")
    } else {
        extraction_root
    };
    Ok(Some(source_root))
}

fn extract_template_layer(layer_path: &Path, extraction_root: &Path) -> Result<(), String> {
    let layer = fs::File::open(layer_path).map_err(io_error_to_string)?;
    let decoder = GzDecoder::new(layer);
    let mut archive = Archive::new(decoder);
    archive.unpack(extraction_root).map_err(io_error_to_string)
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
        "id": manifest_value_or_string(manifest, "id", "unknown"),
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
        manifest_value_or_string(manifest, "name", "Published Template"),
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
                template_value_or_empty_object(feature, "options"),
            );
        }
    }
    if !features.is_empty() {
        devcontainer.insert("features".to_string(), Value::Object(features));
    }

    let config_dir = workspace_root.join(".devcontainer");
    fs::create_dir_all(&config_dir).map_err(io_error_to_string)?;
    fs::write(
        config_dir.join("devcontainer.json"),
        serde_json::to_string_pretty(&Value::Object(devcontainer))
            .expect("serializing JSON value cannot fail"),
    )
    .map_err(io_error_to_string)?;

    Ok(json!({
        "files": ["./.devcontainer/devcontainer.json"],
    }))
}

fn template_option_values(manifest: &Value, template_args: &Value) -> Map<String, Value> {
    let mut options = Map::new();
    if let Some(entries) = manifest.get("options").and_then(Value::as_object) {
        for (name, definition) in entries {
            if let Some(value) = definition.get("default") {
                options.insert(name.clone(), value.clone());
            }
        }
    }
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
    fs::create_dir_all(workspace_root).map_err(io_error_to_string)?;
    for entry in fs::read_dir(template_root).map_err(io_error_to_string)? {
        let entry = entry.map_err(io_error_to_string)?;
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
        fs::create_dir_all(destination).map_err(io_error_to_string)?;
        for entry in fs::read_dir(source).map_err(io_error_to_string)? {
            let entry = entry.map_err(io_error_to_string)?;
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

    let bytes = fs::read(source).map_err(io_error_to_string)?;
    if let Ok(text) = String::from_utf8(bytes) {
        let substituted = substitute_template_options(&text, template_options);
        fs::write(destination, substituted).map_err(io_error_to_string)?;
    } else {
        fs::copy(source, destination).map_err(io_error_to_string)?;
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
    fs::create_dir_all(tmp_dir).map_err(io_error_to_string)?;
    let scratch_root = tmp_dir.join(unique_template_tmp_name());
    common::copy_directory_recursive(source_root, &scratch_root)?;
    Ok(scratch_root)
}

fn template_path_is_omitted(relative_path: &Path, omit_paths: &[String]) -> bool {
    let relative = relative_path.to_string_lossy().replace('\\', "/");
    for pattern in omit_paths {
        if let Some(prefix) = pattern.strip_suffix("/*") {
            if relative == prefix || relative.starts_with(&format!("{prefix}/")) {
                return true;
            }
        } else if relative == *pattern {
            return true;
        }
    }
    false
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
    match value.as_str() {
        Some(text) => text.to_string(),
        None => value.to_string(),
    }
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
    let config_path = match applied_template_config_path(workspace_root) {
        Some(config_path) => config_path,
        None => return Err("Applied template is missing a dev container config".to_string()),
    };
    let raw = fs::read_to_string(&config_path).map_err(io_error_to_string)?;
    let mut config = crate::config::parse_jsonc_value(&raw)?;
    let config_object = match config.as_object_mut() {
        Some(config_object) => config_object,
        None => return Err("Applied template config must be a JSON object".to_string()),
    };
    let features_value = config_object
        .entry("features".to_string())
        .or_insert(json!({}));
    let features = match features_value.as_object_mut() {
        Some(features) => features,
        None => return Err("Applied template features must be a JSON object".to_string()),
    };
    for feature in extra_features {
        let Some(id) = feature.get("id").and_then(Value::as_str) else {
            continue;
        };
        features.insert(
            id.to_string(),
            template_value_or_empty_object(feature, "options"),
        );
    }
    fs::write(
        config_path,
        serde_json::to_string_pretty(&config).expect("serializing JSON value cannot fail"),
    )
    .map_err(io_error_to_string)?;
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

fn manifest_value_or_string(manifest: &Value, key: &str, default: &str) -> Value {
    match manifest.get(key) {
        Some(value) => value.clone(),
        None => Value::String(default.to_string()),
    }
}

fn template_value_or_string(value: &Value, key: &str, default: &str) -> Value {
    match value.get(key) {
        Some(value) => value.clone(),
        None => Value::String(default.to_string()),
    }
}

fn template_value_or_empty_object(value: &Value, key: &str) -> Value {
    match value.get(key) {
        Some(value) => value.clone(),
        None => json!({}),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::{json, Map};

    use super::{
        applied_template_config_path, apply_catalog_template_with_options,
        apply_embedded_published_template, apply_generic_published_template,
        copy_embedded_template_contents, merge_extra_features_into_template, run_template_apply,
        substitute_template_options, template_option_string, template_option_values,
        template_path_is_omitted, template_workspace_folder,
        template_workspace_folder_with_current_dir,
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
    fn template_option_values_handles_missing_options_and_non_object_args() {
        let options = template_option_values(&json!({}), &json!("not an object"));

        assert!(options.is_empty());
    }

    #[test]
    fn template_workspace_folder_defaults_to_current_directory() {
        let current = std::env::current_dir().expect("current directory");
        let workspace = template_workspace_folder(&[]).expect("workspace folder");

        assert_eq!(workspace, current);
    }

    #[test]
    fn template_workspace_folder_reports_current_directory_errors() {
        let error = template_workspace_folder_with_current_dir(&[], || {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "current directory missing",
            ))
        })
        .expect_err("current dir error");

        assert_eq!(error, "Unable to determine workspace folder");
    }

    #[test]
    fn run_template_apply_copies_local_target_into_requested_workspace() {
        let template_root = crate::test_support::unique_temp_dir("devcontainer-template-source");
        let workspace = crate::test_support::unique_temp_dir("devcontainer-template-workspace");
        let source = template_root.join("src");
        fs::create_dir_all(&source).expect("template source");
        fs::write(
            template_root.join("devcontainer-template.json"),
            r#"{
  "id": "local-template",
  "name": "Local Template"
}"#,
        )
        .expect("manifest");
        fs::write(source.join("README.md"), "# local template\n").expect("readme");

        let payload = run_template_apply(&[
            template_root.display().to_string(),
            "--workspace-folder".to_string(),
            workspace.display().to_string(),
        ])
        .expect("apply template");

        assert_eq!(payload["id"], "local-template");
        assert_eq!(
            fs::read_to_string(workspace.join("README.md")).expect("readme"),
            "# local template\n"
        );
        let _ = fs::remove_dir_all(template_root);
        let _ = fs::remove_dir_all(workspace);
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
    fn generic_published_template_omits_features_for_non_array_extra_features() {
        let workspace = crate::test_support::unique_temp_dir("devcontainer-template-test");
        let payload = apply_generic_published_template(&json!({}), &workspace, json!({}))
            .expect("apply generic template");

        assert_eq!(
            payload["files"],
            json!(["./.devcontainer/devcontainer.json"])
        );
        let config: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(workspace.join(".devcontainer").join("devcontainer.json"))
                .expect("config"),
        )
        .expect("config json");
        assert_eq!(config["name"], "Published Template");
        assert!(config.get("features").is_none());
        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn docker_from_docker_catalog_template_merges_args_and_extra_features() {
        let workspace = crate::test_support::unique_temp_dir("devcontainer-template-test");
        let payload = apply_catalog_template_with_options(
            "ghcr.io/devcontainers/templates/docker-from-docker",
            &workspace,
            &[
                "--template-args".to_string(),
                r#"{"installZsh":"false","dockerVersion":"24.0","moby":"false"}"#.to_string(),
                "--features".to_string(),
                r#"[{"name":"missing-id"},{"id":"ghcr.io/devcontainers/features/git:1","options":{"ppa":true}}]"#
                    .to_string(),
            ],
            &[],
            None,
        )
        .expect("apply docker-from-docker template");

        assert_eq!(
            payload["files"],
            json!(["./.devcontainer/devcontainer.json"])
        );
        let config: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(workspace.join(".devcontainer").join("devcontainer.json"))
                .expect("config"),
        )
        .expect("config json");
        assert_eq!(
            config["features"]["ghcr.io/devcontainers/features/common-utils:1"]["installZsh"],
            "false"
        );
        assert_eq!(
            config["features"]["ghcr.io/devcontainers/features/docker-from-docker:1"]["version"],
            "24.0"
        );
        assert_eq!(
            config["features"]["ghcr.io/devcontainers/features/docker-from-docker:1"]["moby"],
            "false"
        );
        assert_eq!(
            config["features"]["ghcr.io/devcontainers/features/git:1"]["ppa"],
            true
        );
        assert!(config["features"].get("missing-id").is_none());
        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn docker_from_docker_catalog_template_ignores_non_array_extra_features() {
        let workspace = crate::test_support::unique_temp_dir("devcontainer-template-test");
        apply_catalog_template_with_options(
            "ghcr.io/devcontainers/templates/docker-from-docker",
            &workspace,
            &["--features".to_string(), "{}".to_string()],
            &[],
            None,
        )
        .expect("apply docker-from-docker template");

        let config: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(workspace.join(".devcontainer").join("devcontainer.json"))
                .expect("config"),
        )
        .expect("config json");
        assert_eq!(
            config["features"]
                .as_object()
                .expect("features object")
                .len(),
            2
        );
        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn embedded_template_copy_uses_tmp_dir_omit_patterns_and_binary_copy() {
        let source = crate::test_support::unique_temp_dir("devcontainer-template-source");
        let workspace = crate::test_support::unique_temp_dir("devcontainer-template-workspace");
        let tmp = crate::test_support::unique_temp_dir("devcontainer-template-tmp");
        fs::create_dir_all(source.join(".devcontainer")).expect("config dir");
        fs::create_dir_all(source.join("nested")).expect("nested dir");
        fs::write(source.join("devcontainer-template.json"), "{}").expect("template marker");
        fs::write(
            source.join(".devcontainer").join("devcontainer.json"),
            r#"{"features":{}}"#,
        )
        .expect("config");
        fs::write(
            source.join("nested").join("message.txt"),
            "channel=${templateOption:channel}",
        )
        .expect("message");
        fs::write(source.join("omit.txt"), "omit").expect("omit");
        fs::write(source.join("binary.bin"), [0xff, 0x00, 0x9f]).expect("binary");

        let payload = apply_embedded_published_template(
            &json!({
                "id": "embedded",
                "options": {
                    "channel": {
                        "type": "string",
                        "default": "stable"
                    }
                }
            }),
            &source,
            &workspace,
            &json!({
                "channel": "nightly"
            }),
            json!([
                {"name": "missing-id"},
                {"id": "ghcr.io/devcontainers/features/git:1", "options": {"ppa": true}}
            ]),
            &["omit.txt".to_string()],
            Some(&tmp),
        )
        .expect("apply embedded");

        assert_eq!(payload["id"], "embedded");
        assert_eq!(
            fs::read_to_string(workspace.join("nested").join("message.txt")).expect("message"),
            "channel=nightly"
        );
        assert_eq!(
            fs::read(workspace.join("binary.bin")).expect("binary"),
            vec![0xff, 0x00, 0x9f]
        );
        assert!(!workspace.join("omit.txt").exists());
        let config: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(workspace.join(".devcontainer").join("devcontainer.json"))
                .expect("config"),
        )
        .expect("config json");
        assert_eq!(
            config["features"]["ghcr.io/devcontainers/features/git:1"]["ppa"],
            true
        );
        assert!(config["features"].get("missing-id").is_none());
        assert!(fs::read_dir(&tmp).expect("tmp dir").next().is_some());
        let _ = fs::remove_dir_all(source);
        let _ = fs::remove_dir_all(workspace);
        let _ = fs::remove_dir_all(tmp);
    }

    #[cfg(unix)]
    #[test]
    fn embedded_template_copy_reports_nested_copy_errors() {
        let source = crate::test_support::unique_temp_dir("devcontainer-template-source");
        let workspace = crate::test_support::unique_temp_dir("devcontainer-template-workspace");
        fs::create_dir_all(source.join("nested")).expect("nested dir");
        std::os::unix::fs::symlink(
            source.join("missing-target"),
            source.join("nested").join("broken-link"),
        )
        .expect("broken symlink");

        let error =
            copy_embedded_template_contents(&source, &workspace, &Map::new(), &[]).unwrap_err();

        assert!(!error.is_empty());
        let _ = fs::remove_dir_all(source);
        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn run_template_apply_reports_missing_target_and_unknown_catalog_template() {
        let missing_target = run_template_apply(&[]).expect_err("missing target");
        assert_eq!(missing_target, "templates apply requires <target>");

        let workspace = crate::test_support::unique_temp_dir("devcontainer-template-test");
        let unknown = run_template_apply(&[
            "--template-id".to_string(),
            "ghcr.io/devcontainers/not-templates/unknown".to_string(),
            "--workspace-folder".to_string(),
            workspace.display().to_string(),
        ])
        .expect_err("unknown template");
        assert!(unknown.contains("Unknown published template"), "{unknown}");
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
