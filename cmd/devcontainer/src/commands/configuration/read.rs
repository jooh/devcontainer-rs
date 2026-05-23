//! Native read-configuration command assembly and output helpers.

use serde_json::{json, Map, Value};

use super::inspect::{merged_configuration_payload, read_configuration_value, workspace_payload};
use super::load::load_optional_config;
use crate::commands::common;

pub(super) fn build_read_configuration_payload(args: &[String]) -> Result<Value, String> {
    let include_merged = common::has_flag(args, "--include-merged-configuration");
    let include_features = common::has_flag(args, "--include-features-configuration");
    let loaded = load_optional_config(args)?;
    let inspected = if let Some(container_id) = common::parse_option_value(args, "--container-id") {
        Some(super::inspect::inspect_container(
            args,
            &container_id,
            loaded.as_ref(),
        )?)
    } else {
        None
    };
    let configuration = read_configuration_value(loaded.as_ref(), inspected.as_ref());
    let mut payload = Map::new();
    payload.insert("configuration".to_string(), configuration.clone());
    let resolved_features = if inspected.is_none() {
        loaded
            .as_ref()
            .map(|loaded| {
                super::resolve_feature_support_without_lockfile(
                    args,
                    &loaded.workspace_folder,
                    &loaded.config_file,
                    &loaded.configuration,
                )
            })
            .transpose()?
            .flatten()
    } else {
        None
    };

    if let Some(loaded) = loaded.as_ref() {
        payload.insert(
            "workspace".to_string(),
            workspace_payload(loaded, &configuration, args),
        );
    }

    if include_features || (include_merged && inspected.is_none()) {
        payload.insert(
            "featuresConfiguration".to_string(),
            match resolved_features.as_ref() {
                Some(resolved) => resolved.features_configuration.clone(),
                None => json!({ "featureSets": [] }),
            },
        );
    }

    if (include_features || include_merged)
        && resolved_features
            .as_ref()
            .is_some_and(|resolved| !resolved.feature_advisories.is_empty())
    {
        payload.insert(
            "featureAdvisories".to_string(),
            Value::Array(
                resolved_features
                    .as_ref()
                    .expect("advisories present when features are resolved")
                    .feature_advisories
                    .clone(),
            ),
        );
    }

    if include_merged {
        payload.insert(
            "mergedConfiguration".to_string(),
            merged_configuration_payload(
                &configuration,
                inspected.as_ref(),
                resolved_features
                    .as_ref()
                    .map(|resolved| resolved.metadata_entries.as_slice())
                    .unwrap_or(&[]),
            ),
        );
    }

    Ok(Value::Object(payload))
}

pub(super) fn should_use_native_read_configuration(args: &[String]) -> bool {
    const SUPPORTED_OPTIONS: [&str; 14] = [
        "--workspace-folder",
        "--config",
        "--override-config",
        "--container-id",
        "--id-label",
        "--docker-path",
        "--docker-compose-path",
        "--include-merged-configuration",
        "--include-features-configuration",
        "--additional-features",
        "--skip-feature-auto-mapping",
        "--mount-workspace-git-root",
        "--mount-git-worktree-common-dir",
        "--workspace-mount-consistency",
    ];
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if !arg.starts_with("--") {
            return false;
        }
        if !SUPPORTED_OPTIONS.contains(&arg.as_str()) {
            return false;
        }
        index += match arg.as_str() {
            "--include-merged-configuration" | "--include-features-configuration" => 1,
            "--skip-feature-auto-mapping"
            | "--mount-workspace-git-root"
            | "--mount-git-worktree-common-dir" => {
                if args
                    .get(index + 1)
                    .is_some_and(|next| !next.starts_with("--"))
                {
                    2
                } else {
                    1
                }
            }
            _ => 2,
        };
    }
    true
}
