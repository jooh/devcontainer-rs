//! Collection-oriented command entrypoints and shared helpers.

mod feature_tests;
mod features;
pub(crate) mod oci;
mod publish;
pub(crate) mod registry;
mod templates;

use std::process::ExitCode;

use serde_json::Value;

use crate::commands::common;

pub(crate) fn validate_oci_auth_options(options: &common::OciAuthOptions) -> Result<(), String> {
    oci::parse_cross_origin_auth_hosts(&options.allowed_cross_origin_auth_hosts).map(|_| ())
}

pub(crate) fn run_features(args: &[String]) -> ExitCode {
    let (subcommand, subcommand_args) = match args.split_first() {
        Some((subcommand, subcommand_args)) => (subcommand.as_str(), subcommand_args),
        None => ("", &[][..]),
    };
    let result = match subcommand {
        "resolve-dependencies" => {
            features::build_features_resolve_dependencies_payload(subcommand_args)
        }
        "info" => {
            let positionals = crate::cli::command_positionals("features info", subcommand_args);
            if positionals.len() < 2 {
                Err("features info requires manifest <feature>".to_string())
            } else {
                let _ = common::parse_option_value(subcommand_args, "--log-level");
                let workspace_folder =
                    common::parse_option_value(subcommand_args, "--workspace-folder")
                        .map(std::path::PathBuf::from);
                match features::build_feature_info_payload_with_workspace(
                    &positionals[0],
                    &positionals[1],
                    workspace_folder.as_deref(),
                ) {
                    Ok(payload)
                        if common::parse_option_value(subcommand_args, "--output-format")
                            .as_deref()
                            == Some("text") =>
                    {
                        println!("{}", render_collection_info_text(&payload));
                        return ExitCode::SUCCESS;
                    }
                    result => result,
                }
            }
        }
        "test" => return feature_tests::run_features_test(subcommand_args),
        "package" => {
            let positionals = crate::cli::command_positionals("features package", subcommand_args);
            if positionals.is_empty() {
                Err("features package requires <target>".to_string())
            } else {
                match publish::package_collection_target(
                    std::path::Path::new(&positionals[0]),
                    "devcontainer-feature.json",
                    "feature",
                ) {
                    Ok(archive) => Ok(serde_json::json!({
                        "outcome": "success",
                        "command": "features package",
                        "archive": archive,
                    })),
                    Err(error) => Err(error),
                }
            }
        }
        "publish" => {
            let positionals = crate::cli::command_positionals("features publish", subcommand_args);
            if positionals.is_empty() {
                Err("features publish requires <target>".to_string())
            } else {
                publish::publish_collection_target_to_oci(
                    std::path::Path::new(&positionals[0]),
                    "devcontainer-feature.json",
                    "feature",
                    "features publish",
                    subcommand_args,
                )
            }
        }
        "generate-docs" => {
            let positionals =
                crate::cli::command_positionals("features generate-docs", subcommand_args);
            if positionals.is_empty() {
                Err("features generate-docs requires <target>".to_string())
            } else {
                let options = common::ManifestDocOptions {
                    registry: Some(
                        common::parse_option_value(subcommand_args, "--registry")
                            .unwrap_or("ghcr.io".to_string()),
                    ),
                    namespace: common::parse_option_value(subcommand_args, "--namespace"),
                    github_owner: common::parse_option_value(subcommand_args, "--github-owner"),
                    github_repo: common::parse_option_value(subcommand_args, "--github-repo"),
                };
                match crate::commands::common::generate_manifest_docs(
                    std::path::Path::new(&positionals[0]),
                    "devcontainer-feature.json",
                    "Feature",
                    &options,
                ) {
                    Ok(readme) => Ok(serde_json::json!({
                        "outcome": "success",
                        "command": "features generate-docs",
                        "readme": readme,
                    })),
                    Err(error) => Err(error),
                }
            }
        }
        "" => Err("features requires a subcommand".to_string()),
        _ => Err(format!("Unsupported features subcommand: {subcommand}")),
    };

    print_result(result)
}

fn render_collection_info_text(payload: &Value) -> String {
    serde_json::to_string_pretty(payload).expect("serializing JSON value cannot fail")
}

pub(crate) fn run_templates(args: &[String]) -> ExitCode {
    let (subcommand, subcommand_args) = match args.split_first() {
        Some((subcommand, subcommand_args)) => (subcommand.as_str(), subcommand_args),
        None => ("", &[][..]),
    };
    let result = match subcommand {
        "apply" => templates::run_template_apply(subcommand_args),
        "metadata" => {
            let positionals =
                crate::cli::command_positionals("templates metadata", subcommand_args);
            if positionals.is_empty() {
                Err("templates metadata requires <target>".to_string())
            } else {
                let workspace_folder =
                    common::parse_option_value(subcommand_args, "--workspace-folder")
                        .map(std::path::PathBuf::from);
                templates::build_template_metadata_payload(
                    &positionals[0],
                    workspace_folder.as_deref(),
                )
            }
        }
        "publish" => {
            let positionals = crate::cli::command_positionals("templates publish", subcommand_args);
            if positionals.is_empty() {
                Err("templates publish requires <target>".to_string())
            } else {
                publish::publish_collection_target_to_oci(
                    std::path::Path::new(&positionals[0]),
                    "devcontainer-template.json",
                    "template",
                    "templates publish",
                    subcommand_args,
                )
            }
        }
        "generate-docs" => {
            let positionals =
                crate::cli::command_positionals("templates generate-docs", subcommand_args);
            if positionals.is_empty() {
                Err("templates generate-docs requires <target>".to_string())
            } else {
                let options = common::ManifestDocOptions {
                    github_owner: common::parse_option_value(subcommand_args, "--github-owner"),
                    github_repo: common::parse_option_value(subcommand_args, "--github-repo"),
                    ..Default::default()
                };
                match crate::commands::common::generate_manifest_docs(
                    std::path::Path::new(&positionals[0]),
                    "devcontainer-template.json",
                    "Template",
                    &options,
                ) {
                    Ok(readme) => Ok(serde_json::json!({
                        "outcome": "success",
                        "command": "templates generate-docs",
                        "readme": readme,
                    })),
                    Err(error) => Err(error),
                }
            }
        }
        "" => Err("templates requires a subcommand".to_string()),
        _ => Err(format!("Unsupported templates subcommand: {subcommand}")),
    };

    print_result(result)
}

fn print_result(result: Result<Value, String>) -> ExitCode {
    match result {
        Ok(payload) => {
            println!("{payload}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod zero_line_tests {
    use std::fs;
    use std::process::ExitCode;

    #[test]
    fn zero_hit_features_resolve_dependencies_entrypoint_passes_subcommand_args() {
        let root = crate::test_support::unique_temp_dir("devcontainer-collections-entrypoint");
        let config_dir = root.join(".devcontainer");
        fs::create_dir_all(&config_dir).expect("config dir");
        fs::write(
            config_dir.join("devcontainer.json"),
            "{\n  \"image\": \"debian:bookworm\"\n}\n",
        )
        .expect("config");

        assert_eq!(
            super::run_features(&[
                "resolve-dependencies".to_string(),
                "--workspace-folder".to_string(),
                root.display().to_string(),
            ]),
            ExitCode::SUCCESS
        );

        let _ = fs::remove_dir_all(root);
    }
}
