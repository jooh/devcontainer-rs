//! Configuration upgrade, lockfile, and outdated command helpers.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::{json, Map, Value};

use super::catalog::build_feature_version_info;
use super::features::ResolvedFeatureSupport;
use super::load::load_config;
use super::{FeatureReference, Lockfile, LockfileEntry};
use crate::commands::common;
use crate::output::{CommandLogLevel, CommandLogger, LogFormat, TerminalDimensions};

const NO_LOCKFILE_FLAG: &str = "--no-lockfile";
const FROZEN_LOCKFILE_FLAG: &str = "--frozen-lockfile";
const EXPERIMENTAL_LOCKFILE_FLAG: &str = "--experimental-lockfile";
const EXPERIMENTAL_FROZEN_LOCKFILE_FLAG: &str = "--experimental-frozen-lockfile";

pub(super) fn run_outdated(args: &[String]) -> ExitCode {
    let logger = outdated_logger(args);
    let result = match validate_outdated_options(args) {
        Ok(()) => build_outdated_payload_with_logger(args, Some(&logger)),
        Err(error) => Err(error),
    };
    match result {
        Ok(payload) => {
            let output_format =
                common::parse_option_value(args, "--output-format").unwrap_or("json".to_string());
            if output_format == "text" {
                println!("{}", render_outdated_text(&payload));
            } else {
                println!("{payload}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            logger.error(error);
            ExitCode::from(1)
        }
    }
}

pub(super) fn run_upgrade(args: &[String]) -> ExitCode {
    let logger = upgrade_logger(args);
    let result = match validate_upgrade_command_options(args) {
        Ok(()) => run_upgrade_lockfile_with_logger(args, Some(&logger)),
        Err(error) => Err(error),
    };
    match result {
        Ok(lockfile) => {
            if common::has_flag(args, "--dry-run") {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&lockfile).expect("lockfile json")
                );
            } else {
                println!(
                    "{}",
                    json!({
                        "outcome": "success",
                        "command": "upgrade",
                        "lockfile": lockfile,
                    })
                );
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            logger.error(error);
            ExitCode::from(1)
        }
    }
}

pub(super) fn ensure_native_lockfile(
    args: &[String],
    config_file: &Path,
    configuration: &Value,
    resolved_features: &ResolvedFeatureSupport,
) -> Result<(), String> {
    validate_lockfile_options(args)?;
    if lockfile_disabled(args) {
        return Ok(());
    }

    let generated = generate_lockfile_from_resolved(args, configuration, resolved_features)?;
    let path = lockfile_path(config_file);
    let existing = existing_native_lockfile(args, &path)?;
    if lockfile_frozen(args) {
        let Some(existing) = existing else {
            return Err("Lockfile does not exist.".to_string());
        };
        if existing != generated {
            return Err(format!(
                "Lockfile at {} is out of date for the current feature configuration",
                path.display()
            ));
        }
        return Ok(());
    }
    let lockfile = serialized_lockfile(&generated)?;
    fs::write(&path, lockfile).map_err(error_to_string)?;
    Ok(())
}

pub(super) fn validate_native_lockfile(
    args: &[String],
    config_file: &Path,
    configuration: &Value,
    resolved_features: &ResolvedFeatureSupport,
) -> Result<(), String> {
    validate_lockfile_options(args)?;
    if lockfile_disabled(args) {
        return Ok(());
    }

    let path = lockfile_path(config_file);
    let existing = existing_native_lockfile(args, &path)?;
    if lockfile_frozen(args) {
        let Some(existing) = existing else {
            return Err("Lockfile does not exist.".to_string());
        };
        let generated = generate_lockfile_from_resolved(args, configuration, resolved_features)?;
        if existing != generated {
            return Err(format!(
                "Lockfile at {} is out of date for the current feature configuration",
                path.display()
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_lockfile_options(args: &[String]) -> Result<(), String> {
    if common::has_flag(args, NO_LOCKFILE_FLAG) {
        for flag in [
            FROZEN_LOCKFILE_FLAG,
            EXPERIMENTAL_FROZEN_LOCKFILE_FLAG,
            EXPERIMENTAL_LOCKFILE_FLAG,
        ] {
            if common::has_flag(args, flag) {
                return Err(format!(
                    "{NO_LOCKFILE_FLAG} and {flag} are mutually exclusive."
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn warn_deprecated_lockfile_flags(args: &[String]) {
    if common::has_flag(args, EXPERIMENTAL_LOCKFILE_FLAG) {
        eprintln!(
            "Warning: {EXPERIMENTAL_LOCKFILE_FLAG} is deprecated. Lockfiles are now enabled by default."
        );
    }
    if common::has_flag(args, EXPERIMENTAL_FROZEN_LOCKFILE_FLAG) {
        eprintln!(
            "Warning: {EXPERIMENTAL_FROZEN_LOCKFILE_FLAG} is deprecated. Use {FROZEN_LOCKFILE_FLAG} instead."
        );
    }
}

fn lockfile_disabled(args: &[String]) -> bool {
    common::has_flag(args, NO_LOCKFILE_FLAG)
}

fn lockfile_frozen(args: &[String]) -> bool {
    common::has_flag(args, FROZEN_LOCKFILE_FLAG)
        || common::has_flag(args, EXPERIMENTAL_FROZEN_LOCKFILE_FLAG)
}

fn existing_native_lockfile(args: &[String], path: &Path) -> Result<Option<Lockfile>, String> {
    if path.exists() || lockfile_frozen(args) {
        read_lockfile(path.to_path_buf())
    } else {
        Ok(None)
    }
}

pub(super) fn lockfile_for_resolution(
    args: &[String],
    config_file: &Path,
) -> Result<Option<Lockfile>, String> {
    validate_lockfile_options(args)?;
    if lockfile_disabled(args) {
        return Ok(None);
    }
    read_lockfile(lockfile_path(config_file))
}

fn serialized_lockfile(lockfile: &Lockfile) -> Result<String, String> {
    serde_json::to_string_pretty(lockfile)
        .map(|json| format!("{json}\n"))
        .map_err(error_to_string)
}

#[cfg(test)]
pub(super) fn build_outdated_payload(args: &[String]) -> Result<Value, String> {
    build_outdated_payload_with_logger(args, None)
}

fn build_outdated_payload_with_logger(
    args: &[String],
    logger: Option<&CommandLogger>,
) -> Result<Value, String> {
    if let Some(logger) = logger {
        logger.debug("Loading dev container configuration");
        logger.trace_terminal_dimensions();
    }
    let loaded = load_config(args)?;
    if let Some(logger) = logger {
        logger.debug(format!(
            "Loading dev container configuration from {}",
            loaded.config_file.display()
        ));
    }
    let lockfile_path = lockfile_path(&loaded.config_file);
    let lockfile = read_lockfile(lockfile_path.clone())?;
    if let Some(logger) = logger {
        if lockfile.is_some() {
            logger.debug(format!("Loaded lockfile from {}", lockfile_path.display()));
        } else {
            logger.debug(format!("No lockfile found at {}", lockfile_path.display()));
        }
    }
    let features = loaded
        .configuration
        .get("features")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(logger) = logger {
        logger.trace(format!(
            "Enumerating {} configured feature definition(s)",
            features.len()
        ));
    }

    let mut payload_features = Map::new();
    for feature_id in features.keys() {
        let Some(reference) = parse_feature_reference(feature_id) else {
            continue;
        };

        if let Some(feature_info) = build_feature_version_info(
            &reference,
            lockfile.as_ref(),
            Some(loaded.workspace_folder.as_path()),
        )? {
            payload_features.insert(feature_id.clone(), feature_info);
        }
    }

    if let Some(logger) = logger {
        logger.debug(format!(
            "Generated outdated payload for {} feature(s)",
            payload_features.len()
        ));
    }
    Ok(json!({
        "features": payload_features,
    }))
}

#[cfg(test)]
pub(super) fn run_upgrade_lockfile(args: &[String]) -> Result<Lockfile, String> {
    run_upgrade_lockfile_with_logger(args, None)
}

fn run_upgrade_lockfile_with_logger(
    args: &[String],
    logger: Option<&CommandLogger>,
) -> Result<Lockfile, String> {
    if let Some(logger) = logger {
        logger.debug("Loading dev container configuration");
    }
    let mut loaded = load_config(args)?;
    if let Some(logger) = logger {
        logger.debug(format!(
            "Loading dev container configuration from {}",
            loaded.config_file.display()
        ));
    }
    if let (Some(feature), Some(target_version)) = (
        common::parse_option_value(args, "--feature"),
        common::parse_option_value(args, "--target-version"),
    ) {
        if let Some(logger) = logger {
            logger.info(format!(
                "Updating '{feature}' to '{target_version}' in devcontainer.json"
            ));
        }
        update_feature_version_in_config(
            &loaded.config_file,
            &loaded.raw_text,
            &loaded.configuration,
            &feature,
            &target_version,
            logger,
        )?;
        if let Some(logger) = logger {
            logger.debug("Reloading dev container configuration after feature update");
        }
        loaded = load_config(args)?;
        if let Some(logger) = logger {
            logger.debug(format!(
                "Loading dev container configuration from {}",
                loaded.config_file.display()
            ));
        }
    }

    let feature_count = loaded
        .configuration
        .get("features")
        .and_then(Value::as_object)
        .map_or(0, Map::len);
    if let Some(logger) = logger {
        logger.debug(format!(
            "Generating lockfile for {feature_count} feature(s)"
        ));
    }
    let resolve = super::resolve_feature_support_without_lockfile;
    let workspace_folder = &loaded.workspace_folder;
    let config_file = &loaded.config_file;
    let configuration = &loaded.configuration;
    let resolved_features = resolve(args, workspace_folder, config_file, configuration)?;
    let generated = if let Some(resolved_features) = resolved_features {
        generate_lockfile_from_resolved(args, &loaded.configuration, &resolved_features)?
    } else {
        Lockfile {
            features: std::collections::BTreeMap::new(),
        }
    };
    if !common::has_flag(args, "--dry-run") {
        let lockfile_path = lockfile_path(&loaded.config_file);
        if let Some(logger) = logger {
            logger.info(format!("Writing lockfile: '{}'", lockfile_path.display()));
        }
        fs::write(&lockfile_path, serialized_lockfile(&generated)?).map_err(error_to_string)?;
        if let Some(logger) = logger {
            logger.debug(format!(
                "Lockfile write complete: '{}'",
                lockfile_path.display()
            ));
        }
    } else if let Some(logger) = logger {
        logger.debug("Dry-run lockfile generation complete");
    }

    Ok(generated)
}

fn validate_outdated_options(args: &[String]) -> Result<(), String> {
    let options = [
        "--user-data-folder",
        "--workspace-folder",
        "--config",
        "--output-format",
        "--log-level",
        "--log-format",
        "--terminal-columns",
        "--terminal-rows",
    ];
    common::validate_option_values(args, &options)
        .and_then(|()| common::validate_choice_option(args, "--output-format", &["text", "json"]))
        .and_then(|()| common::validate_choice_option(args, "--log-format", &["text", "json"]))
        .and_then(|()| {
            common::validate_choice_option(args, "--log-level", &["info", "debug", "trace"])
        })
        .and_then(|()| {
            common::validate_paired_options(args, "--terminal-columns", "--terminal-rows")
        })
        .and_then(|()| common::validate_number_option(args, "--terminal-columns"))
        .and_then(|()| common::validate_number_option(args, "--terminal-rows"))
}

fn validate_upgrade_command_options(args: &[String]) -> Result<(), String> {
    let options = [
        "--workspace-folder",
        "--docker-path",
        "--docker-compose-path",
        "--config",
        "--log-level",
        "--feature",
        "--target-version",
    ];
    common::validate_option_values(args, &options)
        .and_then(|()| {
            common::validate_choice_option(
                args,
                "--log-level",
                &["error", "info", "debug", "trace"],
            )
        })
        .and_then(|()| validate_upgrade_options(args))
}

fn validate_upgrade_options(args: &[String]) -> Result<(), String> {
    let feature = common::parse_option_value(args, "--feature");
    let target_version = common::parse_option_value(args, "--target-version");

    if feature.is_some() != target_version.is_some() {
        return Err(
            "The '--target-version' and '--feature' flag must be used together.".to_string(),
        );
    }

    if let Some(version) = target_version {
        if !version
            .chars()
            .all(|character| character.is_ascii_digit() || character == '.')
            || version.is_empty()
        {
            return Err(format!(
                "Invalid version '{version}'. Must be in the form of 'x', 'x.y', or 'x.y.z'"
            ));
        }
    }

    Ok(())
}

fn generate_lockfile_from_resolved(
    args: &[String],
    configuration: &Value,
    resolved_features: &ResolvedFeatureSupport,
) -> Result<Lockfile, String> {
    let excluded = additional_only_feature_ids(args, configuration)?;
    let mut resolved = std::collections::BTreeMap::new();
    for feature in &resolved_features.lockfile_features {
        if excluded.contains(&feature.user_feature_id) {
            continue;
        }
        resolved.insert(
            feature.user_feature_id.clone(),
            LockfileEntry {
                version: feature.version.clone(),
                resolved: feature.resolved.clone(),
                integrity: feature.integrity.clone(),
                depends_on: feature.depends_on.clone(),
            },
        );
    }

    Ok(Lockfile { features: resolved })
}

fn additional_only_feature_ids(
    args: &[String],
    configuration: &Value,
) -> Result<BTreeSet<String>, String> {
    let config_feature_keys = configuration
        .get("features")
        .and_then(Value::as_object)
        .map(|features| features.keys().cloned().collect::<BTreeSet<_>>())
        .unwrap_or_default();
    let Some(raw_additional) = common::parse_option_value(args, "--additional-features") else {
        return Ok(BTreeSet::new());
    };
    crate::config::parse_jsonc_value(&raw_additional).and_then(|additional| {
        additional.as_object().map_or_else(
            || Err("--additional-features must be a JSON object".to_string()),
            |additional| {
                Ok(additional
                    .keys()
                    .filter(|key| !config_feature_keys.contains(*key))
                    .cloned()
                    .collect())
            },
        )
    })
}

pub(super) fn lockfile_path(config_file: &Path) -> PathBuf {
    let file_name = config_file
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("devcontainer.json");
    let lockfile_name = if file_name.starts_with('.') {
        ".devcontainer-lock.json"
    } else {
        "devcontainer-lock.json"
    };
    config_file
        .parent()
        .unwrap_or(config_file)
        .join(lockfile_name)
}

fn read_lockfile(path: PathBuf) -> Result<Option<Lockfile>, String> {
    match fs::read_to_string(path) {
        Ok(contents) if contents.trim().is_empty() => Ok(None),
        Ok(contents) => serde_json::from_str(&contents)
            .map(Some)
            .map_err(error_to_string),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn update_feature_version_in_config(
    config_path: &Path,
    raw_text: &str,
    configuration: &Value,
    target_feature: &str,
    target_version: &str,
    logger: Option<&CommandLogger>,
) -> Result<(), String> {
    let target_base = feature_id_without_version(target_feature);
    let current_key = configuration
        .get("features")
        .and_then(Value::as_object)
        .and_then(|entries| {
            entries
                .keys()
                .find(|feature_id| feature_id_without_version(feature_id) == target_base)
        })
        .cloned();

    let Some(current_key) = current_key else {
        if let Some(logger) = logger {
            logger.trace(format!(
                "No changes to config file: {}",
                config_path.display()
            ));
        }
        return Ok(());
    };

    let updated = raw_text.replace(&current_key, &format!("{target_base}:{target_version}"));
    if let Some(logger) = logger {
        logger.trace(updated.as_str());
    }
    if updated != raw_text {
        if let Some(logger) = logger {
            logger.info(format!("Updating config file: '{}'", config_path.display()));
        }
        fs::write(config_path, updated).map_err(error_to_string)?;
    } else if let Some(logger) = logger {
        logger.trace(format!(
            "No changes to config file: {}",
            config_path.display()
        ));
    }
    Ok(())
}

pub(super) fn render_outdated_text(payload: &Value) -> String {
    let mut rows = vec![vec![
        "Feature".to_string(),
        "Current".to_string(),
        "Wanted".to_string(),
        "Latest".to_string(),
    ]];

    if let Some(features) = payload.get("features").and_then(Value::as_object) {
        for (key, value) in features {
            rows.push(vec![
                feature_id_without_version(key),
                cell(value.get("current")),
                cell(value.get("wanted")),
                cell(value.get("latest")),
            ]);
        }
    }

    let widths = (0..rows[0].len())
        .map(|index| rows.iter().map(|row| row[index].len()).max().unwrap_or(0))
        .collect::<Vec<_>>();

    rows.into_iter()
        .map(|row| {
            row.into_iter()
                .enumerate()
                .map(|(index, cell)| format!("{cell:width$}", width = widths[index]))
                .collect::<Vec<_>>()
                .join("  ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn cell(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).unwrap_or("-").to_string()
}

fn error_to_string(error: impl ToString) -> String {
    error.to_string()
}

fn outdated_logger(args: &[String]) -> CommandLogger {
    CommandLogger::new(
        parse_requested_log_format(args),
        parse_outdated_log_level(args),
    )
    .with_terminal_dimensions(parse_terminal_dimensions(args))
}

fn upgrade_logger(args: &[String]) -> CommandLogger {
    CommandLogger::new(LogFormat::Text, parse_upgrade_log_level(args))
}

fn parse_requested_log_format(args: &[String]) -> LogFormat {
    match common::parse_option_value(args, "--log-format").as_deref() {
        Some("json") => LogFormat::Json,
        _ => LogFormat::Text,
    }
}

fn parse_outdated_log_level(args: &[String]) -> CommandLogLevel {
    match common::parse_option_value(args, "--log-level").as_deref() {
        Some("trace") => CommandLogLevel::Trace,
        Some("debug") => CommandLogLevel::Debug,
        _ => CommandLogLevel::Info,
    }
}

fn parse_upgrade_log_level(args: &[String]) -> CommandLogLevel {
    match common::parse_option_value(args, "--log-level").as_deref() {
        Some("error") => CommandLogLevel::Error,
        Some("trace") => CommandLogLevel::Trace,
        Some("debug") => CommandLogLevel::Debug,
        _ => CommandLogLevel::Info,
    }
}

fn parse_terminal_dimensions(args: &[String]) -> Option<TerminalDimensions> {
    common::parse_option_value(args, "--terminal-columns")
        .and_then(|value| value.parse::<usize>().ok())
        .zip(
            common::parse_option_value(args, "--terminal-rows")
                .and_then(|value| value.parse::<usize>().ok()),
        )
        .map(|(columns, rows)| TerminalDimensions { columns, rows })
}

pub(super) fn parse_feature_reference(feature_id: &str) -> Option<FeatureReference> {
    if !feature_id.starts_with("ghcr.io/")
        && !feature_id.starts_with("https://")
        && !feature_id.starts_with("http://")
    {
        return None;
    }

    let base = feature_id_without_version(feature_id);
    feature_id.strip_prefix(&base).and_then(|suffix| {
        if suffix.is_empty() {
            return Some(FeatureReference {
                original: feature_id.to_string(),
                base,
                tag: None,
                digest: None,
            });
        }

        if let Some(digest) = suffix.strip_prefix('@') {
            return Some(FeatureReference {
                original: feature_id.to_string(),
                base,
                tag: None,
                digest: Some(digest.to_string()),
            });
        }

        suffix.strip_prefix(':').map(|tag| FeatureReference {
            original: feature_id.to_string(),
            base,
            tag: Some(tag.to_string()),
            digest: None,
        })
    })
}

pub(super) fn feature_id_without_version(feature_id: &str) -> String {
    if let Some(index) = feature_id.find("@sha256:") {
        return feature_id[..index].to_string();
    }

    let last_slash = feature_id.rfind('/').unwrap_or(0);
    let last_colon = feature_id.rfind(':');
    let last_at = feature_id.rfind('@');
    let delimiter = match (last_colon, last_at) {
        (Some(colon), Some(at)) => Some(colon.max(at)),
        (Some(colon), None) => Some(colon),
        (None, Some(at)) => Some(at),
        (None, None) => None,
    };

    match delimiter.filter(|index| *index > last_slash) {
        Some(index) => feature_id[..index].to_string(),
        None => feature_id.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::additional_only_feature_ids;

    #[test]
    fn additional_only_feature_ids_rejects_non_object_payloads() {
        let error = additional_only_feature_ids(
            &["--additional-features".to_string(), "[]".to_string()],
            &json!({}),
        )
        .expect_err("array payload should be rejected");

        assert_eq!(error, "--additional-features must be a JSON object");
    }
}
