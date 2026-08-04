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

    if let Some(target) = find_target_container(
        args,
        Some(resolved.workspace_folder.as_path()),
        Some(resolved.config_file.as_path()),
        false,
    )? {
        return Ok(target.id_labels);
    }
    if let Some(target) = find_target_container(
        args,
        Some(resolved.workspace_folder.as_path()),
        Some(resolved.config_file.as_path()),
        true,
    )? {
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
            LifecycleMode::UpStarted,
        );
    }

    if common::has_flag(args, "--expect-existing-container") {
        return Err(super::dev_container_not_found_message(
            args,
            Some(&resolved.workspace_folder),
            Some(&resolved.config_file),
        ));
    }

    create_compose_container(resolved, args, image_name, remote_workspace_folder)
}

fn ensure_engine_up_container(
    resolved: &ResolvedConfig,
    args: &[String],
    image_name: &str,
    remote_workspace_folder: &str,
) -> Result<UpContainer, String> {
    let running = find_target_container(
        args,
        Some(resolved.workspace_folder.as_path()),
        Some(resolved.config_file.as_path()),
        false,
    )?;
    let remove_existing = common::has_flag(args, "--remove-existing-container");
    match running {
        Some(target) if remove_existing => {
            remove_container(args, &target.container_id)?;
            create_engine_container(resolved, args, image_name, remote_workspace_folder)
        }
        Some(target) => Ok(UpContainer {
            container_id: target.container_id,
            matched_id_labels: target.id_labels,
            lifecycle_mode: LifecycleMode::UpReused,
        }),
        None => match find_target_container(
            args,
            Some(resolved.workspace_folder.as_path()),
            Some(resolved.config_file.as_path()),
            true,
        )? {
            Some(target) if remove_existing => {
                remove_container(args, &target.container_id)?;
                create_engine_container(resolved, args, image_name, remote_workspace_folder)
            }
            Some(target) => {
                start_existing_container(args, &target.container_id)?;
                Ok(UpContainer {
                    container_id: target.container_id,
                    matched_id_labels: target.id_labels,
                    lifecycle_mode: LifecycleMode::UpStarted,
                })
            }
            None if common::has_flag(args, "--expect-existing-container") => {
                Err(super::dev_container_not_found_message(
                    args,
                    Some(&resolved.workspace_folder),
                    Some(&resolved.config_file),
                ))
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
    let up_result =
        compose::up_service(resolved, args, remote_workspace_folder, image_name, false)?;
    let Some(container_id) = compose::resolve_container_id(resolved, args)? else {
        return Err(compose_startup_failure_error(resolved, args, &up_result)?);
    };
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
    let up_result = compose::up_service(resolved, args, remote_workspace_folder, image_name, true)?;
    let Some(updated_container_id) = compose::resolve_container_id(resolved, args)? else {
        return Err(compose_startup_failure_error(resolved, args, &up_result)?);
    };
    let matched_id_labels = if updated_container_id == previous_container_id {
        inspect_matched_default_id_labels(
            args,
            &updated_container_id,
            Some(resolved.workspace_folder.as_path()),
            Some(resolved.config_file.as_path()),
        )?
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

fn compose_startup_failure_error(
    resolved: &ResolvedConfig,
    args: &[String],
    up_result: &compose::ComposeUpResult,
) -> Result<String, String> {
    let stopped_container_id = compose::resolve_container_id_including_stopped(resolved, args)?;
    let mut message = if let Some(container_id) = stopped_container_id {
        format!(
            "Dev container service '{}' for compose project '{}' was created but is not running (container {}).",
            up_result.service, up_result.project_name, container_id
        )
    } else {
        format!(
            "Dev container service '{}' for compose project '{}' was not found after compose up.",
            up_result.service, up_result.project_name
        )
    };

    append_diagnostic_section(&mut message, "Compose up stdout", &up_result.stdout);
    append_diagnostic_section(&mut message, "Compose up stderr", &up_result.stderr);

    match compose::service_logs(resolved, args) {
        Ok(logs) => {
            append_diagnostic_section(&mut message, "Compose logs stdout", &logs.stdout);
            append_diagnostic_section(&mut message, "Compose logs stderr", &logs.stderr);
            if logs.status_code != 0
                && logs.stdout.trim().is_empty()
                && logs.stderr.trim().is_empty()
            {
                message.push_str(&format!(
                    "\nCompose logs exited with status {} without output.",
                    logs.status_code
                ));
            }
        }
        Err(error) => {
            append_diagnostic_section(&mut message, "Compose logs unavailable", &error);
        }
    }

    Ok(message)
}

fn append_diagnostic_section(message: &mut String, title: &str, body: &str) {
    let body = body.trim();
    if body.is_empty() {
        return;
    }
    message.push_str("\n\n");
    message.push_str(title);
    message.push_str(":\n");
    message.push_str(&truncate_diagnostic(body));
}

fn truncate_diagnostic(body: &str) -> String {
    const MAX_DIAGNOSTIC_CHARS: usize = 16 * 1024;
    if body.len() <= MAX_DIAGNOSTIC_CHARS {
        return body.to_string();
    }

    let mut end = MAX_DIAGNOSTIC_CHARS;
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n... truncated compose diagnostic output after {} bytes ...",
        &body[..end],
        MAX_DIAGNOSTIC_CHARS
    )
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
        None => Err(super::dev_container_not_found_message(
            args,
            workspace_folder,
            config_file,
        )),
    }
}

fn find_target_container(
    args: &[String],
    workspace_folder: Option<&Path>,
    config_file: Option<&Path>,
    include_stopped: bool,
) -> Result<Option<ResolvedTargetContainer>, String> {
    find_target_container_for_platform(
        std::env::consts::OS,
        args,
        workspace_folder,
        config_file,
        include_stopped,
    )
}

fn find_target_container_for_platform(
    platform: &str,
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
            target.id_labels = inspect_matched_default_id_labels(
                args,
                &target.container_id,
                workspace_folder,
                config_file,
            )?;
        }
        return Ok(Some(target));
    }

    if has_explicit_id_labels || platform != "windows" {
        return Ok(None);
    }

    find_normalized_default_label_match(args, workspace_folder, config_file, include_stopped)
}

fn query_target_container(
    args: &[String],
    labels: &[String],
    include_stopped: bool,
) -> Result<Option<ResolvedTargetContainer>, String> {
    let result = engine::run_engine(args, ps_engine_args(labels, include_stopped))?;
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
    let candidate_ids = list_container_ids_by_label_name(
        args,
        common::DEVCONTAINER_LOCAL_FOLDER_LABEL,
        include_stopped,
    )?;
    let mut legacy_match = None;
    for container_id in candidate_ids {
        let Some(labels) = inspect_container_labels(args, &container_id)? else {
            continue;
        };
        match normalized_default_label_match(
            &labels,
            normalized_workspace.as_str(),
            config_file.map(|_| normalized_config.as_str()),
            common::DEVCONTAINER_LOCAL_FOLDER_LABEL,
            common::DEVCONTAINER_CONFIG_FILE_LABEL,
        ) {
            Some(DefaultLabelMatch::Current) => {
                return Ok(Some(ResolvedTargetContainer {
                    container_id,
                    id_labels: None,
                }))
            }
            Some(DefaultLabelMatch::Legacy) if legacy_match.is_none() => {
                legacy_match = Some(ResolvedTargetContainer {
                    container_id,
                    id_labels: Some(legacy_default_id_labels(
                        &labels,
                        common::DEVCONTAINER_LOCAL_FOLDER_LABEL,
                        common::DEVCONTAINER_CONFIG_FILE_LABEL,
                    )),
                });
            }
            _ => {}
        }
    }
    Ok(legacy_match)
}

fn list_container_ids_by_label_name(
    args: &[String],
    label_name: &str,
    include_stopped: bool,
) -> Result<Vec<String>, String> {
    let result = engine::run_engine(
        args,
        ps_engine_args(&[label_name.to_string()], include_stopped),
    )?;
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
    let result = engine::run_engine(args, vec!["inspect".to_string(), container_id.to_string()])?;
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

    use serde_json::json;

    use super::{
        compose_startup_failure_error, default_label_match_for_platform, ensure_up_container,
        find_normalized_default_label_match, find_target_container_for_platform,
        inspect_matched_default_id_labels, legacy_default_id_labels,
        list_container_ids_by_label_name, matched_default_id_labels_for_platform,
        normalized_default_label_match, parse_container_ids, probe_up_container_id_labels,
        ps_engine_args, resolve_target_container_match, target_container_labels, DefaultLabelMatch,
    };
    use crate::commands::common;
    use crate::runtime::compose::ComposeUpResult;
    use crate::runtime::context::ResolvedConfig;
    use crate::runtime::lifecycle::LifecycleMode;
    use crate::test_support::{unique_temp_dir, write_executable_script};

    fn engine_args(fake_engine: &Path) -> Vec<String> {
        vec![
            "--docker-path".to_string(),
            fake_engine.display().to_string(),
        ]
    }

    fn resolved_config(
        workspace_folder: &Path,
        configuration: serde_json::Value,
    ) -> ResolvedConfig {
        ResolvedConfig {
            workspace_folder: workspace_folder.to_path_buf(),
            config_file: workspace_folder
                .join(".devcontainer")
                .join("devcontainer.json"),
            configuration,
        }
    }

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
    fn default_label_match_handles_workspace_and_config_mismatches() {
        let mut labels = HashMap::new();
        labels.insert(
            common::DEVCONTAINER_LOCAL_FOLDER_LABEL.to_string(),
            "C:\\CodeBlocks\\remill".to_string(),
        );
        labels.insert(
            common::DEVCONTAINER_CONFIG_FILE_LABEL.to_string(),
            "C:\\CodeBlocks\\remill\\.devcontainer\\other.json".to_string(),
        );

        assert_eq!(
            default_label_match_for_platform(
                "windows",
                &labels,
                "c:\\CodeBlocks\\other",
                Some("c:\\CodeBlocks\\remill\\.devcontainer\\devcontainer.json"),
                common::DEVCONTAINER_LOCAL_FOLDER_LABEL,
                common::DEVCONTAINER_CONFIG_FILE_LABEL,
            ),
            None
        );
        assert_eq!(
            default_label_match_for_platform(
                "windows",
                &labels,
                "c:\\CodeBlocks\\remill",
                None,
                common::DEVCONTAINER_LOCAL_FOLDER_LABEL,
                common::DEVCONTAINER_CONFIG_FILE_LABEL,
            ),
            Some(DefaultLabelMatch::Current)
        );
        assert_eq!(
            default_label_match_for_platform(
                "windows",
                &labels,
                "c:\\CodeBlocks\\remill",
                Some("c:\\CodeBlocks\\remill\\.devcontainer\\devcontainer.json"),
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
    fn resolve_target_container_match_reports_missing_label_match() {
        let root = unique_temp_dir("devcontainer-discovery-missing-target-test");
        fs::create_dir_all(&root).expect("root dir");
        let fake_engine = root.join("docker");
        write_executable_script(
            &fake_engine,
            r#"#!/bin/sh
set -eu
case "$1" in
  ps)
    exit 0
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
        );
        let args = engine_args(&fake_engine);

        let error = resolve_target_container_match(
            &args,
            Some(Path::new("/workspace")),
            Some(Path::new("/workspace/.devcontainer/devcontainer.json")),
        )
        .expect_err("missing container should fail");

        assert_eq!(
            error,
            "Dev container not found for workspace folder '/workspace' and config file '/workspace/.devcontainer/devcontainer.json'. If the container was created with a different config file, pass --config <path> or set DEVCONTAINER_CONFIG."
        );

        let workspace_only_error =
            resolve_target_container_match(&args, Some(Path::new("/workspace")), None)
                .expect_err("missing workspace-only container should fail");
        assert_eq!(workspace_only_error, "Dev container not found.");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn target_lookup_propagates_inspect_failures_for_default_label_matches() {
        let root = unique_temp_dir("devcontainer-discovery-inspect-failure-test");
        fs::create_dir_all(&root).expect("root dir");
        let fake_engine = root.join("docker");
        write_executable_script(
            &fake_engine,
            r#"#!/bin/sh
set -eu
case "$1" in
  ps)
    printf 'target-container\n'
    ;;
  inspect)
    echo "inspect failed" >&2
    exit 2
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
        );
        let args = engine_args(&fake_engine);

        let error = resolve_target_container_match(
            &args,
            Some(Path::new("/workspace")),
            Some(Path::new("/workspace/.devcontainer/devcontainer.json")),
        )
        .expect_err("inspect failure should propagate");

        assert_eq!(error, "inspect failed");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn inspect_matched_default_id_labels_returns_none_without_labels() {
        let root = unique_temp_dir("devcontainer-discovery-no-labels-test");
        fs::create_dir_all(&root).expect("root dir");
        let fake_engine = root.join("docker");
        write_executable_script(
            &fake_engine,
            r#"#!/bin/sh
set -eu
case "$1" in
  inspect)
    printf '%s\n' '[{"Config":{}}]'
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
        );
        let args = engine_args(&fake_engine);

        assert_eq!(
            inspect_matched_default_id_labels(
                &args,
                "target-container",
                Some(Path::new("/workspace")),
                Some(Path::new("/workspace/.devcontainer/devcontainer.json")),
            )
            .expect("missing labels should not fail"),
            None
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn list_container_ids_by_label_name_reports_engine_errors() {
        let missing_args = vec![
            "--docker-path".to_string(),
            "/path/that/does/not/exist".to_string(),
        ];
        assert!(find_normalized_default_label_match(
            &missing_args,
            Some(Path::new("c:\\CodeBlocks\\remill")),
            Some(Path::new(
                "c:\\CodeBlocks\\remill\\.devcontainer\\devcontainer.json"
            )),
            false,
        )
        .expect_err("missing engine should propagate")
        .contains("Container engine executable not found"));
        assert!(list_container_ids_by_label_name(
            &missing_args,
            common::DEVCONTAINER_LOCAL_FOLDER_LABEL,
            false,
        )
        .expect_err("missing engine should propagate")
        .contains("Container engine executable not found"));

        let root = unique_temp_dir("devcontainer-discovery-ps-status-test");
        fs::create_dir_all(&root).expect("root dir");
        let fake_engine = root.join("docker");
        write_executable_script(
            &fake_engine,
            r#"#!/bin/sh
set -eu
case "$1" in
  ps)
    echo "ps failed" >&2
    exit 2
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
        );

        assert_eq!(
            list_container_ids_by_label_name(
                &engine_args(&fake_engine),
                common::DEVCONTAINER_LOCAL_FOLDER_LABEL,
                false,
            )
            .expect_err("ps status failure should propagate"),
            "ps failed"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn probe_id_labels_propagates_stopped_lookup_errors() {
        let root = unique_temp_dir("devcontainer-discovery-probe-stopped-error-test");
        fs::create_dir_all(&root).expect("root dir");
        let fake_engine = root.join("docker");
        write_executable_script(
            &fake_engine,
            r#"#!/bin/sh
set -eu
case "$1" in
  ps)
    case " $* " in
      *" -a "*)
        echo "stopped lookup failed" >&2
        exit 2
        ;;
      *)
        exit 0
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
        let resolved = resolved_config(&root, json!({"image": "alpine:3.20"}));

        let error = probe_up_container_id_labels(&resolved, &engine_args(&fake_engine))
            .expect_err("stopped lookup failure should propagate");

        assert_eq!(error, "stopped lookup failed");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn probe_id_labels_handles_remove_flag_and_running_engine_match() {
        let root = unique_temp_dir("devcontainer-discovery-probe-running-test");
        fs::create_dir_all(&root).expect("root dir");
        let fake_engine = root.join("docker");
        write_executable_script(
            &fake_engine,
            r#"#!/bin/sh
set -eu
case "$1" in
  ps)
    printf 'running-container\n'
    ;;
  inspect)
    printf '%s\n' '[{"Config":{"Labels":{"devcontainer.local_folder":"/workspace","devcontainer.config_file":"/workspace/.devcontainer/devcontainer.json"}}}]'
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
        );
        let resolved = resolved_config(Path::new("/workspace"), json!({"image": "alpine:3.20"}));
        let mut remove_args = engine_args(&fake_engine);
        remove_args.push("--remove-existing-container".to_string());

        assert_eq!(
            probe_up_container_id_labels(&resolved, &remove_args)
                .expect("remove flag should skip probing"),
            None
        );
        assert_eq!(
            probe_up_container_id_labels(&resolved, &engine_args(&fake_engine))
                .expect("running target should be probed"),
            None
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn probe_id_labels_handles_compose_running_and_stopped_matches() {
        let root = unique_temp_dir("devcontainer-discovery-probe-compose-test");
        let config_root = root.join(".devcontainer");
        fs::create_dir_all(&config_root).expect("config dir");
        fs::write(
            config_root.join("docker-compose.yml"),
            "services:\n  app:\n    image: alpine:3.20\n",
        )
        .expect("compose file");
        let fake_engine = root.join("docker");
        write_executable_script(
            &fake_engine,
            &format!(
                r#"#!/bin/sh
set -eu
case "$1" in
  ps)
    case " $* " in
      *" -a "*)
        printf 'stopped-compose-container\n'
        ;;
      *)
        if [ -f "$(dirname "$0")/running" ]; then
          printf 'running-compose-container\n'
        fi
        ;;
    esac
    ;;
  inspect)
    case "$2" in
      running-compose-container)
        printf '%s\n' '[{{"Config":{{"Labels":{{"devcontainer.local_folder":"{workspace}","devcontainer.config_file":"{config}"}}}}}}]'
        ;;
      stopped-compose-container)
        printf '%s\n' '[{{"Config":{{"Labels":{{"devcontainer.local_folder":"{workspace}"}}}}}}]'
        ;;
      *)
        echo "unexpected inspect $2" >&2
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
                workspace = root.display(),
                config = root
                    .join(".devcontainer")
                    .join("devcontainer.json")
                    .display()
            ),
        );
        let resolved = resolved_config(
            &root,
            json!({
                "dockerComposeFile": "docker-compose.yml",
                "service": "app"
            }),
        );
        fs::write(root.join("running"), "").expect("running marker");
        assert_eq!(
            probe_up_container_id_labels(&resolved, &engine_args(&fake_engine))
                .expect("running compose target"),
            None
        );
        fs::remove_file(root.join("running")).expect("remove running marker");
        assert_eq!(
            probe_up_container_id_labels(&resolved, &engine_args(&fake_engine))
                .expect("stopped compose target"),
            Some(HashMap::from([(
                common::DEVCONTAINER_LOCAL_FOLDER_LABEL.to_string(),
                root.display().to_string(),
            )]))
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn probe_id_labels_returns_none_when_compose_has_no_matching_containers() {
        let root = unique_temp_dir("devcontainer-discovery-probe-compose-empty-test");
        let config_root = root.join(".devcontainer");
        fs::create_dir_all(&config_root).expect("config dir");
        fs::write(
            config_root.join("docker-compose.yml"),
            "services:\n  app:\n    image: alpine:3.20\n",
        )
        .expect("compose file");
        let fake_engine = root.join("docker");
        write_executable_script(
            &fake_engine,
            r#"#!/bin/sh
set -eu
case "$1" in
  ps)
    exit 0
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
        );
        let resolved = resolved_config(
            &root,
            json!({
                "dockerComposeFile": "docker-compose.yml",
                "service": "app"
            }),
        );

        assert_eq!(
            probe_up_container_id_labels(&resolved, &engine_args(&fake_engine))
                .expect("missing compose targets should not fail"),
            None
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn probe_id_labels_handles_stopped_engine_match_and_missing_engine_match() {
        let root = unique_temp_dir("devcontainer-discovery-probe-engine-stopped-test");
        fs::create_dir_all(&root).expect("root dir");
        let resolved = resolved_config(&root, json!({"image": "alpine:3.20"}));

        let stopped_engine = root.join("stopped-docker");
        write_executable_script(
            &stopped_engine,
            &format!(
                r#"#!/bin/sh
set -eu
case "$1" in
  ps)
    case " $* " in
      *" -a "*)
        printf 'stopped-container\n'
        ;;
      *)
        exit 0
        ;;
    esac
    ;;
  inspect)
    printf '%s\n' '[{{"Config":{{"Labels":{{"devcontainer.local_folder":"{workspace}"}}}}}}]'
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
                workspace = root.display()
            ),
        );
        assert_eq!(
            probe_up_container_id_labels(&resolved, &engine_args(&stopped_engine))
                .expect("stopped engine match should be probed"),
            Some(HashMap::from([(
                common::DEVCONTAINER_LOCAL_FOLDER_LABEL.to_string(),
                root.display().to_string(),
            )]))
        );

        let empty_engine = root.join("empty-docker");
        write_executable_script(
            &empty_engine,
            r#"#!/bin/sh
set -eu
case "$1" in
  ps)
    exit 0
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
        );
        assert_eq!(
            probe_up_container_id_labels(&resolved, &engine_args(&empty_engine))
                .expect("missing engine match should not fail"),
            None
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ensure_engine_container_reports_lookup_errors() {
        let root = unique_temp_dir("devcontainer-discovery-engine-lookup-errors-test");
        fs::create_dir_all(&root).expect("root dir");
        let running_error_engine = root.join("running-error-docker");
        write_executable_script(
            &running_error_engine,
            r#"#!/bin/sh
set -eu
case "$1" in
  ps)
    echo "running lookup failed" >&2
    exit 2
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
        );
        let resolved = resolved_config(&root, json!({"image": "alpine:3.20"}));

        let running_error = ensure_up_container(
            &resolved,
            &engine_args(&running_error_engine),
            "alpine:3.20",
            "/workspace",
        )
        .err()
        .expect("running lookup failure should propagate");
        assert_eq!(running_error, "running lookup failed");

        let stopped_error_engine = root.join("stopped-error-docker");
        write_executable_script(
            &stopped_error_engine,
            r#"#!/bin/sh
set -eu
case "$1" in
  ps)
    case " $* " in
      *" -a "*)
        echo "stopped lookup failed" >&2
        exit 2
        ;;
      *)
        exit 0
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

        let stopped_error = ensure_up_container(
            &resolved,
            &engine_args(&stopped_error_engine),
            "alpine:3.20",
            "/workspace",
        )
        .err()
        .expect("stopped lookup failure should propagate");
        assert_eq!(stopped_error, "stopped lookup failed");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ensure_engine_container_reuses_running_starts_stopped_and_creates_missing() {
        let root = unique_temp_dir("devcontainer-discovery-engine-paths-test");
        fs::create_dir_all(&root).expect("root dir");
        let resolved = resolved_config(&root, json!({"image": "alpine:3.20"}));

        let reuse_engine = root.join("reuse-docker");
        write_executable_script(
            &reuse_engine,
            &format!(
                r#"#!/bin/sh
set -eu
case "$1" in
  ps)
    case " $* " in
      *" -a "*)
        exit 0
        ;;
      *)
        printf 'running-container\n'
        ;;
    esac
    ;;
  inspect)
    printf '%s\n' '[{{"Config":{{"Labels":{{"devcontainer.local_folder":"{workspace}","devcontainer.config_file":"{config}"}}}}}}]'
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
                workspace = root.display(),
                config = root
                    .join(".devcontainer")
                    .join("devcontainer.json")
                    .display()
            ),
        );
        let reused = ensure_up_container(
            &resolved,
            &engine_args(&reuse_engine),
            "alpine:3.20",
            "/workspace",
        )
        .expect("running container should be reused");
        assert_eq!(reused.container_id, "running-container");
        assert_eq!(reused.lifecycle_mode, LifecycleMode::UpReused);

        let stopped_engine = root.join("stopped-docker");
        write_executable_script(
            &stopped_engine,
            &format!(
                r#"#!/bin/sh
set -eu
case "$1" in
  ps)
    case " $* " in
      *" -a "*)
        printf 'stopped-container\n'
        ;;
      *)
        exit 0
        ;;
    esac
    ;;
  inspect)
    printf '%s\n' '[{{"Config":{{"Labels":{{"devcontainer.local_folder":"{workspace}"}}}}}}]'
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
                workspace = root.display()
            ),
        );
        let started = ensure_up_container(
            &resolved,
            &engine_args(&stopped_engine),
            "alpine:3.20",
            "/workspace",
        )
        .expect("stopped container should be started");
        assert_eq!(started.container_id, "stopped-container");
        assert_eq!(started.lifecycle_mode, LifecycleMode::UpStarted);
        assert_eq!(
            started.matched_id_labels,
            Some(HashMap::from([(
                common::DEVCONTAINER_LOCAL_FOLDER_LABEL.to_string(),
                root.display().to_string(),
            )]))
        );

        let create_engine = root.join("create-docker");
        write_executable_script(
            &create_engine,
            r#"#!/bin/sh
set -eu
case "$1" in
  ps)
    exit 0
    ;;
  run)
    printf 'new-container\n'
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
        );
        let created = ensure_up_container(
            &resolved,
            &engine_args(&create_engine),
            "alpine:3.20",
            "/workspace",
        )
        .expect("missing container should be created");
        assert_eq!(created.container_id, "new-container");
        assert_eq!(created.lifecycle_mode, LifecycleMode::UpCreated);

        let expect_error_args = engine_args(&create_engine)
            .into_iter()
            .chain(["--expect-existing-container".to_string()])
            .collect::<Vec<_>>();
        assert_eq!(
            ensure_up_container(&resolved, &expect_error_args, "alpine:3.20", "/workspace")
                .err()
                .expect("expect existing should reject missing containers"),
            format!(
                "Dev container not found for workspace folder '{}' and config file '{}'. If the container was created with a different config file, pass --config <path> or set DEVCONTAINER_CONFIG.",
                resolved.workspace_folder.display(),
                resolved.config_file.display()
            )
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ensure_engine_container_removes_running_container_when_requested() {
        let root = unique_temp_dir("devcontainer-discovery-engine-remove-running-test");
        fs::create_dir_all(&root).expect("root dir");
        let fake_engine = root.join("docker");
        let rm_log = root.join("rm.log");
        write_executable_script(
            &fake_engine,
            &format!(
                r#"#!/bin/sh
set -eu
case "$1" in
  ps)
    case " $* " in
      *" -a "*)
        exit 0
        ;;
      *)
        printf 'running-container\n'
        ;;
    esac
    ;;
  inspect)
    printf '%s\n' '[{{"Config":{{"Labels":{{"devcontainer.local_folder":"{workspace}","devcontainer.config_file":"{config}"}}}}}}]'
    ;;
  rm)
    printf '%s\n' "$*" >> "{rm_log}"
    ;;
  run)
    printf 'new-container\n'
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
                workspace = root.display(),
                config = root
                    .join(".devcontainer")
                    .join("devcontainer.json")
                    .display(),
                rm_log = rm_log.display()
            ),
        );
        let mut args = engine_args(&fake_engine);
        args.push("--remove-existing-container".to_string());
        let resolved = resolved_config(&root, json!({"image": "alpine:3.20"}));

        let up = ensure_up_container(&resolved, &args, "alpine:3.20", "/workspace")
            .expect("running container should be removed and recreated");

        assert_eq!(up.container_id, "new-container");
        assert!(fs::read_to_string(&rm_log)
            .expect("rm log")
            .contains("rm -f running-container"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ensure_engine_container_removes_stopped_container_when_requested() {
        let root = unique_temp_dir("devcontainer-discovery-engine-remove-stopped-test");
        fs::create_dir_all(&root).expect("root dir");
        let fake_engine = root.join("docker");
        let rm_log = root.join("rm.log");
        write_executable_script(
            &fake_engine,
            &format!(
                r#"#!/bin/sh
set -eu
case "$1" in
  ps)
    case " $* " in
      *" -a "*)
        printf 'stopped-container\n'
        ;;
      *)
        exit 0
        ;;
    esac
    ;;
  rm)
    printf '%s\n' "$*" >> "{rm_log}"
    ;;
  run)
    printf 'new-container\n'
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
                rm_log = rm_log.display()
            ),
        );
        let mut args = engine_args(&fake_engine);
        args.extend([
            "--id-label".to_string(),
            "custom=one".to_string(),
            "--remove-existing-container".to_string(),
        ]);
        let resolved = resolved_config(&root, json!({"image": "alpine:3.20"}));

        let up = ensure_up_container(&resolved, &args, "alpine:3.20", "/workspace")
            .expect("stopped container should be removed and recreated");

        assert_eq!(up.container_id, "new-container");
        assert!(fs::read_to_string(&rm_log)
            .expect("rm log")
            .contains("rm -f stopped-container"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ensure_compose_container_removes_stopped_container_when_requested() {
        let root = unique_temp_dir("devcontainer-discovery-compose-remove-stopped-test");
        let config_root = root.join(".devcontainer");
        fs::create_dir_all(&config_root).expect("config dir");
        fs::write(
            config_root.join("docker-compose.yml"),
            "services:\n  app:\n    image: alpine:3.20\n",
        )
        .expect("compose file");
        let fake_engine = root.join("docker");
        let up_marker = root.join("compose-up-called");
        write_executable_script(
            &fake_engine,
            &format!(
                r#"#!/bin/sh
set -eu
case "$1" in
  compose)
    shift
    case " $* " in
      *" version "*)
        echo "2.24.0"
        ;;
      *" up "*)
        : > "{up_marker}"
        ;;
      *)
        echo "unexpected compose command $*" >&2
        exit 2
        ;;
    esac
    ;;
  ps)
    if [ -f "{up_marker}" ]; then
      printf 'new-compose-container\n'
      exit 0
    fi
    case " $* " in
      *" -a "*)
        printf 'stopped-compose-container\n'
        ;;
      *)
        exit 0
        ;;
    esac
    ;;
  rm)
    exit 0
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
                up_marker = up_marker.display()
            ),
        );
        let mut args = engine_args(&fake_engine);
        args.push("--remove-existing-container".to_string());
        let resolved = resolved_config(
            &root,
            json!({
                "dockerComposeFile": "docker-compose.yml",
                "service": "app"
            }),
        );

        let up = ensure_up_container(&resolved, &args, "alpine:3.20", "/workspace")
            .expect("stopped compose container should be removed and recreated");

        assert_eq!(up.container_id, "new-compose-container");
        assert!(up_marker.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ensure_compose_container_removes_running_container_when_requested() {
        let root = unique_temp_dir("devcontainer-discovery-compose-remove-running-test");
        let config_root = root.join(".devcontainer");
        fs::create_dir_all(&config_root).expect("config dir");
        fs::write(
            config_root.join("docker-compose.yml"),
            "services:\n  app:\n    image: alpine:3.20\n",
        )
        .expect("compose file");
        let fake_engine = root.join("docker");
        let up_marker = root.join("compose-up-called");
        let rm_log = root.join("rm.log");
        write_executable_script(
            &fake_engine,
            &format!(
                r#"#!/bin/sh
set -eu
case "$1" in
  compose)
    shift
    case " $* " in
      *" version "*)
        echo "2.24.0"
        ;;
      *" up "*)
        : > "{up_marker}"
        ;;
      *)
        echo "unexpected compose command $*" >&2
        exit 2
        ;;
    esac
    ;;
  ps)
    if [ -f "{up_marker}" ]; then
      printf 'new-compose-container\n'
      exit 0
    fi
    case " $* " in
      *" -a "*)
        exit 0
        ;;
      *)
        printf 'running-compose-container\n'
        ;;
    esac
    ;;
  rm)
    printf '%s\n' "$*" >> "{rm_log}"
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
                up_marker = up_marker.display(),
                rm_log = rm_log.display()
            ),
        );
        let mut args = engine_args(&fake_engine);
        args.push("--remove-existing-container".to_string());
        let resolved = resolved_config(
            &root,
            json!({
                "dockerComposeFile": "docker-compose.yml",
                "service": "app"
            }),
        );

        let up = ensure_up_container(&resolved, &args, "alpine:3.20", "/workspace")
            .expect("running compose container should be removed and recreated");

        assert_eq!(up.container_id, "new-compose-container");
        assert!(fs::read_to_string(&rm_log)
            .expect("rm log")
            .contains("rm -f running-compose-container"));
        assert!(up_marker.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ensure_compose_container_refreshes_stopped_container_without_recreating() {
        let root = unique_temp_dir("devcontainer-discovery-compose-refresh-stopped-test");
        let config_root = root.join(".devcontainer");
        fs::create_dir_all(&config_root).expect("config dir");
        fs::write(
            config_root.join("docker-compose.yml"),
            "services:\n  app:\n    image: alpine:3.20\n",
        )
        .expect("compose file");
        let fake_engine = root.join("docker");
        let up_marker = root.join("compose-up-called");
        write_executable_script(
            &fake_engine,
            &format!(
                r#"#!/bin/sh
set -eu
case "$1" in
  compose)
    shift
    case " $* " in
      *" version "*)
        echo "2.24.0"
        ;;
      *" up "*)
        : > "{up_marker}"
        exit 0
        ;;
      *)
        echo "unexpected compose command $*" >&2
        exit 2
        ;;
    esac
    ;;
  ps)
    case " $* " in
      *" -a "*)
        printf 'stopped-compose-container\n'
        ;;
      *)
        if [ -f "{up_marker}" ]; then
          printf 'stopped-compose-container\n'
        fi
        ;;
    esac
    ;;
  inspect)
    printf '%s\n' '[{{"Config":{{"Labels":{{"devcontainer.local_folder":"{workspace}"}}}}}}]'
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
                workspace = root.display(),
                up_marker = up_marker.display()
            ),
        );
        let resolved = resolved_config(
            &root,
            json!({
                "dockerComposeFile": "docker-compose.yml",
                "service": "app"
            }),
        );

        let up = ensure_up_container(
            &resolved,
            &engine_args(&fake_engine),
            "alpine:3.20",
            "/workspace",
        )
        .expect("stopped compose container should be refreshed");

        assert_eq!(up.container_id, "stopped-compose-container");
        assert_eq!(up.lifecycle_mode, LifecycleMode::UpStarted);
        assert_eq!(
            up.matched_id_labels,
            Some(HashMap::from([(
                common::DEVCONTAINER_LOCAL_FOLDER_LABEL.to_string(),
                root.display().to_string(),
            )]))
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ensure_compose_container_creates_missing_container_and_honors_expect_existing() {
        let root = unique_temp_dir("devcontainer-discovery-compose-create-test");
        let config_root = root.join(".devcontainer");
        fs::create_dir_all(&config_root).expect("config dir");
        fs::write(
            config_root.join("docker-compose.yml"),
            "services:\n  app:\n    image: alpine:3.20\n",
        )
        .expect("compose file");
        let fake_engine = root.join("docker");
        let up_marker = root.join("compose-up-called");
        write_executable_script(
            &fake_engine,
            &format!(
                r#"#!/bin/sh
set -eu
case "$1" in
  compose)
    shift
    case " $* " in
      *" version "*)
        echo "2.24.0"
        ;;
      *" up "*)
        : > "{up_marker}"
        ;;
      *)
        echo "unexpected compose command $*" >&2
        exit 2
        ;;
    esac
    ;;
  ps)
    if [ -f "{up_marker}" ]; then
      printf 'created-compose-container\n'
    fi
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
                up_marker = up_marker.display()
            ),
        );
        let resolved = resolved_config(
            &root,
            json!({
                "dockerComposeFile": "docker-compose.yml",
                "service": "app"
            }),
        );
        let mut expect_args = engine_args(&fake_engine);
        expect_args.push("--expect-existing-container".to_string());

        let error = ensure_up_container(&resolved, &expect_args, "alpine:3.20", "/workspace")
            .err()
            .expect("expect existing should reject missing compose containers");
        assert_eq!(
            error,
            format!(
                "Dev container not found for workspace folder '{}' and config file '{}'. If the container was created with a different config file, pass --config <path> or set DEVCONTAINER_CONFIG.",
                resolved.workspace_folder.display(),
                resolved.config_file.display()
            )
        );

        let up = ensure_up_container(
            &resolved,
            &engine_args(&fake_engine),
            "alpine:3.20",
            "/workspace",
        )
        .expect("missing compose container should be created");

        assert_eq!(up.container_id, "created-compose-container");
        assert_eq!(up.lifecycle_mode, LifecycleMode::UpCreated);
        assert_eq!(up.matched_id_labels, None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ensure_compose_container_reports_logs_when_created_service_is_not_running() {
        let root = unique_temp_dir("devcontainer-discovery-compose-missing-after-up-test");
        let config_root = root.join(".devcontainer");
        fs::create_dir_all(&config_root).expect("config dir");
        fs::write(
            config_root.join("docker-compose.yml"),
            "services:\n  app:\n    image: alpine:3.20\n",
        )
        .expect("compose file");
        let fake_engine = root.join("docker");
        let up_marker = root.join("compose-up-called");
        write_executable_script(
            &fake_engine,
            &format!(
                r#"#!/bin/sh
set -eu
case "$1" in
  compose)
    shift
    case " $* " in
      *" version "*)
        echo "2.24.0"
        ;;
      *" up "*)
        echo "compose up stdout: service dependencies started"
        echo "compose up stderr: app failed during startup" >&2
        : > "{up_marker}"
        ;;
      *" logs "*)
        echo "app log: migration failed"
        echo "app stderr: stack trace" >&2
        ;;
      *)
        echo "unexpected compose command $*" >&2
        exit 2
        ;;
    esac
    ;;
  ps)
    if [ -f "{up_marker}" ]; then
      case " $* " in
        *" -a "*)
          printf 'stopped-compose-container\n'
          ;;
        *)
          exit 0
          ;;
      esac
    fi
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
                up_marker = up_marker.display()
            ),
        );
        let resolved = resolved_config(
            &root,
            json!({
                "dockerComposeFile": "docker-compose.yml",
                "service": "app"
            }),
        );

        let error = ensure_up_container(
            &resolved,
            &engine_args(&fake_engine),
            "alpine:3.20",
            "/workspace",
        )
        .err()
        .expect("stopped service should report compose diagnostics");

        assert!(
            error.contains("Dev container service 'app' for compose project '"),
            "{error}"
        );
        assert!(
            error.contains(
                "' was created but is not running (container stopped-compose-container)."
            ),
            "{error}"
        );
        assert!(
            error.contains("Compose up stdout:\ncompose up stdout: service dependencies started"),
            "{error}"
        );
        assert!(
            error.contains("Compose up stderr:\ncompose up stderr: app failed during startup"),
            "{error}"
        );
        assert!(
            error.contains("Compose logs stdout:\napp log: migration failed"),
            "{error}"
        );
        assert!(
            error.contains("Compose logs stderr:\napp stderr: stack trace"),
            "{error}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn compose_startup_failure_reports_missing_service_and_empty_failing_logs() {
        let root = unique_temp_dir("devcontainer-discovery-compose-missing-service-test");
        let config_root = root.join(".devcontainer");
        fs::create_dir_all(&config_root).expect("config dir");
        fs::write(
            config_root.join("docker-compose.yml"),
            "services:\n  app:\n    image: alpine:3.20\n",
        )
        .expect("compose file");
        let fake_engine = root.join("docker");
        write_executable_script(
            &fake_engine,
            r#"#!/bin/sh
set -eu
case "$1" in
  compose)
    shift
    case " $* " in
      *" version "*)
        echo "2.24.0"
        ;;
      *" logs "*)
        exit 7
        ;;
      *)
        echo "unexpected compose command $*" >&2
        exit 2
        ;;
    esac
    ;;
  ps)
    exit 0
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
        );
        let resolved = resolved_config(
            &root,
            json!({
                "dockerComposeFile": "docker-compose.yml",
                "service": "app"
            }),
        );
        let up_result = ComposeUpResult {
            project_name: "missing-project".to_string(),
            service: "app".to_string(),
            stdout: String::new(),
            stderr: String::new(),
        };

        let error =
            compose_startup_failure_error(&resolved, &engine_args(&fake_engine), &up_result)
                .expect("diagnostic message");

        assert_eq!(
            error,
            "Dev container service 'app' for compose project 'missing-project' was not found after compose up.\nCompose logs exited with status 7 without output."
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn compose_startup_failure_reports_unavailable_logs() {
        let root = unique_temp_dir("devcontainer-discovery-compose-logs-unavailable-test");
        let config_root = root.join(".devcontainer");
        fs::create_dir_all(&config_root).expect("config dir");
        fs::write(
            config_root.join("docker-compose.yml"),
            "services:\n  app:\n    image: alpine:3.20\n",
        )
        .expect("compose file");
        let fake_engine = root.join("docker");
        let missing_compose = root.join("missing-compose");
        write_executable_script(
            &fake_engine,
            r#"#!/bin/sh
set -eu
case "$1" in
  ps)
    exit 0
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
        );
        let mut args = engine_args(&fake_engine);
        args.extend([
            "--docker-compose-path".to_string(),
            missing_compose.display().to_string(),
        ]);
        let resolved = resolved_config(
            &root,
            json!({
                "dockerComposeFile": "docker-compose.yml",
                "service": "app"
            }),
        );
        let up_result = ComposeUpResult {
            project_name: "missing-project".to_string(),
            service: "app".to_string(),
            stdout: String::new(),
            stderr: String::new(),
        };

        let error = compose_startup_failure_error(&resolved, &args, &up_result)
            .expect("diagnostic message");

        assert!(
            error.contains(
                "Dev container service 'app' for compose project 'missing-project' was not found after compose up."
            ),
            "{error}"
        );
        assert!(
            error.contains("Compose logs unavailable:\nContainer compose executable not found:"),
            "{error}"
        );
        assert!(
            error.contains(
                "Verify --docker-compose-path or DEVCONTAINER_DOCKER_COMPOSE_PATH, or install the requested compose CLI."
            ),
            "{error}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn compose_startup_failure_truncates_large_diagnostics() {
        let root = unique_temp_dir("devcontainer-discovery-compose-truncate-diagnostics-test");
        let config_root = root.join(".devcontainer");
        fs::create_dir_all(&config_root).expect("config dir");
        fs::write(
            config_root.join("docker-compose.yml"),
            "services:\n  app:\n    image: alpine:3.20\n",
        )
        .expect("compose file");
        let fake_engine = root.join("docker");
        write_executable_script(
            &fake_engine,
            r#"#!/bin/sh
set -eu
case "$1" in
  compose)
    shift
    case " $* " in
      *" version "*)
        echo "2.24.0"
        ;;
      *" logs "*)
        exit 0
        ;;
      *)
        echo "unexpected compose command $*" >&2
        exit 2
        ;;
    esac
    ;;
  ps)
    printf 'stopped-compose-container\n'
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
        );
        let resolved = resolved_config(
            &root,
            json!({
                "dockerComposeFile": "docker-compose.yml",
                "service": "app"
            }),
        );
        let mut stdout = "x".repeat(16 * 1024 - 1);
        stdout.push('🙂');
        stdout.push_str("tail");
        let up_result = ComposeUpResult {
            project_name: "truncate-project".to_string(),
            service: "app".to_string(),
            stdout,
            stderr: String::new(),
        };

        let error =
            compose_startup_failure_error(&resolved, &engine_args(&fake_engine), &up_result)
                .expect("diagnostic message");

        assert!(
            error.contains("was created but is not running (container stopped-compose-container)."),
            "{error}"
        );
        assert!(
            error.contains("... truncated compose diagnostic output after 16384 bytes ..."),
            "{error}"
        );
        assert!(!error.contains('🙂'), "{error}");
        assert!(error.len() < 17 * 1024, "{error}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ensure_compose_container_reports_logs_when_refreshed_service_is_not_running() {
        let root = unique_temp_dir("devcontainer-discovery-compose-refresh-missing-test");
        let config_root = root.join(".devcontainer");
        fs::create_dir_all(&config_root).expect("config dir");
        fs::write(
            config_root.join("docker-compose.yml"),
            "services:\n  app:\n    image: alpine:3.20\n",
        )
        .expect("compose file");
        let fake_engine = root.join("docker");
        let up_marker = root.join("compose-up-called");
        write_executable_script(
            &fake_engine,
            &format!(
                r#"#!/bin/sh
set -eu
case "$1" in
  compose)
    shift
    case " $* " in
      *" version "*)
        echo "2.24.0"
        ;;
      *" up "*)
        echo "refresh up output"
        : > "{up_marker}"
        ;;
      *" logs "*)
        echo "refresh logs output"
        ;;
      *)
        echo "unexpected compose command $*" >&2
        exit 2
        ;;
    esac
    ;;
  ps)
    if [ -f "{up_marker}" ]; then
      case " $* " in
        *" -a "*)
          printf 'existing-compose-container\n'
          ;;
        *)
          exit 0
          ;;
      esac
    else
      printf 'existing-compose-container\n'
    fi
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
                up_marker = up_marker.display()
            ),
        );
        let resolved = resolved_config(
            &root,
            json!({
                "dockerComposeFile": "docker-compose.yml",
                "service": "app"
            }),
        );

        let error = ensure_up_container(
            &resolved,
            &engine_args(&fake_engine),
            "alpine:3.20",
            "/workspace",
        )
        .err()
        .expect("refresh failure should report compose diagnostics");

        assert!(
            error
                .contains("was created but is not running (container existing-compose-container)."),
            "{error}"
        );
        assert!(
            error.contains("Compose up stdout:\nrefresh up output"),
            "{error}"
        );
        assert!(
            error.contains("Compose logs stdout:\nrefresh logs output"),
            "{error}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ensure_compose_container_marks_changed_container_id_as_created() {
        let root = unique_temp_dir("devcontainer-discovery-compose-refresh-created-test");
        let config_root = root.join(".devcontainer");
        fs::create_dir_all(&config_root).expect("config dir");
        fs::write(
            config_root.join("docker-compose.yml"),
            "services:\n  app:\n    image: alpine:3.20\n",
        )
        .expect("compose file");
        let fake_engine = root.join("docker");
        let up_marker = root.join("compose-up-called");
        write_executable_script(
            &fake_engine,
            &format!(
                r#"#!/bin/sh
set -eu
case "$1" in
  compose)
    shift
    case " $* " in
      *" version "*)
        echo "2.24.0"
        ;;
      *" up "*)
        : > "{up_marker}"
        ;;
      *)
        echo "unexpected compose command $*" >&2
        exit 2
        ;;
    esac
    ;;
  ps)
    if [ -f "{up_marker}" ]; then
      printf 'replacement-compose-container\n'
    else
      printf 'original-compose-container\n'
    fi
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
                up_marker = up_marker.display()
            ),
        );
        let resolved = resolved_config(
            &root,
            json!({
                "dockerComposeFile": "docker-compose.yml",
                "service": "app"
            }),
        );

        let up = ensure_up_container(
            &resolved,
            &engine_args(&fake_engine),
            "alpine:3.20",
            "/workspace",
        )
        .expect("changed compose container should be treated as created");

        assert_eq!(up.container_id, "replacement-compose-container");
        assert_eq!(up.lifecycle_mode, LifecycleMode::UpCreated);
        assert_eq!(up.matched_id_labels, None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ensure_compose_container_propagates_refresh_label_inspect_errors() {
        let root = unique_temp_dir("devcontainer-discovery-compose-refresh-error-test");
        let config_root = root.join(".devcontainer");
        fs::create_dir_all(&config_root).expect("config dir");
        fs::write(
            config_root.join("docker-compose.yml"),
            "services:\n  app:\n    image: alpine:3.20\n",
        )
        .expect("compose file");
        let fake_engine = root.join("docker");
        write_executable_script(
            &fake_engine,
            r#"#!/bin/sh
set -eu
case "$1" in
  compose)
    shift
    case " $* " in
      *" version "*)
        echo "2.24.0"
        ;;
      *" up "*)
        exit 0
        ;;
      *)
        echo "unexpected compose command $*" >&2
        exit 2
        ;;
    esac
    ;;
  ps)
    printf 'same-compose-container\n'
    ;;
  inspect)
    echo "inspect labels failed" >&2
    exit 2
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
        );
        let resolved = resolved_config(
            &root,
            json!({
                "dockerComposeFile": "docker-compose.yml",
                "service": "app"
            }),
        );

        let error = ensure_up_container(
            &resolved,
            &engine_args(&fake_engine),
            "alpine:3.20",
            "/workspace",
        )
        .err()
        .expect("refresh inspect failure should propagate");

        assert_eq!(error, "inspect labels failed");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn windows_target_lookup_uses_normalized_default_label_fallback() {
        let root = unique_temp_dir("devcontainer-discovery-windows-fallback-test");
        fs::create_dir_all(&root).expect("root dir");
        let fake_engine = root.join("docker");
        write_executable_script(
            &fake_engine,
            r#"#!/bin/sh
set -eu
case "$1" in
  ps)
    case " $* " in
      *devcontainer.config_file*)
        exit 0
        ;;
      *)
        printf 'legacy-container\n'
        ;;
    esac
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
        let args = engine_args(&fake_engine);

        let target = find_target_container_for_platform(
            "windows",
            &args,
            Some(Path::new("c:\\CodeBlocks\\remill")),
            Some(Path::new(
                "c:\\CodeBlocks\\remill\\.devcontainer\\devcontainer.json",
            )),
            false,
        )
        .expect("fallback lookup should succeed")
        .expect("legacy fallback should match");

        assert_eq!(target.container_id, "legacy-container");
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
    printf 'missing\nmismatch\nlegacy\ncurrent\nbad id\n'
    ;;
  inspect)
    case "$2" in
      missing)
        printf '%s\n' '[{"Config":{}}]'
        ;;
      mismatch)
        printf '%s\n' '[{"Config":{"Labels":{"devcontainer.local_folder":"C:\\Other\\workspace","devcontainer.config_file":"C:\\Other\\workspace\\.devcontainer\\devcontainer.json"}}}]'
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
    fn normalized_default_label_lookup_without_workspace_is_empty() {
        assert_eq!(
            find_normalized_default_label_match(&[], None, None, false)
                .expect("missing workspace should not invoke engine"),
            None
        );
    }
}
