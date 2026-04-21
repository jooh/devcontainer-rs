//! Configuration upgrade, lockfile, and outdated command helpers.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::{json, Map, Value};

use super::catalog::{
    build_feature_version_info, catalog_entry_for_version, exact_catalog_entry, latest_version,
    resolve_wanted_version,
};
use super::load::load_config;
use super::{FeatureReference, Lockfile, LockfileEntry};
use crate::commands::common;
use crate::output::{CommandLogLevel, CommandLogger, LogFormat, TerminalDimensions};

pub(super) fn run_outdated(args: &[String]) -> ExitCode {
    let logger = outdated_logger(args);
    match validate_outdated_options(args)
        .and_then(|()| build_outdated_payload_with_logger(args, Some(&logger)))
    {
        Ok(payload) => {
            let output_format = common::parse_option_value(args, "--output-format")
                .unwrap_or_else(|| "json".to_string());
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
    match validate_upgrade_command_options(args)
        .and_then(|()| run_upgrade_lockfile_with_logger(args, Some(&logger)))
    {
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
) -> Result<(), String> {
    let wants_lockfile = common::has_flag(args, "--experimental-lockfile")
        || common::has_flag(args, "--experimental-frozen-lockfile");
    if !wants_lockfile {
        return Ok(());
    }

    let workspace_folder = common::parse_option_value(args, "--workspace-folder")
        .map(PathBuf::from)
        .or_else(|| config_file.parent().map(Path::to_path_buf));
    let generated = generate_lockfile(configuration, workspace_folder.as_deref())?;
    let path = lockfile_path(config_file);
    if common::has_flag(args, "--experimental-frozen-lockfile") {
        let existing = read_lockfile(path.clone())?;
        let Some(existing) = existing else {
            return Err("Lockfile does not exist.".to_string());
        };
        if existing != generated {
            return Err(format!(
                "Lockfile at {} is out of date for the current feature configuration",
                path.display()
            ));
        }
    }
    if common::has_flag(args, "--experimental-lockfile") {
        let lockfile = serialized_lockfile(&generated)?;
        fs::write(&path, lockfile).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn serialized_lockfile(lockfile: &Lockfile) -> Result<String, String> {
    serde_json::to_string_pretty(lockfile).map_err(|error| error.to_string())
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

        let Some(feature_info) = build_feature_version_info(
            &reference,
            lockfile.as_ref(),
            Some(loaded.workspace_folder.as_path()),
        ) else {
            continue;
        };
        payload_features.insert(feature_id.clone(), feature_info);
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
    let generated = generate_lockfile(
        &loaded.configuration,
        Some(loaded.workspace_folder.as_path()),
    )?;
    if !common::has_flag(args, "--dry-run") {
        let lockfile_path = lockfile_path(&loaded.config_file);
        if let Some(logger) = logger {
            logger.info(format!("Writing lockfile: '{}'", lockfile_path.display()));
        }
        fs::write(&lockfile_path, serialized_lockfile(&generated)?)
            .map_err(|error| error.to_string())?;
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
    common::validate_option_values(
        args,
        &[
            "--user-data-folder",
            "--workspace-folder",
            "--config",
            "--output-format",
            "--log-level",
            "--log-format",
            "--terminal-columns",
            "--terminal-rows",
        ],
    )?;
    common::validate_choice_option(args, "--output-format", &["text", "json"])?;
    common::validate_choice_option(args, "--log-format", &["text", "json"])?;
    common::validate_choice_option(args, "--log-level", &["info", "debug", "trace"])?;
    common::validate_paired_options(args, "--terminal-columns", "--terminal-rows")?;
    common::validate_number_option(args, "--terminal-columns")?;
    common::validate_number_option(args, "--terminal-rows")?;
    Ok(())
}

fn validate_upgrade_command_options(args: &[String]) -> Result<(), String> {
    common::validate_option_values(
        args,
        &[
            "--workspace-folder",
            "--docker-path",
            "--docker-compose-path",
            "--config",
            "--log-level",
            "--feature",
            "--target-version",
        ],
    )?;
    common::validate_choice_option(args, "--log-level", &["error", "info", "debug", "trace"])?;
    validate_upgrade_options(args)
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

fn generate_lockfile(
    configuration: &Value,
    workspace_folder: Option<&Path>,
) -> Result<Lockfile, String> {
    let features = configuration
        .get("features")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let mut resolved = std::collections::BTreeMap::new();
    for feature_id in features.keys() {
        let Some(reference) = parse_feature_reference(feature_id) else {
            continue;
        };

        let (lockfile_key, entry) = generate_lockfile_entry(&reference, workspace_folder)
            .ok_or_else(|| {
                format!("Unsupported feature for native lockfile generation: {feature_id}")
            })?;
        resolved.insert(lockfile_key, entry);
    }

    Ok(Lockfile { features: resolved })
}

fn generate_lockfile_entry(
    feature: &FeatureReference,
    workspace_folder: Option<&Path>,
) -> Option<(String, LockfileEntry)> {
    if feature.digest.is_some() {
        return exact_catalog_entry(&feature.original, workspace_folder).map(|entry| {
            (
                feature.original.clone(),
                LockfileEntry {
                    version: entry.version.clone(),
                    resolved: entry.resolved.clone(),
                    integrity: entry.integrity.clone(),
                    depends_on: entry.depends_on.clone(),
                },
            )
        });
    }

    let version = if let Some(tag) = feature.tag.as_deref() {
        if tag == "latest" {
            latest_version(&feature.base, workspace_folder)?
        } else if tag.matches('.').count() == 2 {
            tag.to_string()
        } else {
            resolve_wanted_version(feature, None, workspace_folder)?
        }
    } else {
        latest_version(&feature.base, workspace_folder)?
    };

    let entry = catalog_entry_for_version(&feature.base, &version, workspace_folder)?;
    Some((
        feature.original.clone(),
        LockfileEntry {
            version,
            resolved: entry.resolved.clone(),
            integrity: entry.integrity.clone(),
            depends_on: entry.depends_on.clone(),
        },
    ))
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
            .map_err(|error| error.to_string()),
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
        fs::write(config_path, updated).map_err(|error| error.to_string())?;
    } else if let Some(logger) = logger {
        logger.trace(format!(
            "No changes to config file: {}",
            config_path.display()
        ));
    }
    Ok(())
}

fn render_outdated_text(payload: &Value) -> String {
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
    let columns = common::parse_option_value(args, "--terminal-columns")?
        .parse::<usize>()
        .ok()?;
    let rows = common::parse_option_value(args, "--terminal-rows")?
        .parse::<usize>()
        .ok()?;
    Some(TerminalDimensions { columns, rows })
}

pub(super) fn parse_feature_reference(feature_id: &str) -> Option<FeatureReference> {
    if !feature_id.starts_with("ghcr.io/")
        && !feature_id.starts_with("https://")
        && !feature_id.starts_with("http://")
    {
        return None;
    }

    let base = feature_id_without_version(feature_id);
    let suffix = feature_id.strip_prefix(&base)?;
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
