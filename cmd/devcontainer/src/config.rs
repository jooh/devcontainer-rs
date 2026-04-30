//! Dev container config parsing, path resolution, and variable substitution.

mod jsonc;
mod lifecycle;
mod path;
mod substitution;

pub use jsonc::parse_jsonc_value;
pub(crate) use lifecycle::{flatten_lifecycle_value, lifecycle_value_from_flattened};
pub use path::{expected_config_path, resolve_config_path};
pub use substitution::{substitute_container_env, substitute_local_context, ConfigContext};

#[cfg(test)]
mod tests {
    //! Unit tests for config parsing and substitution behavior.

    use super::{
        parse_jsonc_value, resolve_config_path, substitute_container_env, substitute_local_context,
        ConfigContext,
    };
    use crate::test_support::unique_temp_dir;
    use serde_json::json;
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn discovers_standard_devcontainer_config_path() {
        let root = unique_temp_dir("devcontainer-config-test");
        let config_dir = root.join(".devcontainer");
        let config_path = config_dir.join("devcontainer.json");
        fs::create_dir_all(&config_dir).expect("failed to create config directory");
        fs::write(&config_path, "{ \"image\": \"example\" }").expect("failed to write config");

        let resolved = resolve_config_path(&root, None).expect("expected config path");

        assert_eq!(
            resolved,
            fs::canonicalize(config_path).expect("failed to canonicalize")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn parses_jsonc_with_comments_and_trailing_commas() {
        let parsed = parse_jsonc_value("{\n  // comment\n  \"name\": \"demo\",\n}\n")
            .expect("expected parse");
        assert_eq!(parsed["name"], "demo");
    }

    #[test]
    fn substitutes_local_env_and_workspace_tokens() {
        let mut env = HashMap::new();
        env.insert("USER".to_string(), "johan".to_string());
        env.insert("baz".to_string(), "somevalue".to_string());
        let context = ConfigContext {
            workspace_folder: PathBuf::from("/workspace/demo"),
            env,
            container_workspace_folder: Some("/workspaces/demo".to_string()),
            id_labels: HashMap::new(),
        };
        let value = json!({
            "containerEnv": {
                "USER_NAME": "${localEnv:USER}",
                "ENV_ALIAS": "bar${env:baz}bar",
                "WORKSPACE": "${localWorkspaceFolder}",
                "CONTAINER_WORKSPACE": "${containerWorkspaceFolder}",
                "CONTAINER_BASENAME": "${containerWorkspaceFolderBasename}"
            }
        });

        let substituted = substitute_local_context(&value, &context);

        assert_eq!(substituted["containerEnv"]["USER_NAME"], "johan");
        assert_eq!(substituted["containerEnv"]["ENV_ALIAS"], "barsomevaluebar");
        assert_eq!(substituted["containerEnv"]["WORKSPACE"], "/workspace/demo");
        assert_eq!(
            substituted["containerEnv"]["CONTAINER_WORKSPACE"],
            "/workspaces/demo"
        );
        assert_eq!(substituted["containerEnv"]["CONTAINER_BASENAME"], "demo");
    }

    #[test]
    fn substitutes_workspace_basename_and_defaulted_env_tokens() {
        let context = ConfigContext {
            workspace_folder: PathBuf::from("/workspace/demo"),
            env: HashMap::new(),
            container_workspace_folder: Some("/workspaces/${localWorkspaceFolderBasename}".into()),
            id_labels: HashMap::new(),
        };
        let value = json!({
            "containerEnv": {
                "BASENAME": "${localWorkspaceFolderBasename}",
                "DEFAULTED": "${localEnv:USER:fallback}",
                "DEFAULT_WITH_EXTRA_SEGMENTS": "${env:USER:fallback:ignored}",
                "MISSING": "before-${localEnv:UNSET}-after",
                "CONTAINER_PATH": "${containerWorkspaceFolder}"
            }
        });

        let substituted = substitute_local_context(&value, &context);

        assert_eq!(substituted["containerEnv"]["BASENAME"], "demo");
        assert_eq!(substituted["containerEnv"]["DEFAULTED"], "fallback");
        assert_eq!(
            substituted["containerEnv"]["DEFAULT_WITH_EXTRA_SEGMENTS"],
            "fallback"
        );
        assert_eq!(substituted["containerEnv"]["MISSING"], "before--after");
        assert_eq!(
            substituted["containerEnv"]["CONTAINER_PATH"],
            "/workspaces/demo"
        );
    }

    #[test]
    fn substitutes_container_env_tokens_without_replacing_local_env_tokens() {
        let value = json!({
            "remoteEnv": {
                "PATH_FROM_CONTAINER": "${containerEnv:PATH}",
                "FALLBACK": "${containerEnv:MISSING:fallback}",
                "LOCAL_PATH": "${localEnv:PATH}"
            }
        });
        let substituted = substitute_container_env(
            &value,
            &HashMap::from([("PATH".to_string(), "/usr/local/bin:/usr/bin".to_string())]),
        );

        assert_eq!(
            substituted["remoteEnv"]["PATH_FROM_CONTAINER"],
            "/usr/local/bin:/usr/bin"
        );
        assert_eq!(substituted["remoteEnv"]["FALLBACK"], "fallback");
        assert_eq!(substituted["remoteEnv"]["LOCAL_PATH"], "${localEnv:PATH}");
    }

    #[test]
    fn substitutes_stable_devcontainer_id_from_sorted_labels() {
        let value = json!({
            "mounts": [
                {
                    "source": "cache-${devcontainerId}",
                    "target": "/cache",
                    "type": "volume"
                }
            ]
        });
        let first = substitute_local_context(
            &value,
            &ConfigContext {
                workspace_folder: PathBuf::from("/workspace/demo"),
                env: HashMap::new(),
                container_workspace_folder: None,
                id_labels: HashMap::from([
                    ("b".to_string(), "2".to_string()),
                    ("a".to_string(), "1".to_string()),
                ]),
            },
        );
        let second = substitute_local_context(
            &value,
            &ConfigContext {
                workspace_folder: PathBuf::from("/workspace/demo"),
                env: HashMap::new(),
                container_workspace_folder: None,
                id_labels: HashMap::from([
                    ("a".to_string(), "1".to_string()),
                    ("b".to_string(), "2".to_string()),
                ]),
            },
        );
        let id = first["mounts"][0]["source"]
            .as_str()
            .expect("mount source")
            .trim_start_matches("cache-")
            .to_string();

        assert_eq!(first, second);
        assert_eq!(id.len(), 52);
        assert!(id
            .chars()
            .all(|character| matches!(character, '0'..='9' | 'a'..='v')));
    }

    #[test]
    fn devcontainer_id_changes_when_labels_change() {
        let value = json!({
            "test": "${devcontainerId}"
        });
        let first = substitute_local_context(
            &value,
            &ConfigContext {
                workspace_folder: PathBuf::from("/workspace/demo"),
                env: HashMap::new(),
                container_workspace_folder: None,
                id_labels: HashMap::from([("a".to_string(), "b".to_string())]),
            },
        );
        let second = substitute_local_context(
            &value,
            &ConfigContext {
                workspace_folder: PathBuf::from("/workspace/demo"),
                env: HashMap::new(),
                container_workspace_folder: None,
                id_labels: HashMap::from([
                    ("a".to_string(), "b".to_string()),
                    ("c".to_string(), "d".to_string()),
                ]),
            },
        );

        assert_ne!(first["test"], second["test"]);
    }
}
