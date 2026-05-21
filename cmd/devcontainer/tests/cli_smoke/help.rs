//! CLI smoke tests for help text and unsupported command surfaces.

use serde::Deserialize;

use crate::support::test_support::devcontainer_command;

const UNSUPPORTED_MARKER: &str = "  [not yet implemented in native Rust CLI]";
const CLI_METADATA_JSON: &str = include_str!("../../src/cli_metadata.json");

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
    lines: Vec<HelpLine>,
    unsupported_options: Vec<String>,
    unsupported_positionals: Vec<String>,
}

#[test]
fn top_level_help_matches_public_cli_surface() {
    let output = devcontainer_command(None)
        .arg("--help")
        .output()
        .expect("help command should run");

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(stdout.contains("devcontainer <command>"), "{stdout}");
    assert!(stdout.contains("devcontainer up"), "{stdout}");
    assert!(
        stdout.contains("devcontainer exec <cmd> [args..]"),
        "{stdout}"
    );
    assert!(stdout.contains("Options:"), "{stdout}");
    assert!(stdout.contains("--version"), "{stdout}");
    assert!(
        !stdout.contains("docs/upstream/command-reference.md"),
        "{stdout}"
    );
    assert!(!stdout.contains("parity-inventory"), "{stdout}");
    assert!(!stdout.contains("Current state:"), "{stdout}");
}

#[test]
fn up_help_lists_upstream_options() {
    let output = devcontainer_command(None)
        .args(["up", "--help"])
        .output()
        .expect("up help should run");

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(stdout.contains("devcontainer up"), "{stdout}");
    assert!(stdout.contains("--workspace-folder"), "{stdout}");
    assert!(stdout.contains("--build-no-cache"), "{stdout}");
    assert!(stdout.contains("--dotfiles-target-path"), "{stdout}");
    assert!(
        stdout.contains("--include-merged-configuration"),
        "{stdout}"
    );
    assert!(!stdout.contains("Current state:"), "{stdout}");
}

#[test]
fn features_help_matches_upstream_group_shape() {
    let output = devcontainer_command(None)
        .args(["features", "--help"])
        .output()
        .expect("features help should run");

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(stdout.contains("devcontainer features"), "{stdout}");
    assert!(
        stdout.contains("devcontainer features test [target]"),
        "{stdout}"
    );
    assert!(
        stdout.contains("devcontainer features package <target>"),
        "{stdout}"
    );
    assert!(
        stdout.contains("devcontainer features publish <target>"),
        "{stdout}"
    );
    assert!(!stdout.contains("list"), "{stdout}");
    assert!(!stdout.contains("Native support:"), "{stdout}");
}

#[test]
fn templates_apply_help_lists_nested_options() {
    let output = devcontainer_command(None)
        .args(["templates", "apply", "--help"])
        .output()
        .expect("templates apply help should run");

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(stdout.contains("devcontainer templates apply"), "{stdout}");
    assert!(stdout.contains("--template-id"), "{stdout}");
    assert!(stdout.contains("--omit-paths"), "{stdout}");
    assert!(stdout.contains("--workspace-folder"), "{stdout}");
}

#[test]
fn native_only_collection_list_commands_are_not_supported() {
    for args in [
        ["features", "list"].as_slice(),
        ["features", "ls"].as_slice(),
        ["templates", "list"].as_slice(),
        ["templates", "ls"].as_slice(),
    ] {
        let output = devcontainer_command(None)
            .args(args)
            .output()
            .expect("collection list command should run");

        assert!(!output.status.success(), "{output:?}");
    }
}

#[test]
fn only_top_level_long_version_flag_is_supported() {
    let output = devcontainer_command(None)
        .arg("--version")
        .output()
        .expect("version command should run");

    assert!(output.status.success(), "{output:?}");

    for args in [["-V"], ["version"]] {
        let output = devcontainer_command(None)
            .args(args)
            .output()
            .expect("version alias should run");

        assert!(!output.status.success(), "{output:?}");
    }
}

#[test]
fn command_scoped_unsupported_options_are_rejected_before_dispatch() {
    let output = devcontainer_command(None)
        .args(["up", "--definitely-unsupported"])
        .output()
        .expect("unsupported command option should run");

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8(output.stderr)
            .expect("utf8 stderr")
            .trim(),
        "Unknown option for up: --definitely-unsupported"
    );
}

#[test]
fn committed_help_metadata_matches_actual_native_help_output() {
    let metadata: CliMetadata = serde_json::from_str(CLI_METADATA_JSON).expect("cli metadata");

    let output = devcontainer_command(None)
        .arg("--help")
        .output()
        .expect("top-level help should run");

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        rendered_lines(&metadata.root.lines, &[], &[])
    );

    for command in &metadata.commands {
        let mut args = command
            .path
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>();
        args.push("--help".to_string());

        let output = devcontainer_command(None)
            .args(&args)
            .output()
            .unwrap_or_else(|error| panic!("help command {:?} should run: {error}", args));

        assert!(output.status.success(), "{output:?}");
        let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
        assert_eq!(
            stdout.lines().collect::<Vec<_>>(),
            rendered_lines(
                &command.lines,
                &command.unsupported_options,
                &command.unsupported_positionals,
            ),
            "help mismatch for {}",
            command.path
        );
    }
}

#[test]
fn outdated_help_no_longer_marks_log_and_terminal_flags_as_unsupported() {
    let output = devcontainer_command(None)
        .args(["outdated", "--help"])
        .output()
        .expect("outdated help should run");

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    for flag in ["--log-level", "--terminal-columns", "--terminal-rows"] {
        let line = stdout
            .lines()
            .find(|line| line.contains(flag))
            .unwrap_or_else(|| panic!("missing help line for {flag}: {stdout}"));
        assert!(!line.contains(UNSUPPORTED_MARKER), "{stdout}");
    }
}

#[test]
fn help_omits_hidden_upstream_options() {
    let output = devcontainer_command(None)
        .args(["up", "--help"])
        .output()
        .expect("up help should run");

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(!stdout.contains("--omit-syntax-directive"), "{stdout}");
}

#[test]
fn upgrade_help_no_longer_marks_log_level_as_unsupported() {
    let output = devcontainer_command(None)
        .args(["upgrade", "--help"])
        .output()
        .expect("upgrade help should run");

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    let line = stdout
        .lines()
        .find(|line| line.contains("--log-level"))
        .expect("log-level help line");
    assert!(!line.contains(UNSUPPORTED_MARKER), "{stdout}");
}

fn rendered_lines(
    lines: &[HelpLine],
    unsupported_options: &[String],
    unsupported_positionals: &[String],
) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            if line
                .option_names
                .iter()
                .any(|name| unsupported_options.contains(name))
                || line
                    .positional_names
                    .iter()
                    .any(|name| unsupported_positionals.contains(name))
            {
                format!("{}{}", line.text, UNSUPPORTED_MARKER)
            } else {
                line.text.clone()
            }
        })
        .collect()
}
