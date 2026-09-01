//! Compose-specific argument parsing helpers for runtime flows.

use std::path::PathBuf;

use super::ComposeSpec;

use crate::commands::common;

pub(super) fn compose_args_owned(
    spec: &ComposeSpec,
    subcommand: &str,
    override_file: Option<&PathBuf>,
    tail: Vec<String>,
) -> Vec<String> {
    let mut args = Vec::new();
    args.push("--project-name".to_string());
    args.push(spec.project_name.clone());
    for file in &spec.files {
        args.push("-f".to_string());
        args.push(file.display().to_string());
    }
    if let Some(override_file) = override_file {
        args.push("-f".to_string());
        args.push(override_file.display().to_string());
    }
    args.push(subcommand.to_string());
    args.extend(tail);
    args
}

pub(super) fn reject_unsupported_build_options(args: &[String]) -> Result<(), String> {
    if compose_build_option_is_present(args, "--cache-to") {
        return Err("--cache-to not supported for compose builds.".to_string());
    }
    if compose_build_option_is_present(args, "--platform")
        || common::parse_bool_option(args, "--push", false)
    {
        return Err("--platform or --push not supported.".to_string());
    }
    if compose_build_option_is_present(args, "--output") {
        return Err("--output not supported.".to_string());
    }
    if compose_build_option_is_present(args, "--label") {
        return Err("--label not supported for compose builds.".to_string());
    }
    Ok(())
}

fn compose_build_option_is_present(args: &[String], flag: &str) -> bool {
    common::has_flag(args, flag)
        || common::parse_option_value(args, flag).is_some()
        || !common::parse_option_values(args, flag).is_empty()
}
