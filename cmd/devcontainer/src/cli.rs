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
    lines: Vec<HelpLine>,
    options: Vec<CommandOption>,
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

impl CommandOption {
    fn takes_value(&self) -> bool {
        self.description
            .as_deref()
            .is_some_and(|description| !description.contains("[boolean]"))
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
    cli_metadata().commands.iter().find(|command| {
        command.path.starts_with(parent_path)
            && command.token_path.len() == expected_length
            && command
                .token_path
                .last()
                .is_some_and(|token| token == child_token)
    })
}

pub fn print_help() {
    render_lines(&cli_metadata().root.lines, &[], &[]);
}

pub fn print_command_help(path: &str) {
    let Some(command) = command_help(path) else {
        println!("devcontainer {path}");
        return;
    };
    render_lines(
        &command.lines,
        &command.unsupported_options,
        &command.unsupported_positionals,
    );
}

fn render_lines(
    lines: &[HelpLine],
    unsupported_options: &[String],
    unsupported_positionals: &[String],
) {
    for line in lines {
        if line
            .option_names
            .iter()
            .any(|name| unsupported_options.contains(name))
            || line
                .positional_names
                .iter()
                .any(|name| unsupported_positionals.contains(name))
        {
            println!("{}{}", line.text, UNSUPPORTED_MARKER);
        } else {
            println!("{}", line.text);
        }
    }
}

pub fn parse_log_format(args: &[String]) -> (&str, usize) {
    if args.len() >= 3 && args[0] == "--log-format" {
        return (args[1].as_str(), 2);
    }
    ("text", 0)
}

pub fn emit_log(log_format: &str, message: &str) {
    let format = match log_format {
        "json" => LogFormat::Json,
        _ => LogFormat::Text,
    };
    println!(
        "{}",
        output::render_log(format, CommandLogLevel::Info, message)
    );
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
        let flag = arg.split_once('=').map_or(arg.as_str(), |(name, _)| name);
        let short_alias = arg
            .strip_prefix('-')
            .filter(|value| !value.starts_with('-'));
        let option = command.options.iter().find(|option| {
            flag.strip_prefix("--")
                .is_some_and(|name| name == option.name)
                || short_alias
                    .is_some_and(|alias| option.aliases.iter().any(|candidate| candidate == alias))
        });
        if let Some(option) = option.filter(|_| !arg.contains('=')) {
            if short_alias
                .is_some_and(|alias| option.aliases.iter().any(|candidate| candidate == alias))
            {
                normalized.push(format!("--{}", option.name));
            } else {
                normalized.push(arg.clone());
            }
        } else {
            normalized.push(arg.clone());
        }
        if option.is_some_and(CommandOption::takes_value)
            && !arg.contains('=')
            && args.get(index + 1).is_some_and(|value| value != "--")
        {
            index += 1;
            normalized.push(args[index].clone());
        }
        index += 1;
    }
    normalized
}

pub fn unsupported_argument_error(command_path: &str, args: &[String]) -> Option<String> {
    let command = command_help(command_path)?;
    let mut unsupported_flags = Vec::new();

    for option in &command.options {
        if command.unsupported_options.contains(&option.name) {
            unsupported_flags.push((format!("--{}", option.name), option.name.as_str()));
            for alias in &option.aliases {
                unsupported_flags.push((format!("-{alias}"), option.name.as_str()));
            }
        }
    }

    for arg in args {
        if arg == "--" {
            break;
        }

        if command.path == "exec" && !arg.starts_with('-') {
            break;
        }

        let flag = arg.split_once('=').map_or(arg.as_str(), |(name, _)| name);
        if let Some((matched_flag, _)) = unsupported_flags
            .iter()
            .find(|(candidate, _)| candidate == flag)
        {
            return Some(format!(
                "Option {matched_flag} {UNSUPPORTED_ARGUMENT_MESSAGE}: devcontainer {command_path}"
            ));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::{
        command_help, is_command_help_request, is_command_version_request,
        normalize_option_aliases, resolve_command_help, unsupported_argument_error,
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
    fn supported_command_options_are_not_reported_as_unsupported() {
        let error = unsupported_argument_error(
            "outdated",
            &["--log-level".to_string(), "trace".to_string()],
        );

        assert!(error.is_none());
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
        let command = command_help("features test").expect("features test metadata");
        assert!(command
            .lines
            .iter()
            .any(|line| line.positional_names.contains(&"target".to_string())));
    }
}
