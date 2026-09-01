//! Command-line parsing and top-level dispatch for the devcontainer binary.

use std::sync::OnceLock;

use serde::Deserialize;

use crate::output::{self, CommandLogLevel, LogFormat};

const CLI_METADATA_JSON: &str = include_str!("cli_metadata.json");
const UNSUPPORTED_MARKER: &str = "  [not yet implemented in native Rust CLI]";
const UNSUPPORTED_ARGUMENT_MESSAGE: &str =
    "is recognized for this command but is not yet implemented in the native Rust CLI";

pub const SUPPORTED_TOP_LEVEL_COMMANDS: [&str; 10] = [
    "read-configuration",
    "build",
    "up",
    "set-up",
    "run-user-commands",
    "outdated",
    "upgrade",
    "exec",
    "features",
    "templates",
];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CliMetadata {
    root: HelpPage,
    commands: Vec<CommandHelp>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HelpPage {
    lines: Vec<HelpLine>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HelpLine {
    text: String,
    option_names: Vec<String>,
    positional_names: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommandHelp {
    path: String,
    token_path: Vec<String>,
    subcommands: Vec<String>,
    lines: Vec<HelpLine>,
    options: Vec<CommandOption>,
    positionals: Vec<CommandPositional>,
    unsupported_options: Vec<String>,
    unsupported_positionals: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommandOption {
    name: String,
    aliases: Vec<String>,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CommandPositional {
    name: String,
    description: String,
}

impl CommandOption {
    fn takes_value(&self) -> bool {
        match self.description.as_deref() {
            Some(description) => !description.contains("[boolean]"),
            None => false,
        }
    }

    fn accepts_explicit_boolean_value(&self, value: Option<&String>) -> bool {
        self.description
            .as_deref()
            .is_none_or(|description| description.contains("[boolean]"))
            && value.is_some_and(|value| {
                matches!(
                    value.as_str(),
                    "false" | "0" | "no" | "off" | "true" | "1" | "yes" | "on"
                )
            })
    }

    fn takes_multiple_values(&self) -> bool {
        self.name != "allow-cross-origin-auth-host"
            && self
                .description
                .as_deref()
                .is_some_and(|description| description.contains("[array]"))
    }
}

impl CommandPositional {
    fn takes_multiple_values(&self) -> bool {
        self.description.contains("[array]")
    }
}

pub struct ResolvedCommandHelp<'a> {
    pub path: &'a str,
    pub consumed_args: usize,
}

fn cli_metadata() -> &'static CliMetadata {
    static CLI_METADATA: OnceLock<CliMetadata> = OnceLock::new();
    CLI_METADATA.get_or_init(|| {
        serde_json::from_str(CLI_METADATA_JSON).expect("valid generated CLI metadata")
    })
}

fn command_help(path: &str) -> Option<&'static CommandHelp> {
    cli_metadata()
        .commands
        .iter()
        .find(|command| command.path == path)
}

fn child_command(parent_path: &str, child_token: &str) -> Option<&'static CommandHelp> {
    let expected_length = parent_path.split(' ').count() + 1;
    for command in &cli_metadata().commands {
        if !command.path.starts_with(parent_path) {
            continue;
        }
        if command.token_path.len() != expected_length {
            continue;
        }
        if command.token_path.last().map(String::as_str) == Some(child_token) {
            return Some(command);
        }
    }
    None
}

pub fn print_help() {
    render_lines(&cli_metadata().root.lines, &[], &[]);
}

pub fn print_command_help(path: &str) {
    println!("{}", command_help_text(path));
}

fn command_help_text(path: &str) -> String {
    let Some(command) = command_help(path) else {
        return format!("devcontainer {path}");
    };
    rendered_lines(
        &command.lines,
        &command.unsupported_options,
        &command.unsupported_positionals,
    )
}

fn render_lines(
    lines: &[HelpLine],
    unsupported_options: &[String],
    unsupported_positionals: &[String],
) {
    println!(
        "{}",
        rendered_lines(lines, unsupported_options, unsupported_positionals)
    );
}

fn rendered_lines(
    lines: &[HelpLine],
    unsupported_options: &[String],
    unsupported_positionals: &[String],
) -> String {
    let mut rendered = Vec::with_capacity(lines.len());
    for line in lines {
        if line_has_unsupported_entry(line, unsupported_options, unsupported_positionals) {
            rendered.push(format!("{}{}", line.text, UNSUPPORTED_MARKER));
        } else {
            rendered.push(line.text.clone());
        }
    }
    rendered.join("\n")
}

fn line_has_unsupported_entry(
    line: &HelpLine,
    unsupported_options: &[String],
    unsupported_positionals: &[String],
) -> bool {
    for name in &line.option_names {
        if unsupported_options.contains(name) {
            return true;
        }
    }
    for name in &line.positional_names {
        if unsupported_positionals.contains(name) {
            return true;
        }
    }
    false
}

pub fn parse_log_format(args: &[String]) -> (&str, usize) {
    if args.len() >= 2 && args[0] == "--log-format" {
        return (args[1].as_str(), 2);
    }
    ("text", 0)
}

pub(crate) fn parse_leading_oci_auth_options(
    args: &[String],
) -> Result<(Vec<String>, usize), String> {
    let mut normalized = Vec::new();
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        if let Some(value) = arg.strip_prefix("--oci-auth-hardening=") {
            normalized.push("--oci-auth-hardening".to_string());
            normalized.push(value.to_string());
            index += 1;
            continue;
        }
        if arg == "--oci-auth-hardening" {
            normalized.push(arg.clone());
            if args.get(index + 1).is_some_and(|value| {
                matches!(
                    value.as_str(),
                    "false" | "0" | "no" | "off" | "true" | "1" | "yes" | "on"
                )
            }) {
                index += 1;
                normalized.push(args[index].clone());
            }
            index += 1;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--allow-cross-origin-auth-host=") {
            if value.is_empty() {
                return Err("Missing value for option: --allow-cross-origin-auth-host".to_string());
            }
            normalized.push("--allow-cross-origin-auth-host".to_string());
            normalized.push(value.to_string());
            index += 1;
            continue;
        }
        if arg == "--allow-cross-origin-auth-host" {
            let Some(value) = args.get(index + 1).filter(|value| !value.starts_with('-')) else {
                return Err("Missing value for option: --allow-cross-origin-auth-host".to_string());
            };
            normalized.push(arg.clone());
            normalized.push(value.clone());
            index += 2;
            continue;
        }
        break;
    }
    Ok((normalized, index))
}

pub fn emit_log(log_format: &str, message: &str) {
    println!("{}", rendered_cli_log(log_format, message));
}

fn rendered_cli_log(log_format: &str, message: &str) -> String {
    let format = match log_format {
        "json" => LogFormat::Json,
        _ => LogFormat::Text,
    };
    output::render_log(format, CommandLogLevel::Info, message)
}

pub fn is_command_help_request(args: &[String]) -> bool {
    matches!(
        args.first().map(String::as_str),
        Some("--help") | Some("-h")
    )
}

pub fn is_command_version_request(args: &[String]) -> bool {
    matches!(args.first().map(String::as_str), Some("--version"))
}

pub fn resolve_command_help<'a>(
    command: &'a str,
    args: &[String],
) -> Option<ResolvedCommandHelp<'a>> {
    let mut current = command_help(command)?;
    let mut consumed_args = 0;

    while let Some(next_arg) = args.get(consumed_args) {
        let Some(child) = child_command(&current.path, next_arg) else {
            break;
        };
        current = child;
        consumed_args += 1;
    }

    Some(ResolvedCommandHelp {
        path: &current.path,
        consumed_args,
    })
}

pub(crate) fn normalize_option_aliases(command_path: &str, args: &[String]) -> Vec<String> {
    let Some(command) = command_help(command_path) else {
        return args.to_vec();
    };
    let mut normalized = Vec::with_capacity(args.len());
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" || (command.path == "exec" && !arg.starts_with('-')) {
            normalized.extend_from_slice(&args[index..]);
            break;
        }
        let (flag, inline_value) = match arg.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (arg.as_str(), None),
        };
        let short_alias = match flag.strip_prefix('-') {
            Some(value) if !value.starts_with('-') => Some(value),
            _ => None,
        };
        let option = find_command_option(command, flag, short_alias);
        if let Some(option) = option {
            normalized.push(format!("--{}", option.name));
            if let Some(value) = inline_value {
                normalized.push(value.to_string());
                index += 1;
                continue;
            }
        } else {
            normalized.push(arg.clone());
        }
        let next_arg_is_value = match args.get(index + 1) {
            Some(value) => value != "--",
            None => false,
        };
        let option_takes_value = match option {
            Some(option) => option.takes_value(),
            None => false,
        };
        if option_takes_value && next_arg_is_value {
            index += 1;
            normalized.push(args[index].clone());
        }
        index += 1;
    }
    normalized
}

fn find_command_option<'a>(
    command: &'a CommandHelp,
    flag: &str,
    short_alias: Option<&str>,
) -> Option<&'a CommandOption> {
    command
        .options
        .iter()
        .find(|option| option_matches_arg(option, flag, short_alias))
}

fn option_matches_arg(option: &CommandOption, flag: &str, short_alias: Option<&str>) -> bool {
    if flag.strip_prefix("--") == Some(option.name.as_str()) {
        return true;
    }
    match short_alias {
        Some(alias) => option_has_alias(option, alias),
        None => false,
    }
}

fn option_has_alias(option: &CommandOption, alias: &str) -> bool {
    for candidate in &option.aliases {
        if candidate == alias {
            return true;
        }
    }
    false
}

pub(crate) fn command_positionals(command_path: &str, args: &[String]) -> Vec<String> {
    let Some(command) = command_help(command_path) else {
        return Vec::new();
    };
    let mut positionals = Vec::new();
    let mut options_ended = false;
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        if !options_ended && arg == "--" {
            options_ended = true;
            index += 1;
            continue;
        }
        if options_ended || !arg.starts_with('-') {
            positionals.push(arg.clone());
            index += 1;
            continue;
        }

        let flag = arg.split_once('=').map_or(arg.as_str(), |(name, _)| name);
        let short_alias = match flag.strip_prefix('-') {
            Some(alias) if !alias.starts_with('-') => Some(alias),
            _ => None,
        };
        let Some(option) = find_command_option(command, flag, short_alias) else {
            index += 1;
            continue;
        };
        if arg.contains('=') {
            index += 1;
        } else if option.takes_multiple_values() {
            index += 1;
            while args
                .get(index)
                .is_some_and(|value| value != "--" && !value.starts_with('-'))
            {
                index += 1;
            }
        } else if args.get(index + 1).is_some_and(|value| {
            value != "--"
                && (option.takes_value() || option.accepts_explicit_boolean_value(Some(value)))
        }) {
            index += 2;
        } else {
            index += 1;
        }
    }
    positionals
}

pub fn unsupported_argument_error(command_path: &str, args: &[String]) -> Option<String> {
    let command = command_help(command_path)?;

    // Dispatch retains resolved nested command tokens (for example `test` in
    // `features test`) in its argument vector. They identify the command and
    // are not positionals declared by the leaf command's metadata.
    let nested_tokens = &command.token_path[1..];
    let command_args = match args.strip_prefix(nested_tokens) {
        Some(command_args) => command_args,
        None => args,
    };

    unsupported_argument_error_for(command, command_path, command_args)
}

fn unsupported_argument_error_for(
    command: &CommandHelp,
    command_path: &str,
    args: &[String],
) -> Option<String> {
    let mut index = 0;
    let mut positional_index = 0;
    let mut options_ended = false;
    while let Some(arg) = args.get(index) {
        if !options_ended && arg == "--" {
            if command.path == "exec" {
                break;
            }
            options_ended = true;
            index += 1;
            continue;
        }

        if !options_ended && command.path == "exec" && !arg.starts_with('-') {
            break;
        }

        if options_ended || !arg.starts_with('-') {
            // Group commands still need to pass unknown tokens through to the
            // dispatcher so it can report an unsupported subcommand. Leaf
            // commands, however, accept only the positionals in their
            // generated upstream metadata.
            if command.subcommands.is_empty() {
                let Some(positional) = command.positionals.get(positional_index) else {
                    return Some(format!(
                        "Unknown argument: {arg}: devcontainer {command_path}"
                    ));
                };
                if command.unsupported_positionals.contains(&positional.name) {
                    return Some(format!(
                        "Argument {arg} {UNSUPPORTED_ARGUMENT_MESSAGE}: devcontainer {command_path}"
                    ));
                }
                if !positional.takes_multiple_values() {
                    positional_index += 1;
                }
            }
            index += 1;
            continue;
        }

        let flag = match arg.split_once('=') {
            Some((name, _)) => name,
            None => arg.as_str(),
        };
        let short_alias = match flag.strip_prefix('-') {
            Some(alias) if !alias.starts_with('-') => Some(alias),
            _ => None,
        };
        let Some(option) = find_command_option(command, flag, short_alias) else {
            if command_path == "up" && flag == "--pull" {
                return Some(
                    "Option --pull is not supported by devcontainer up. Use --pull-always instead."
                        .to_string(),
                );
            }
            return Some(format!(
                "Unknown option: {flag}: devcontainer {command_path}"
            ));
        };
        if command.unsupported_options.contains(&option.name) {
            return Some(format!(
                "Option {flag} {UNSUPPORTED_ARGUMENT_MESSAGE}: devcontainer {command_path}"
            ));
        }

        let next = args.get(index + 1);
        if !arg.contains('=') && option.takes_multiple_values() {
            index += 1;
            while args
                .get(index)
                .is_some_and(|value| value != "--" && !value.starts_with('-'))
            {
                index += 1;
            }
        } else if !arg.contains('=')
            && next.is_some_and(|value| value != "--")
            && (option.takes_value() || option.accepts_explicit_boolean_value(next))
        {
            index += 2;
        } else {
            index += 1;
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::{
        cli_metadata, command_help, command_help_text, command_positionals,
        is_command_help_request, is_command_version_request, normalize_option_aliases,
        rendered_cli_log, rendered_lines, resolve_command_help, unsupported_argument_error,
        unsupported_argument_error_for, CommandHelp, CommandOption, CommandPositional, HelpLine,
    };

    #[test]
    fn detects_subcommand_help_requests() {
        assert!(is_command_help_request(&["--help".to_string()]));
        assert!(is_command_help_request(&["-h".to_string()]));
        assert!(!is_command_help_request(&["list".to_string()]));
    }

    #[test]
    fn detects_subcommand_version_requests() {
        assert!(is_command_version_request(&["--version".to_string()]));
        assert!(!is_command_version_request(&["-V".to_string()]));
        assert!(!is_command_version_request(&["version".to_string()]));
    }

    #[test]
    fn resolves_nested_help_paths() {
        let resolved =
            resolve_command_help("templates", &["apply".to_string(), "--help".to_string()])
                .expect("resolved help");

        assert_eq!(resolved.path, "templates apply");
        assert_eq!(resolved.consumed_args, 1);
    }

    #[test]
    fn unknown_help_paths_do_not_resolve() {
        assert!(resolve_command_help("unknown", &[]).is_none());
    }

    #[test]
    fn normalizes_command_scoped_short_option_aliases() {
        let normalized = normalize_option_aliases(
            "templates apply",
            &[
                "-w".to_string(),
                "/tmp/workspace".to_string(),
                "-t".to_string(),
                "ghcr.io/devcontainers/templates/docker-from-docker:latest".to_string(),
                "-a".to_string(),
                "{}".to_string(),
                "-f".to_string(),
                "[]".to_string(),
            ],
        );

        assert_eq!(
            normalized,
            vec![
                "--workspace-folder".to_string(),
                "/tmp/workspace".to_string(),
                "--template-id".to_string(),
                "ghcr.io/devcontainers/templates/docker-from-docker:latest".to_string(),
                "--template-args".to_string(),
                "{}".to_string(),
                "--features".to_string(),
                "[]".to_string(),
            ]
        );
    }

    #[test]
    fn normalizes_inline_long_and_short_option_values() {
        let normalized = normalize_option_aliases(
            "templates apply",
            &[
                "--workspace-folder=/tmp/workspace".to_string(),
                "--template-id=ghcr.io/devcontainers/templates/docker-from-docker:latest"
                    .to_string(),
            ],
        );

        assert_eq!(
            normalized,
            vec![
                "--workspace-folder".to_string(),
                "/tmp/workspace".to_string(),
                "--template-id".to_string(),
                "ghcr.io/devcontainers/templates/docker-from-docker:latest".to_string(),
            ]
        );

        let normalized = normalize_option_aliases(
            "upgrade",
            &[
                "-f=ghcr.io/example/features/demo".to_string(),
                "-v=2".to_string(),
            ],
        );
        assert_eq!(
            normalized,
            vec![
                "--feature".to_string(),
                "ghcr.io/example/features/demo".to_string(),
                "--target-version".to_string(),
                "2".to_string(),
            ]
        );
    }

    #[test]
    fn extracts_positionals_across_option_and_separator_forms() {
        assert!(command_positionals("unknown", &["target".to_string()]).is_empty());
        assert_eq!(
            command_positionals(
                "templates apply",
                &[
                    "--workspace-folder=/workspace".to_string(),
                    "--unknown".to_string(),
                    "--".to_string(),
                    "--target".to_string(),
                ],
            ),
            vec!["--target"]
        );

        assert_eq!(
            command_positionals(
                "features info",
                &[
                    "--allow-cross-origin-auth-host".to_string(),
                    "registry.example=auth.example".to_string(),
                    "manifest".to_string(),
                    "registry.example/features/demo".to_string(),
                ],
            ),
            vec!["manifest", "registry.example/features/demo"]
        );
    }

    #[test]
    fn unknown_command_paths_preserve_arguments_without_alias_normalization() {
        let normalized =
            normalize_option_aliases("unknown", &["-w".to_string(), "/tmp/workspace".to_string()]);

        assert_eq!(
            normalized,
            vec!["-w".to_string(), "/tmp/workspace".to_string()]
        );
    }

    #[test]
    fn bare_double_dash_stops_alias_normalization() {
        let normalized = normalize_option_aliases(
            "features test",
            &[
                "--".to_string(),
                "-q".to_string(),
                "--filter".to_string(),
                "scenario".to_string(),
            ],
        );

        assert_eq!(
            normalized,
            vec![
                "--".to_string(),
                "-q".to_string(),
                "--filter".to_string(),
                "scenario".to_string()
            ]
        );
    }

    #[test]
    fn preserves_aliases_outside_the_resolved_command_scope() {
        let normalized = normalize_option_aliases(
            "templates apply",
            &["-p".to_string(), "project".to_string()],
        );

        assert_eq!(normalized, vec!["-p".to_string(), "project".to_string()]);
    }

    #[test]
    fn preserves_alias_like_values_after_options_that_take_values() {
        let normalized = normalize_option_aliases(
            "features test",
            &[
                "--project-folder".to_string(),
                ".".to_string(),
                "--filter".to_string(),
                "-p".to_string(),
                "-q".to_string(),
            ],
        );

        assert_eq!(
            normalized,
            vec![
                "--project-folder".to_string(),
                ".".to_string(),
                "--filter".to_string(),
                "-p".to_string(),
                "--quiet".to_string(),
            ]
        );

        let normalized = normalize_option_aliases(
            "features test",
            &["-p".to_string(), "-q".to_string(), "-q".to_string()],
        );

        assert_eq!(
            normalized,
            vec![
                "--project-folder".to_string(),
                "-q".to_string(),
                "--quiet".to_string(),
            ]
        );
    }

    #[test]
    fn preserves_exec_command_args_after_first_non_option() {
        let normalized = normalize_option_aliases(
            "exec",
            &[
                "--workspace-folder".to_string(),
                "/workspace".to_string(),
                "/bin/sh".to_string(),
                "-c".to_string(),
                "echo hi".to_string(),
            ],
        );

        assert_eq!(
            normalized,
            vec![
                "--workspace-folder".to_string(),
                "/workspace".to_string(),
                "/bin/sh".to_string(),
                "-c".to_string(),
                "echo hi".to_string(),
            ]
        );
    }

    #[test]
    fn metadata_tracks_no_remaining_visible_unsupported_flags() {
        let command = command_help("outdated").expect("outdated metadata");
        assert!(command.unsupported_options.is_empty());

        let upgrade = command_help("upgrade").expect("upgrade metadata");
        assert!(upgrade.unsupported_options.is_empty());
    }

    #[test]
    fn generated_command_option_names_are_unique() {
        for command in &cli_metadata().commands {
            let mut names = std::collections::HashSet::new();
            for option in &command.options {
                assert!(
                    names.insert(option.name.as_str()),
                    "duplicate option metadata for {}: {}",
                    command.path,
                    option.name
                );
            }
        }
    }

    #[test]
    fn supported_command_options_are_not_reported_as_unsupported() {
        let error = unsupported_argument_error(
            "outdated",
            &["--log-level".to_string(), "trace".to_string()],
        );

        assert!(error.is_none());
    }

    #[test]
    fn features_test_accepts_hidden_camel_case_project_folder_alias() {
        let error = unsupported_argument_error(
            "features test",
            &[
                "test".to_string(),
                "--projectFolder".to_string(),
                "/workspace".to_string(),
            ],
        );

        assert!(error.is_none(), "{error:?}");
        assert!(
            !command_help_text("features test").contains("--projectFolder"),
            "hidden alias should not be duplicated in help"
        );
    }

    #[test]
    fn set_up_accepts_native_compose_path_option() {
        let error = unsupported_argument_error(
            "set-up",
            &[
                "--docker-compose-path".to_string(),
                "podman-compose".to_string(),
            ],
        );

        assert!(error.is_none(), "{error:?}");
    }

    #[test]
    fn unsupported_argument_error_reports_synthetic_unsupported_options() {
        let command = CommandHelp {
            path: "sample".to_string(),
            token_path: vec!["sample".to_string()],
            subcommands: Vec::new(),
            lines: Vec::new(),
            options: vec![CommandOption {
                name: "legacy".to_string(),
                aliases: vec!["l".to_string()],
                description: Some("Legacy option".to_string()),
            }],
            positionals: vec![CommandPositional {
                name: "legacy-target".to_string(),
                description: "Legacy target  [string]".to_string(),
            }],
            unsupported_options: vec!["legacy".to_string()],
            unsupported_positionals: vec!["legacy-target".to_string()],
        };

        let error = unsupported_argument_error_for(&command, "sample", &["-l".to_string()])
            .expect("unsupported option");
        assert!(error.contains("Option -l"), "{error}");
        assert!(error.contains("devcontainer sample"), "{error}");

        let error =
            unsupported_argument_error_for(&command, "sample", &["--legacy=value".to_string()])
                .expect("unsupported option with value");
        assert!(error.contains("Option --legacy"), "{error}");

        let after_separator = unsupported_argument_error_for(
            &command,
            "sample",
            &["--".to_string(), "-l".to_string()],
        );
        assert_eq!(
            after_separator.as_deref(),
            Some(
                "Argument -l is recognized for this command but is not yet implemented in the native Rust CLI: devcontainer sample"
            )
        );

        let positional_error =
            unsupported_argument_error_for(&command, "sample", &["target".to_string()])
                .expect("unsupported positional");
        assert_eq!(
            positional_error,
            "Argument target is recognized for this command but is not yet implemented in the native Rust CLI: devcontainer sample"
        );
    }

    #[test]
    fn ignores_exec_command_arguments_after_first_non_option() {
        let error = unsupported_argument_error(
            "exec",
            &[
                "/bin/echo".to_string(),
                "--dotfiles-target-path".to_string(),
                "/tmp/dotfiles".to_string(),
            ],
        );

        assert!(error.is_none());
    }

    #[test]
    fn preserves_positional_metadata_for_nested_commands() {
        let build = command_help("build").expect("build metadata");
        assert_eq!(build.positionals[0].name, "path");

        let command = command_help("features test").expect("features test metadata");
        assert!(command
            .lines
            .iter()
            .any(|line| line.positional_names.contains(&"target".to_string())));
        assert_eq!(command.positionals[0].name, "target");

        for path in [
            "features package",
            "features publish",
            "templates publish",
            "templates apply",
            "features generate-docs",
            "templates generate-docs",
        ] {
            let command = command_help(path).expect("nested command metadata");
            assert_eq!(command.positionals[0].name, "target", "{path}");
        }
    }

    #[test]
    fn accepts_all_values_of_array_options_before_validating_positionals() {
        let error = unsupported_argument_error(
            "features test",
            &[
                "test".to_string(),
                "--features".to_string(),
                "one".to_string(),
                "two".to_string(),
                "--quiet".to_string(),
            ],
        );

        assert!(error.is_none(), "{error:?}");
    }

    #[test]
    fn accepts_declared_positionals_and_rejects_extras() {
        assert!(unsupported_argument_error("build", &["workspace".to_string()]).is_none());

        let error =
            unsupported_argument_error("build", &["workspace".to_string(), "extra".to_string()]);
        assert_eq!(
            error.as_deref(),
            Some("Unknown argument: extra: devcontainer build")
        );

        assert!(unsupported_argument_error(
            "features info",
            &[
                "info".to_string(),
                "manifest".to_string(),
                "ghcr.io/example/features/sample:1".to_string(),
            ],
        )
        .is_none());

        assert!(unsupported_argument_error(
            "features info",
            &[
                "manifest".to_string(),
                "ghcr.io/example/features/sample:1".to_string(),
            ],
        )
        .is_none());
    }

    #[test]
    fn render_lines_marks_unsupported_entries() {
        let rendered = rendered_lines(
            &[
                HelpLine {
                    text: "  --legacy  Old option".to_string(),
                    option_names: vec!["legacy".to_string()],
                    positional_names: Vec::new(),
                },
                HelpLine {
                    text: "  target  Old positional".to_string(),
                    option_names: Vec::new(),
                    positional_names: vec!["target".to_string()],
                },
            ],
            &["legacy".to_string()],
            &["target".to_string()],
        );

        assert_eq!(
            rendered,
            "  --legacy  Old option  [not yet implemented in native Rust CLI]\n  target  Old positional  [not yet implemented in native Rust CLI]"
        );
    }

    #[test]
    fn print_unknown_command_help_falls_back_to_usage() {
        assert_eq!(
            command_help_text("unknown nested"),
            "devcontainer unknown nested"
        );
    }

    #[test]
    fn emit_log_supports_text_and_json_formats() {
        assert_eq!(rendered_cli_log("text", "plain"), "plain");

        let rendered: serde_json::Value =
            serde_json::from_str(&rendered_cli_log("json", "structured")).expect("json log");
        assert_eq!(rendered["type"], "text");
        assert_eq!(rendered["level"], 3);
        assert_eq!(rendered["text"], "structured");
        assert!(rendered["timestamp"].as_u64().is_some(), "{rendered}");
    }
}
