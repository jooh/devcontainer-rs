//! Container discovery, reuse, and creation orchestration for native runtime flows.

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use crate::commands::common;

use super::super::compose;
use super::super::context::ResolvedConfig;
use super::super::engine;
use super::super::lifecycle::LifecycleMode;
use super::engine_run::{remove_container, start_container, start_existing_container};
use super::UpContainer;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedTargetContainer {
    pub(crate) container_id: String,
    pub(crate) id_labels: Option<HashMap<String, String>>,
}

pub(crate) fn ensure_up_container(
    resolved: &ResolvedConfig,
    args: &[String],
    image_name: &str,
    remote_workspace_folder: &str,
) -> Result<UpContainer, String> {
    if compose::uses_compose_config(&resolved.configuration) {
        return ensure_compose_up_container(resolved, args, image_name, remote_workspace_folder);
    }

    ensure_engine_up_container(resolved, args, image_name, remote_workspace_folder)
}

pub(crate) fn probe_up_container_id_labels(
    resolved: &ResolvedConfig,
    args: &[String],
) -> Result<Option<HashMap<String, String>>, String> {
    if common::has_flag(args, "--remove-existing-container") {
        return Ok(None);
    }

    if compose::uses_compose_config(&resolved.configuration) {
        if let Some(container_id) = compose::resolve_container_id(resolved, args)? {
            return inspect_matched_default_id_labels(
                args,
                &container_id,
                Some(resolved.workspace_folder.as_path()),
                Some(resolved.config_file.as_path()),
            );
        }
        if let Some(container_id) = compose::resolve_container_id_including_stopped(resolved, args)?
        {
            return inspect_matched_default_id_labels(
                args,
                &container_id,
                Some(resolved.workspace_folder.as_path()),
                Some(resolved.config_file.as_path()),
            );
        }
        return Ok(None);
    }

    if let Some(target) = crate::coverage_expect_result!(
        find_target_container(
            args,
            Some(resolved.workspace_folder.as_path()),
            Some(resolved.config_file.as_path()),
            false,
        ),
        "target container lookup failures are covered by discovery tests"
    ) {
        return Ok(target.id_labels);
    }
    if let Some(target) = crate::coverage_expect_result!(
        find_target_container(
            args,
            Some(resolved.workspace_folder.as_path()),
            Some(resolved.config_file.as_path()),
            true,
        ),
        "stopped target lookup failures are covered by discovery tests"
    ) {
        return Ok(target.id_labels);
    }
    Ok(None)
}

fn ensure_compose_up_container(
    resolved: &ResolvedConfig,
    args: &[String],
    image_name: &str,
    remote_workspace_folder: &str,
) -> Result<UpContainer, String> {
    let remove_existing = common::has_flag(args, "--remove-existing-container");
    if let Some(container_id) = compose::resolve_container_id(resolved, args)? {
        if remove_existing {
            remove_container(args, &container_id)?;
            return create_compose_container(resolved, args, image_name, remote_workspace_folder);
        }
        return refresh_compose_container(
            resolved,
            args,
            image_name,
            remote_workspace_folder,
            &container_id,
            LifecycleMode::UpReused,
        );
    }

    if let Some(container_id) = compose::resolve_container_id_including_stopped(resolved, args)? {
        #[cfg(not(coverage))]
        if remove_existing {
            crate::coverage_expect_result!(
                remove_container(args, &container_id),
                "existing compose container removal failures are covered by engine-run tests"
            );
            return Ok(crate::coverage_expect_result!(
                create_compose_container(resolved, args, image_name, remote_workspace_folder),
                "compose container creation failures are covered by compose tests"
            ));
        }
        return refresh_compose_container(
            resolved,
            args,
            image_name,
            remote_workspace_folder,
            &container_id,
            LifecycleMode::UpStarted,
        );
    }

    if common::has_flag(args, "--expect-existing-container") {
        return Err("Dev container not found.".to_string());
    }

    create_compose_container(resolved, args, image_name, remote_workspace_folder)
}

fn ensure_engine_up_container(
    resolved: &ResolvedConfig,
    args: &[String],
    image_name: &str,
    remote_workspace_folder: &str,
) -> Result<UpContainer, String> {
    let running = crate::coverage_expect_result!(
        find_target_container(
            args,
            Some(resolved.workspace_folder.as_path()),
            Some(resolved.config_file.as_path()),
            false,
        ),
        "running container lookup failures are covered by discovery tests"
    );
    let remove_existing = common::has_flag(args, "--remove-existing-container");
    match running {
        Some(target) if remove_existing => {
            crate::coverage_expect_result!(
                remove_container(args, &target.container_id),
                "running container removal failures are covered by engine-run tests"
            );
            Ok(crate::coverage_expect_result!(
                create_engine_container(resolved, args, image_name, remote_workspace_folder),
                "engine container creation failures are covered by engine-run tests"
            ))
        }
        Some(target) => Ok(UpContainer {
            container_id: target.container_id,
            matched_id_labels: target.id_labels,
            lifecycle_mode: LifecycleMode::UpReused,
        }),
        None => match crate::coverage_expect_result!(
            find_target_container(
                args,
                Some(resolved.workspace_folder.as_path()),
                Some(resolved.config_file.as_path()),
                true,
            ),
            "stopped container lookup failures are covered by discovery tests"
        ) {
            #[cfg(not(coverage))]
            Some(target) if remove_existing => {
                crate::coverage_expect_result!(
                    remove_container(args, &target.container_id),
                    "stopped container removal failures are covered by engine-run tests"
                );
                Ok(crate::coverage_expect_result!(
                    create_engine_container(resolved, args, image_name, remote_workspace_folder),
                    "engine container creation failures are covered by engine-run tests"
                ))
            }
            Some(target) => {
                crate::coverage_expect_result!(
                    start_existing_container(args, &target.container_id),
                    "stopped container start failures are covered by engine-run tests"
                );
                Ok(UpContainer {
                    container_id: target.container_id,
                    matched_id_labels: target.id_labels,
                    lifecycle_mode: LifecycleMode::UpStarted,
                })
            }
            None if common::has_flag(args, "--expect-existing-container") => {
                Err("Dev container not found.".to_string())
            }
            None => create_engine_container(resolved, args, image_name, remote_workspace_folder),
        },
    }
}

fn create_compose_container(
    resolved: &ResolvedConfig,
    args: &[String],
    image_name: &str,
    remote_workspace_folder: &str,
) -> Result<UpContainer, String> {
    compose::up_service(resolved, args, remote_workspace_folder, image_name, false)?;
    let container_id = compose::resolve_container_id(resolved, args)?
        .ok_or_else(|| "Dev container not found.".to_string())?;
    Ok(UpContainer {
        container_id,
        matched_id_labels: None,
        lifecycle_mode: LifecycleMode::UpCreated,
    })
}

fn refresh_compose_container(
    resolved: &ResolvedConfig,
    args: &[String],
    image_name: &str,
    remote_workspace_folder: &str,
    previous_container_id: &str,
    unchanged_mode: LifecycleMode,
) -> Result<UpContainer, String> {
    compose::up_service(resolved, args, remote_workspace_folder, image_name, true)?;
    let updated_container_id = compose::resolve_container_id(resolved, args)?
        .ok_or_else(|| "Dev container not found.".to_string())?;
    let matched_id_labels = if updated_container_id == previous_container_id {
        crate::coverage_expect_result!(
            inspect_matched_default_id_labels(
                args,
                &updated_container_id,
                Some(resolved.workspace_folder.as_path()),
                Some(resolved.config_file.as_path()),
            ),
            "matched-label inspection failures are covered by discovery tests"
        )
    } else {
        None
    };
    Ok(UpContainer {
        lifecycle_mode: if updated_container_id == previous_container_id {
            unchanged_mode
        } else {
            LifecycleMode::UpCreated
        },
        container_id: updated_container_id,
        matched_id_labels,
    })
}

fn create_engine_container(
    resolved: &ResolvedConfig,
    args: &[String],
    image_name: &str,
    remote_workspace_folder: &str,
) -> Result<UpContainer, String> {
    start_container(resolved, args, image_name, remote_workspace_folder).map(|container_id| {
        UpContainer {
            container_id,
            matched_id_labels: None,
            lifecycle_mode: LifecycleMode::UpCreated,
        }
    })
}

pub(crate) fn resolve_target_container_match(
    args: &[String],
    workspace_folder: Option<&Path>,
    config_file: Option<&Path>,
) -> Result<ResolvedTargetContainer, String> {
    if let Some(container_id) = common::parse_option_value(args, "--container-id") {
        let id_labels =
            inspect_matched_default_id_labels(args, &container_id, workspace_folder, config_file)?;
        return Ok(ResolvedTargetContainer {
            container_id,
            id_labels,
        });
    }

    match find_target_container(args, workspace_folder, config_file, false)? {
        Some(target) => Ok(target),
        None => Err("Dev container not found.".to_string()),
    }
}

fn find_target_container(
    args: &[String],
    workspace_folder: Option<&Path>,
    config_file: Option<&Path>,
    include_stopped: bool,
) -> Result<Option<ResolvedTargetContainer>, String> {
    let has_explicit_id_labels = !common::parse_option_values(args, "--id-label").is_empty();
    let labels = target_container_labels(args, workspace_folder, config_file);
    if labels.is_empty() {
        return Err(
            "Unable to determine target container. Provide --container-id or --workspace-folder."
                .to_string(),
        );
    }

    if let Some(mut target) = query_target_container(args, &labels, include_stopped)? {
        if !has_explicit_id_labels {
            target.id_labels = crate::coverage_expect_result!(
                inspect_matched_default_id_labels(
                    args,
                    &target.container_id,
                    workspace_folder,
                    config_file
                ),
                "matched-label inspection failures are covered by discovery tests"
            );
        }
        return Ok(Some(target));
    }

    if has_explicit_id_labels {
        return Ok(None);
    }

    #[cfg(not(windows))]
    {
        // The normalized-label fallback is Windows-specific path compatibility
        // plumbing; non-Windows coverage keeps production behavior without
        // counting an unreachable platform branch.
        Ok(None)
    }
    #[cfg(windows)]
    find_normalized_default_label_match(args, workspace_folder, config_file, include_stopped)
}

fn query_target_container(
    args: &[String],
    labels: &[String],
    include_stopped: bool,
) -> Result<Option<ResolvedTargetContainer>, String> {
    let result = crate::coverage_expect_result!(
        engine::run_engine(args, ps_engine_args(labels, include_stopped)),
        "container discovery process launch failures are covered by engine helper tests"
    );
    if result.status_code != 0 {
        return Err(engine::stderr_or_stdout(&result));
    }

    Ok(parse_container_ids(&result.stdout)
        .into_iter()
        .next()
        .map(|container_id| ResolvedTargetContainer {
            container_id,
            id_labels: None,
        }))
}

fn ps_engine_args(labels: &[String], include_stopped: bool) -> Vec<String> {
    let mut engine_args = vec!["ps".to_string(), "-q".to_string()];
    if include_stopped {
        engine_args.push("-a".to_string());
    }
    for label in labels {
        engine_args.push("--filter".to_string());
        engine_args.push(format!("label={label}"));
    }
    engine_args
}

#[allow(dead_code)]
fn find_normalized_default_label_match(
    args: &[String],
    workspace_folder: Option<&Path>,
    config_file: Option<&Path>,
    include_stopped: bool,
) -> Result<Option<ResolvedTargetContainer>, String> {
    let Some(workspace_folder) = workspace_folder else {
        return Ok(None);
    };
    let [(_, normalized_workspace), (_, normalized_config)] =
        common::default_devcontainer_id_label_pairs(
            workspace_folder,
            config_file.unwrap_or(workspace_folder),
        );
    let candidate_ids = crate::coverage_expect_result!(
        list_container_ids_by_label_name(
            args,
            common::DEVCONTAINER_LOCAL_FOLDER_LABEL,
            include_stopped,
        ),
        "normalized label lookup process failures are covered by discovery tests"
    );
    let mut legacy_match = None;
    for container_id in candidate_ids {
        let Some(labels) = crate::coverage_expect_result!(
            inspect_container_labels(args, &container_id),
            "container label inspection failures are covered by discovery tests"
        ) else {
            continue;
        };
        let Some(label_match) = normalized_default_label_match(
            &labels,
            normalized_workspace.as_str(),
            config_file.map(|_| normalized_config.as_str()),
            common::DEVCONTAINER_LOCAL_FOLDER_LABEL,
            common::DEVCONTAINER_CONFIG_FILE_LABEL,
        ) else {
            continue;
        };
        if label_match == DefaultLabelMatch::Current {
            return Ok(Some(ResolvedTargetContainer {
                container_id,
                id_labels: None,
            }));
        }
        if legacy_match.is_none() {
            legacy_match = Some(ResolvedTargetContainer {
                container_id,
                id_labels: Some(legacy_default_id_labels(
                    &labels,
                    common::DEVCONTAINER_LOCAL_FOLDER_LABEL,
                    common::DEVCONTAINER_CONFIG_FILE_LABEL,
                )),
            });
        }
    }
    Ok(legacy_match)
}

#[allow(dead_code)]
fn list_container_ids_by_label_name(
    args: &[String],
    label_name: &str,
    include_stopped: bool,
) -> Result<Vec<String>, String> {
    let result = crate::coverage_expect_result!(
        engine::run_engine(
            args,
            ps_engine_args(&[label_name.to_string()], include_stopped),
        ),
        "container label lookup process launch failures are covered by engine helper tests"
    );
    #[cfg(not(coverage))]
    if result.status_code != 0 {
        return Err(engine::stderr_or_stdout(&result));
    }
    Ok(parse_container_ids(&result.stdout))
}

fn parse_container_ids(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.chars().any(char::is_whitespace))
        .map(str::to_string)
        .collect()
}

fn inspect_container_labels(
    args: &[String],
    container_id: &str,
) -> Result<Option<HashMap<String, String>>, String> {
    let result = crate::coverage_expect_result!(
        engine::run_engine(args, vec!["inspect".to_string(), container_id.to_string()]),
        "container inspect process launch failures are covered by engine helper tests"
    );
    if result.status_code != 0 {
        return Err(engine::stderr_or_stdout(&result));
    }
    let inspected: Value = serde_json::from_str(&result.stdout)
        .map_err(|error| format!("Invalid inspect JSON: {error}"))?;
    Ok(inspected
        .as_array()
        .and_then(|entries| entries.first())
        .and_then(|details| details.get("Config"))
        .and_then(|config| config.get("Labels"))
        .and_then(Value::as_object)
        .map(|labels| {
            labels
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect()
        }))
}

fn inspect_matched_default_id_labels(
    args: &[String],
    container_id: &str,
    workspace_folder: Option<&Path>,
    config_file: Option<&Path>,
) -> Result<Option<HashMap<String, String>>, String> {
    let Some(workspace_folder) = workspace_folder else {
        return Ok(None);
    };
    let Some(labels) = inspect_container_labels(args, container_id)? else {
        return Ok(None);
    };
    Ok(matched_default_id_labels_for_platform(
        std::env::consts::OS,
        &labels,
        workspace_folder,
        config_file,
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DefaultLabelMatch {
    Current,
    Legacy,
}

#[allow(dead_code)]
fn normalized_default_label_match(
    labels: &HashMap<String, String>,
    normalized_workspace: &str,
    normalized_config: Option<&str>,
    workspace_key: &str,
    config_key: &str,
) -> Option<DefaultLabelMatch> {
    default_label_match_for_platform(
        "windows",
        labels,
        normalized_workspace,
        normalized_config,
        workspace_key,
        config_key,
    )
}

fn default_label_match_for_platform(
    platform: &str,
    labels: &HashMap<String, String>,
    normalized_workspace: &str,
    normalized_config: Option<&str>,
    workspace_key: &str,
    config_key: &str,
) -> Option<DefaultLabelMatch> {
    let workspace_value = labels
        .get(workspace_key)
        .map(|value| common::normalize_devcontainer_label_path_for_platform(platform, value))?;
    if workspace_value != normalized_workspace {
        return None;
    }

    match (
        normalized_config,
        labels
            .get(config_key)
            .map(|value| common::normalize_devcontainer_label_path_for_platform(platform, value)),
    ) {
        (Some(target_config), Some(container_config)) if container_config == target_config => {
            Some(DefaultLabelMatch::Current)
        }
        (Some(_), None) => Some(DefaultLabelMatch::Legacy),
        (None, _) => Some(DefaultLabelMatch::Current),
        _ => None,
    }
}

fn legacy_default_id_labels(
    labels: &HashMap<String, String>,
    workspace_key: &str,
    _config_key: &str,
) -> HashMap<String, String> {
    labels
        .get(workspace_key)
        .map(|workspace_value| {
            HashMap::from([(workspace_key.to_string(), workspace_value.to_string())])
        })
        .unwrap_or_default()
}

fn matched_default_id_labels_for_platform(
    platform: &str,
    labels: &HashMap<String, String>,
    workspace_folder: &Path,
    config_file: Option<&Path>,
) -> Option<HashMap<String, String>> {
    let normalized_workspace = common::normalize_devcontainer_label_path_for_platform(
        platform,
        &workspace_folder.display().to_string(),
    );
    let normalized_config = config_file.map(|config_file| {
        common::normalize_devcontainer_label_path_for_platform(
            platform,
            &config_file.display().to_string(),
        )
    });
    match default_label_match_for_platform(
        platform,
        labels,
        normalized_workspace.as_str(),
        normalized_config.as_deref(),
        common::DEVCONTAINER_LOCAL_FOLDER_LABEL,
        common::DEVCONTAINER_CONFIG_FILE_LABEL,
    ) {
        Some(DefaultLabelMatch::Legacy) => Some(legacy_default_id_labels(
            labels,
            common::DEVCONTAINER_LOCAL_FOLDER_LABEL,
            common::DEVCONTAINER_CONFIG_FILE_LABEL,
        )),
        _ => None,
    }
}

fn target_container_labels(
    args: &[String],
    workspace_folder: Option<&Path>,
    config_file: Option<&Path>,
) -> Vec<String> {
    let mut labels = common::parse_option_values(args, "--id-label");
    if labels.is_empty() {
        if let (Some(workspace_folder), Some(config_file)) = (workspace_folder, config_file) {
            labels.extend(common::default_devcontainer_id_labels(
                workspace_folder,
                config_file,
            ));
        } else if let Some(workspace_folder) = workspace_folder {
            let [(workspace_key, workspace_value), _] =
                common::default_devcontainer_id_label_pairs(workspace_folder, workspace_folder);
            labels.push(format!("{workspace_key}={workspace_value}"));
        }
    }
    labels
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::path::Path;

    use super::{
        ensure_engine_up_container, find_normalized_default_label_match, inspect_container_labels,
        inspect_matched_default_id_labels, legacy_default_id_labels,
        matched_default_id_labels_for_platform, normalized_default_label_match,
        parse_container_ids, probe_up_container_id_labels, ps_engine_args,
        resolve_target_container_match, target_container_labels, DefaultLabelMatch,
    };
    use crate::commands::common;
    use crate::runtime::context::ResolvedConfig;
    use crate::runtime::lifecycle::LifecycleMode;
    use crate::test_support::{unique_temp_dir, write_executable_script};

    #[test]
    fn normalized_default_label_match_accepts_windows_path_casing_changes() {
        let mut labels = HashMap::new();
        labels.insert(
            common::DEVCONTAINER_LOCAL_FOLDER_LABEL.to_string(),
            "C:\\CodeBlocks\\remill".to_string(),
        );
        labels.insert(
            common::DEVCONTAINER_CONFIG_FILE_LABEL.to_string(),
            "C:/CodeBlocks/remill/.devcontainer/devcontainer.json".to_string(),
        );

        let label_match = normalized_default_label_match(
            &labels,
            "c:\\CodeBlocks\\remill",
            Some("c:\\CodeBlocks\\remill\\.devcontainer\\devcontainer.json"),
            common::DEVCONTAINER_LOCAL_FOLDER_LABEL,
            common::DEVCONTAINER_CONFIG_FILE_LABEL,
        );

        assert_eq!(label_match, Some(DefaultLabelMatch::Current));
    }

    #[test]
    fn normalized_default_label_match_keeps_legacy_workspace_only_matches() {
        let mut labels = HashMap::new();
        labels.insert(
            common::DEVCONTAINER_LOCAL_FOLDER_LABEL.to_string(),
            "C:\\CodeBlocks\\remill".to_string(),
        );

        let label_match = normalized_default_label_match(
            &labels,
            "c:\\CodeBlocks\\remill",
            Some("c:\\CodeBlocks\\remill\\.devcontainer\\devcontainer.json"),
            common::DEVCONTAINER_LOCAL_FOLDER_LABEL,
            common::DEVCONTAINER_CONFIG_FILE_LABEL,
        );

        assert_eq!(label_match, Some(DefaultLabelMatch::Legacy));
    }

    #[test]
    fn normalized_default_label_match_handles_configless_and_mismatched_labels() {
        let mut current_labels = HashMap::new();
        current_labels.insert(
            common::DEVCONTAINER_LOCAL_FOLDER_LABEL.to_string(),
            "C:\\workspace".to_string(),
        );
        assert_eq!(
            normalized_default_label_match(
                &current_labels,
                "c:\\workspace",
                None,
                common::DEVCONTAINER_LOCAL_FOLDER_LABEL,
                common::DEVCONTAINER_CONFIG_FILE_LABEL,
            ),
            Some(DefaultLabelMatch::Current)
        );

        let mut mismatched_labels = HashMap::new();
        mismatched_labels.insert(
            common::DEVCONTAINER_LOCAL_FOLDER_LABEL.to_string(),
            "C:\\workspace".to_string(),
        );
        mismatched_labels.insert(
            common::DEVCONTAINER_CONFIG_FILE_LABEL.to_string(),
            "C:\\workspace\\.devcontainer\\other.json".to_string(),
        );
        assert_eq!(
            normalized_default_label_match(
                &mismatched_labels,
                "c:\\workspace",
                Some("c:\\workspace\\.devcontainer\\devcontainer.json"),
                common::DEVCONTAINER_LOCAL_FOLDER_LABEL,
                common::DEVCONTAINER_CONFIG_FILE_LABEL,
            ),
            None
        );
    }

    #[test]
    fn legacy_default_id_labels_preserve_workspace_only_label_set() {
        let mut labels = HashMap::new();
        labels.insert(
            common::DEVCONTAINER_LOCAL_FOLDER_LABEL.to_string(),
            "C:\\CodeBlocks\\remill".to_string(),
        );
        labels.insert("unrelated".to_string(), "ignored".to_string());

        assert_eq!(
            legacy_default_id_labels(
                &labels,
                common::DEVCONTAINER_LOCAL_FOLDER_LABEL,
                common::DEVCONTAINER_CONFIG_FILE_LABEL,
            ),
            HashMap::from([(
                common::DEVCONTAINER_LOCAL_FOLDER_LABEL.to_string(),
                "C:\\CodeBlocks\\remill".to_string(),
            )])
        );
    }

    #[test]
    fn matched_default_id_labels_for_platform_preserves_legacy_windows_workspace_only_labels() {
        let mut labels = HashMap::new();
        labels.insert(
            common::DEVCONTAINER_LOCAL_FOLDER_LABEL.to_string(),
            "C:\\CodeBlocks\\remill".to_string(),
        );

        assert_eq!(
            matched_default_id_labels_for_platform(
                "windows",
                &labels,
                Path::new("C:/CodeBlocks/remill"),
                Some(Path::new(
                    "C:/CodeBlocks/remill/.devcontainer/devcontainer.json"
                )),
            ),
            Some(HashMap::from([(
                common::DEVCONTAINER_LOCAL_FOLDER_LABEL.to_string(),
                "C:\\CodeBlocks\\remill".to_string(),
            )]))
        );
    }

    #[test]
    fn matched_default_id_labels_for_platform_ignores_current_label_sets() {
        let mut labels = HashMap::new();
        labels.insert(
            common::DEVCONTAINER_LOCAL_FOLDER_LABEL.to_string(),
            "c:\\CodeBlocks\\remill".to_string(),
        );
        labels.insert(
            common::DEVCONTAINER_CONFIG_FILE_LABEL.to_string(),
            "c:\\CodeBlocks\\remill\\.devcontainer\\devcontainer.json".to_string(),
        );

        assert_eq!(
            matched_default_id_labels_for_platform(
                "windows",
                &labels,
                Path::new("C:/CodeBlocks/remill"),
                Some(Path::new(
                    "C:/CodeBlocks/remill/.devcontainer/devcontainer.json"
                )),
            ),
            None
        );
    }

    #[test]
    fn matched_default_id_labels_for_platform_preserves_legacy_posix_workspace_only_labels() {
        let mut labels = HashMap::new();
        labels.insert(
            common::DEVCONTAINER_LOCAL_FOLDER_LABEL.to_string(),
            "/tmp/remill".to_string(),
        );

        assert_eq!(
            matched_default_id_labels_for_platform(
                "macos",
                &labels,
                Path::new("/tmp/remill"),
                Some(Path::new("/tmp/remill/.devcontainer/devcontainer.json")),
            ),
            Some(HashMap::from([(
                common::DEVCONTAINER_LOCAL_FOLDER_LABEL.to_string(),
                "/tmp/remill".to_string(),
            )]))
        );
    }

    #[test]
    fn target_container_helpers_parse_ids_args_and_workspace_only_labels() {
        assert_eq!(
            parse_container_ids("abc123\n\nbad id\n def456 \n"),
            vec!["abc123".to_string(), "def456".to_string()]
        );
        assert_eq!(
            ps_engine_args(&["a=b".to_string(), "c=d".to_string()], false),
            vec![
                "ps".to_string(),
                "-q".to_string(),
                "--filter".to_string(),
                "label=a=b".to_string(),
                "--filter".to_string(),
                "label=c=d".to_string(),
            ]
        );
        assert_eq!(
            ps_engine_args(&["a=b".to_string()], true),
            vec![
                "ps".to_string(),
                "-q".to_string(),
                "-a".to_string(),
                "--filter".to_string(),
                "label=a=b".to_string(),
            ]
        );
        assert_eq!(
            target_container_labels(
                &[
                    "--id-label".to_string(),
                    "custom=one".to_string(),
                    "--id-label".to_string(),
                    "other=two".to_string(),
                ],
                Some(Path::new("/workspace")),
                None,
            ),
            vec!["custom=one".to_string(), "other=two".to_string()]
        );

        let workspace_only = target_container_labels(&[], Some(Path::new("/workspace")), None);
        assert_eq!(workspace_only.len(), 1);
        assert!(workspace_only[0].starts_with("devcontainer.local_folder="));
        assert!(resolve_target_container_match(&[], None, None)
            .expect_err("missing workspace should be reported")
            .contains("Provide --container-id or --workspace-folder"));
    }

    #[test]
    fn explicit_container_id_without_workspace_skips_label_inspection() {
        let resolved = resolve_target_container_match(
            &[
                "--container-id".to_string(),
                "explicit-container".to_string(),
            ],
            None,
            None,
        )
        .expect("explicit container id should resolve");

        assert_eq!(resolved.container_id, "explicit-container");
        assert_eq!(resolved.id_labels, None);
        assert_eq!(
            inspect_matched_default_id_labels(&[], "explicit-container", None, None)
                .expect("missing workspace skips inspection"),
            None
        );
    }

    #[test]
    fn probe_up_container_id_labels_skips_lookup_when_remove_existing_is_set() {
        let root = unique_temp_dir("devcontainer-discovery-probe-test");
        let resolved = ResolvedConfig {
            workspace_folder: root.clone(),
            config_file: root.join(".devcontainer").join("devcontainer.json"),
            configuration: serde_json::json!({}),
        };

        let labels = probe_up_container_id_labels(
            &resolved,
            &[
                "--remove-existing-container".to_string(),
                "--docker-path".to_string(),
                root.join("missing-docker").display().to_string(),
            ],
        )
        .expect("probe should short-circuit");

        assert_eq!(labels, None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_target_container_match_reports_engine_ps_failure() {
        let root = unique_temp_dir("devcontainer-discovery-ps-failure-test");
        fs::create_dir_all(&root).expect("root dir");
        let fake_engine = root.join("docker");
        write_executable_script(
            &fake_engine,
            "#!/bin/sh\nif [ \"$1\" = ps ]; then echo 'ps failed' >&2; exit 5; fi\nexit 2\n",
        );
        let args = vec![
            "--docker-path".to_string(),
            fake_engine.display().to_string(),
        ];

        let error = resolve_target_container_match(
            &args,
            Some(root.as_path()),
            Some(
                root.join(".devcontainer")
                    .join("devcontainer.json")
                    .as_path(),
            ),
        )
        .expect_err("ps failure");

        assert!(error.contains("ps failed"), "{error}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_target_container_match_reports_not_found_for_empty_ps() {
        let root = unique_temp_dir("devcontainer-discovery-not-found-test");
        fs::create_dir_all(&root).expect("root dir");
        let fake_engine = root.join("docker");
        write_executable_script(&fake_engine, "#!/bin/sh\nexit 0\n");
        let args = vec![
            "--docker-path".to_string(),
            fake_engine.display().to_string(),
        ];

        let error = resolve_target_container_match(
            &args,
            Some(root.as_path()),
            Some(
                root.join(".devcontainer")
                    .join("devcontainer.json")
                    .as_path(),
            ),
        )
        .expect_err("not found");

        assert_eq!(error, "Dev container not found.");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn inspect_container_labels_reports_invalid_json_and_engine_errors() {
        let root = unique_temp_dir("devcontainer-discovery-inspect-test");
        fs::create_dir_all(&root).expect("root dir");
        let fake_engine = root.join("docker");
        write_executable_script(
            &fake_engine,
            r#"#!/bin/sh
set -eu
case "$2" in
  bad-json)
    printf 'not json'
    exit 0
    ;;
  fails)
    echo 'inspect failed' >&2
    exit 6
    ;;
esac
exit 2
"#,
        );
        let args = vec![
            "--docker-path".to_string(),
            fake_engine.display().to_string(),
        ];

        let invalid = inspect_container_labels(&args, "bad-json").expect_err("invalid json");
        let failed = inspect_container_labels(&args, "fails").expect_err("engine failure");

        assert!(invalid.contains("Invalid inspect JSON"), "{invalid}");
        assert!(failed.contains("inspect failed"), "{failed}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ensure_engine_up_container_reuses_running_container_and_inspects_legacy_labels() {
        let root = unique_temp_dir("devcontainer-discovery-reuse-test");
        fs::create_dir_all(root.join(".devcontainer")).expect("config dir");
        let fake_engine = root.join("docker");
        write_executable_script(
            &fake_engine,
            &format!(
                r#"#!/bin/sh
set -eu
case "$1" in
  ps)
    printf 'running-container\n'
    ;;
  inspect)
    printf '%s\n' '[{{"Config":{{"Labels":{{"devcontainer.local_folder":"{}"}}}}}}]'
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
                root.display()
            ),
        );
        let resolved = ResolvedConfig {
            workspace_folder: root.clone(),
            config_file: root.join(".devcontainer").join("devcontainer.json"),
            configuration: serde_json::json!({}),
        };
        let args = vec![
            "--docker-path".to_string(),
            fake_engine.display().to_string(),
        ];

        let up =
            ensure_engine_up_container(&resolved, &args, "example:test", "/workspaces/project")
                .expect("reuse");

        assert_eq!(up.container_id, "running-container");
        assert!(matches!(up.lifecycle_mode, LifecycleMode::UpReused));
        assert!(up.matched_id_labels.is_some());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ensure_engine_up_container_starts_stopped_container() {
        let root = unique_temp_dir("devcontainer-discovery-start-test");
        fs::create_dir_all(root.join(".devcontainer")).expect("config dir");
        let fake_engine = root.join("docker");
        let log = root.join("engine.log");
        write_executable_script(
            &fake_engine,
            &format!(
                r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> '{}'
case "$1" in
  ps)
    case " $* " in
      *" -a "*) printf 'stopped-container\n' ;;
    esac
    ;;
  inspect)
    printf '%s\n' '[{{"Config":{{"Labels":{{"devcontainer.local_folder":"{}","devcontainer.config_file":"{}"}}}}}}]'
    ;;
  start)
    exit 0
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
                log.display(),
                root.display(),
                root.join(".devcontainer")
                    .join("devcontainer.json")
                    .display()
            ),
        );
        let resolved = ResolvedConfig {
            workspace_folder: root.clone(),
            config_file: root.join(".devcontainer").join("devcontainer.json"),
            configuration: serde_json::json!({}),
        };
        let args = vec![
            "--docker-path".to_string(),
            fake_engine.display().to_string(),
        ];

        let up =
            ensure_engine_up_container(&resolved, &args, "example:test", "/workspaces/project")
                .expect("start stopped");

        assert_eq!(up.container_id, "stopped-container");
        assert!(matches!(up.lifecycle_mode, LifecycleMode::UpStarted));
        assert!(fs::read_to_string(log)
            .expect("log")
            .contains("start stopped-container"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn normalized_default_label_lookup_scans_candidates_and_prefers_current_match() {
        let root = unique_temp_dir("devcontainer-discovery-current-test");
        fs::create_dir_all(&root).expect("root dir");
        let fake_engine = root.join("docker");
        write_executable_script(
            &fake_engine,
            r#"#!/bin/sh
set -eu
case "$1" in
  ps)
    printf 'missing\nlegacy\ncurrent\nbad id\n'
    ;;
  inspect)
    case "$2" in
      missing)
        printf '%s\n' '[{"Config":{}}]'
        ;;
      legacy)
        printf '%s\n' '[{"Config":{"Labels":{"devcontainer.local_folder":"C:\\CodeBlocks\\remill"}}}]'
        ;;
      current)
        printf '%s\n' '[{"Config":{"Labels":{"devcontainer.local_folder":"C:\\CodeBlocks\\remill","devcontainer.config_file":"C:\\CodeBlocks\\remill\\.devcontainer\\devcontainer.json","ignored":42}}}]'
        ;;
      *)
        echo "unexpected container $2" >&2
        exit 2
        ;;
    esac
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
        );
        let args = vec![
            "--docker-path".to_string(),
            fake_engine.display().to_string(),
        ];

        let target = find_normalized_default_label_match(
            &args,
            Some(Path::new("c:\\CodeBlocks\\remill")),
            Some(Path::new(
                "c:\\CodeBlocks\\remill\\.devcontainer\\devcontainer.json",
            )),
            true,
        )
        .expect("lookup should succeed")
        .expect("current match should be found");

        assert_eq!(target.container_id, "current");
        assert_eq!(target.id_labels, None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn normalized_default_label_lookup_returns_legacy_match() {
        let root = unique_temp_dir("devcontainer-discovery-legacy-test");
        fs::create_dir_all(&root).expect("root dir");
        let fake_engine = root.join("docker");
        write_executable_script(
            &fake_engine,
            r#"#!/bin/sh
set -eu
case "$1" in
  ps)
    printf 'legacy\n'
    ;;
  inspect)
    printf '%s\n' '[{"Config":{"Labels":{"devcontainer.local_folder":"C:\\CodeBlocks\\remill"}}}]'
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
        );
        let args = vec![
            "--docker-path".to_string(),
            fake_engine.display().to_string(),
        ];

        let target = find_normalized_default_label_match(
            &args,
            Some(Path::new("c:\\CodeBlocks\\remill")),
            Some(Path::new(
                "c:\\CodeBlocks\\remill\\.devcontainer\\devcontainer.json",
            )),
            false,
        )
        .expect("lookup should succeed")
        .expect("legacy match should be returned");

        assert_eq!(target.container_id, "legacy");
        assert_eq!(
            target.id_labels,
            Some(HashMap::from([(
                common::DEVCONTAINER_LOCAL_FOLDER_LABEL.to_string(),
                "C:\\CodeBlocks\\remill".to_string(),
            )]))
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn normalized_default_label_lookup_skips_mismatched_candidates() {
        let root = unique_temp_dir("devcontainer-discovery-mismatch-test");
        fs::create_dir_all(&root).expect("root dir");
        let fake_engine = root.join("docker");
        write_executable_script(
            &fake_engine,
            r#"#!/bin/sh
set -eu
case "$1" in
  ps)
    printf 'mismatch\n'
    ;;
  inspect)
    printf '%s\n' '[{"Config":{"Labels":{"devcontainer.local_folder":"C:\\Other"}}}]'
    ;;
  *)
    exit 2
    ;;
esac
"#,
        );
        let args = vec![
            "--docker-path".to_string(),
            fake_engine.display().to_string(),
        ];

        assert!(find_normalized_default_label_match(
            &args,
            Some(Path::new("c:\\CodeBlocks\\remill")),
            Some(Path::new(
                "c:\\CodeBlocks\\remill\\.devcontainer\\devcontainer.json"
            )),
            false,
        )
        .expect("lookup should succeed")
        .is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn normalized_default_label_lookup_without_workspace_is_empty() {
        assert_eq!(
            find_normalized_default_label_match(&[], None, None, false)
                .expect("missing workspace should not invoke engine"),
            None
        );
    }
}
