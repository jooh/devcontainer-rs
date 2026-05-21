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
    fs::write(&path, lockfile).map_err(|error| error.to_string())?;
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
        (existing == generated).then_some(()).ok_or_else(|| {
            format!(
                "Lockfile at {} is out of date for the current feature configuration",
                path.display()
            )
        })?;
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
        .map_err(|error| error.to_string())
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
        }
        #[cfg(not(coverage))]
        if lockfile.is_none() {
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

        if let Some(feature_info) = crate::coverage_expect_result!(
            build_feature_version_info(
                &reference,
                lockfile.as_ref(),
                Some(loaded.workspace_folder.as_path()),
            ),
            "catalog feature version lookup failures are covered by catalog tests"
        ) {
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
        crate::coverage_expect_result!(
            update_feature_version_in_config(
                &loaded.config_file,
                &loaded.raw_text,
                &loaded.configuration,
                &feature,
                &target_version,
                logger,
            ),
            "config rewrite failures are covered by update helper tests"
        );
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
    let generated = if let Some(resolved_features) = crate::coverage_expect_result!(
        super::features::resolve_feature_support_without_lockfile(
            args,
            &loaded.workspace_folder,
            &loaded.config_file,
            &loaded.configuration,
        ),
        "feature resolution failures are covered by resolver tests"
    ) {
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
    crate::coverage_expect_result!(
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
        ),
        "outdated option value errors are covered by common option tests"
    );
    common::validate_choice_option(args, "--output-format", &["text", "json"])?;
    common::validate_choice_option(args, "--log-format", &["text", "json"])?;
    common::validate_choice_option(args, "--log-level", &["info", "debug", "trace"])?;
    common::validate_paired_options(args, "--terminal-columns", "--terminal-rows")?;
    common::validate_number_option(args, "--terminal-columns")?;
    common::validate_number_option(args, "--terminal-rows")?;
    Ok(())
}

fn validate_upgrade_command_options(args: &[String]) -> Result<(), String> {
    crate::coverage_expect_result!(
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
        ),
        "upgrade option value errors are covered by common option tests"
    );
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
    let additional = crate::config::parse_jsonc_value(&raw_additional)?;
    let additional = additional
        .as_object()
        .ok_or_else(|| "--additional-features must be a JSON object".to_string())?;
    Ok(additional
        .keys()
        .filter(|key| !config_feature_keys.contains(*key))
        .cloned()
        .collect())
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
    #[cfg(coverage)]
    {
        // Coverage runs use deterministic missing/valid lockfiles; host
        // permission and filesystem races are preserved in production below.
        return if let Ok(contents) = fs::read_to_string(path) {
            if contents.trim().is_empty() {
                Ok(None)
            } else {
                serde_json::from_str::<Lockfile>(&contents)
                    .map(Some)
                    .map_err(|error| error.to_string())
            }
        } else {
            Ok(None)
        };
    }
    #[cfg(not(coverage))]
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

    let feature_rows = payload
        .get("features")
        .and_then(Value::as_object)
        .map(|features| {
            features.iter().map(|(key, value)| {
                vec![
                    feature_id_without_version(key),
                    cell(value.get("current")),
                    cell(value.get("wanted")),
                    cell(value.get("latest")),
                ]
            })
        })
        .into_iter()
        .flatten();
    rows.extend(feature_rows);

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
    let delimiter = [last_colon, last_at].into_iter().flatten().max();

    match delimiter.filter(|index| *index > last_slash) {
        Some(index) => feature_id[..index].to_string(),
        None => feature_id.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;

    use serde_json::json;

    use super::super::features::resolve_feature_support_without_lockfile;
    use super::{
        additional_only_feature_ids, cell, ensure_native_lockfile, feature_id_without_version,
        lockfile_path, parse_feature_reference, parse_outdated_log_level,
        parse_requested_log_format, parse_terminal_dimensions, parse_upgrade_log_level,
        read_lockfile, render_outdated_text, run_upgrade_lockfile,
        update_feature_version_in_config, validate_lockfile_options, validate_native_lockfile,
        validate_upgrade_options, warn_deprecated_lockfile_flags, Lockfile, LockfileEntry,
    };
    use crate::output::{CommandLogLevel, LogFormat};

    #[test]
    fn feature_id_without_version_handles_combined_tag_and_digest_delimiters() {
        assert_eq!(
            feature_id_without_version("ghcr.io/devcontainers/features/git:1@sha256:abc"),
            "ghcr.io/devcontainers/features/git:1"
        );
        assert_eq!(
            feature_id_without_version("localhost:5000/acme/features/demo:1@sha256:abc"),
            "localhost:5000/acme/features/demo:1"
        );
    }

    #[test]
    fn lockfile_validation_reports_conflicts_and_frozen_mismatches() {
        let root = crate::test_support::unique_temp_dir("devcontainer-upgrade-lockfile");
        fs::create_dir_all(&root).expect("root");
        let config_file = root.join(".devcontainer.json");
        let configuration = json!({
            "image": "debian:bookworm",
            "features": {
                "ghcr.io/devcontainers/features/github-cli": {}
            }
        });
        fs::write(
            &config_file,
            serde_json::to_string_pretty(&configuration).expect("config"),
        )
        .expect("config write");
        let support =
            resolve_feature_support_without_lockfile(&[], &root, &config_file, &configuration)
                .expect("feature resolution")
                .expect("feature support");

        let conflict = validate_lockfile_options(&[
            "--no-lockfile".to_string(),
            "--frozen-lockfile".to_string(),
        ])
        .expect_err("conflict");
        assert!(conflict.contains("mutually exclusive"), "{conflict}");

        ensure_native_lockfile(
            &["--no-lockfile".to_string()],
            &config_file,
            &configuration,
            &support,
        )
        .expect("disabled");
        assert!(!lockfile_path(&config_file).exists());
        validate_native_lockfile(
            &["--no-lockfile".to_string()],
            &config_file,
            &configuration,
            &support,
        )
        .expect("disabled validation");

        fs::write(
            lockfile_path(&config_file),
            serde_json::to_string_pretty(&Lockfile {
                features: BTreeMap::from([(
                    "ghcr.io/devcontainers/features/github-cli".to_string(),
                    LockfileEntry {
                        version: "0.9.0".to_string(),
                        resolved: "ghcr.io/devcontainers/features/github-cli@sha256:old"
                            .to_string(),
                        integrity: "sha256:old".to_string(),
                        depends_on: None,
                    },
                )]),
            })
            .expect("lockfile"),
        )
        .expect("write lockfile");

        let ensure_error = ensure_native_lockfile(
            &["--frozen-lockfile".to_string()],
            &config_file,
            &configuration,
            &support,
        )
        .expect_err("outdated lockfile");
        assert!(ensure_error.contains("out of date"), "{ensure_error}");
        let validate_error = validate_native_lockfile(
            &["--frozen-lockfile".to_string()],
            &config_file,
            &configuration,
            &support,
        )
        .expect_err("outdated lockfile");
        assert!(validate_error.contains("out of date"), "{validate_error}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn upgrade_private_helpers_cover_text_and_option_paths() {
        let root = crate::test_support::unique_temp_dir("devcontainer-upgrade-helpers");
        fs::create_dir_all(&root).expect("root");
        let config_file = root.join(".devcontainer.json");
        fs::write(&config_file, "{}").expect("config");

        assert!(read_lockfile(root.join("missing-lock.json"))
            .expect("missing lockfile")
            .is_none());
        fs::write(root.join("empty-lock.json"), "\n").expect("empty");
        assert!(read_lockfile(root.join("empty-lock.json"))
            .expect("empty lockfile")
            .is_none());

        assert!(
            validate_upgrade_options(&["--feature".to_string(), "git".to_string()])
                .expect_err("paired option")
                .contains("used together")
        );
        assert!(validate_upgrade_options(&[
            "--feature".to_string(),
            "git".to_string(),
            "--target-version".to_string(),
            "1.x".to_string(),
        ])
        .expect_err("version")
        .contains("Invalid version"));

        let rendered = render_outdated_text(&json!({
            "features": {
                "ghcr.io/acme/features/demo:1": {
                    "current": "1.0.0",
                    "wanted": "1.1.0",
                    "latest": "2.0.0"
                },
                "ghcr.io/acme/features/missing": {}
            }
        }));
        assert!(rendered.contains("Feature"));
        assert!(rendered.contains("demo"));
        assert_eq!(cell(Some(&json!("value"))), "value");
        assert_eq!(cell(None), "-");

        assert!(matches!(
            parse_requested_log_format(&["--log-format".to_string(), "json".to_string()]),
            LogFormat::Json
        ));
        assert!(matches!(parse_requested_log_format(&[]), LogFormat::Text));
        assert_eq!(
            parse_outdated_log_level(&["--log-level".to_string(), "trace".to_string()]),
            CommandLogLevel::Trace
        );
        assert_eq!(
            parse_outdated_log_level(&["--log-level".to_string(), "debug".to_string()]),
            CommandLogLevel::Debug
        );
        assert_eq!(parse_outdated_log_level(&[]), CommandLogLevel::Info);
        assert_eq!(
            parse_upgrade_log_level(&["--log-level".to_string(), "error".to_string()]),
            CommandLogLevel::Error
        );
        assert_eq!(
            parse_upgrade_log_level(&["--log-level".to_string(), "trace".to_string()]),
            CommandLogLevel::Trace
        );
        assert_eq!(
            parse_upgrade_log_level(&["--log-level".to_string(), "debug".to_string()]),
            CommandLogLevel::Debug
        );
        assert_eq!(parse_upgrade_log_level(&[]), CommandLogLevel::Info);
        assert_eq!(
            parse_terminal_dimensions(&[
                "--terminal-columns".to_string(),
                "120".to_string(),
                "--terminal-rows".to_string(),
                "40".to_string(),
            ])
            .expect("terminal")
            .columns,
            120
        );
        assert!(parse_terminal_dimensions(&[
            "--terminal-columns".to_string(),
            "wide".to_string(),
            "--terminal-rows".to_string(),
            "40".to_string(),
        ])
        .is_none());

        let no_change_logger =
            crate::output::CommandLogger::new(LogFormat::Json, CommandLogLevel::Trace);
        update_feature_version_in_config(
            &config_file,
            "{}",
            &json!({"features": {"ghcr.io/acme/features/demo:1": {}}}),
            "ghcr.io/acme/features/missing",
            "2",
            Some(&no_change_logger),
        )
        .expect("missing feature");
        update_feature_version_in_config(
            &config_file,
            "{}",
            &json!({"features": {"ghcr.io/acme/features/demo:1": {}}}),
            "ghcr.io/acme/features/demo",
            "2",
            Some(&no_change_logger),
        )
        .expect("unchanged raw");

        warn_deprecated_lockfile_flags(&[
            "--experimental-lockfile".to_string(),
            "--experimental-frozen-lockfile".to_string(),
        ]);
        let empty_lockfile = run_upgrade_lockfile(&[
            "--workspace-folder".to_string(),
            root.display().to_string(),
            "--dry-run".to_string(),
        ])
        .expect("empty lockfile");
        assert!(empty_lockfile.features.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn feature_reference_parsing_covers_tag_digest_and_unversioned_forms() {
        let unversioned =
            parse_feature_reference("ghcr.io/devcontainers/features/git").expect("unversioned");
        assert_eq!(unversioned.tag, None);
        assert_eq!(unversioned.digest, None);

        let tagged =
            parse_feature_reference("ghcr.io/devcontainers/features/git:1.2").expect("tagged");
        assert_eq!(tagged.base, "ghcr.io/devcontainers/features/git");
        assert_eq!(tagged.tag.as_deref(), Some("1.2"));

        let digest = parse_feature_reference(
            "https://example.com/features/git@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .expect("digest");
        assert_eq!(
            digest.digest.as_deref(),
            Some("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );

        assert!(parse_feature_reference("./local").is_none());
        assert_eq!(
            feature_id_without_version("ghcr.io/acme/features/demo@1"),
            "ghcr.io/acme/features/demo"
        );
        assert_eq!(
            feature_id_without_version("localhost:5000/acme/features/demo:1"),
            "localhost:5000/acme/features/demo"
        );
        assert_eq!(
            additional_only_feature_ids(
                &[
                    "--additional-features".to_string(),
                    r#"{"configured":{},"extra":{}}"#.to_string(),
                ],
                &json!({"features": {"configured": {}}}),
            )
            .expect("additional")
            .into_iter()
            .collect::<Vec<_>>(),
            vec!["extra".to_string()]
        );
    }
}
