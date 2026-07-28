//! Feature metadata extraction and metadata-merge policy helpers.

use serde_json::{Map, Value};

use crate::config::{flatten_lifecycle_value, lifecycle_value_from_flattened};
use crate::runtime::mounts::mount_option_target;

pub(super) fn feature_metadata_entry(manifest: &Value) -> Value {
    let Some(entries) = manifest.as_object() else {
        return Value::Object(Map::new());
    };
    let mut metadata = Map::new();
    for key in FEATURE_METADATA_KEYS.iter().copied() {
        if let Some(value) = entries.get(key) {
            metadata.insert(key.to_string(), value.clone());
        }
    }
    Value::Object(metadata)
}

pub(crate) fn apply_feature_metadata(
    configuration: &Value,
    metadata_entries: &[Value],
    skip_feature_customizations: bool,
) -> Value {
    let config_metadata = feature_metadata_entry(configuration);
    let mut merged = configuration_without_metadata(configuration);
    for metadata in metadata_entries {
        merge_metadata_entry(&mut merged, metadata, !skip_feature_customizations);
    }
    if config_metadata
        .as_object()
        .is_some_and(|entries| !entries.is_empty())
    {
        merge_metadata_entry(&mut merged, &config_metadata, true);
    }
    Value::Object(merged)
}

const FEATURE_METADATA_KEYS: &[&str] = &[
    "containerEnv",
    "customizations",
    "entrypoint",
    "hostRequirements",
    "init",
    "mounts",
    "overrideCommand",
    "onCreateCommand",
    "updateContentCommand",
    "postCreateCommand",
    "postStartCommand",
    "postAttachCommand",
    "portsAttributes",
    "otherPortsAttributes",
    "forwardPorts",
    "privileged",
    "capAdd",
    "securityOpt",
    "remoteEnv",
    "remoteUser",
    "containerUser",
    "shutdownAction",
    "updateRemoteUserUID",
    "userEnvProbe",
    "waitFor",
];

fn configuration_without_metadata(configuration: &Value) -> Map<String, Value> {
    let mut merged = configuration.as_object().cloned().unwrap_or_default();
    for key in FEATURE_METADATA_KEYS.iter().copied() {
        merged.remove(key);
    }
    merged
}

fn merge_metadata_entry(
    merged: &mut Map<String, Value>,
    metadata: &Value,
    merge_customizations: bool,
) {
    merge_boolean_true(merged, metadata, "init");
    merge_boolean_true(merged, metadata, "privileged");
    merge_unique_array(merged, metadata, "capAdd");
    merge_unique_array(merged, metadata, "securityOpt");
    merge_mounts(merged, metadata);
    merge_unique_array(merged, metadata, "forwardPorts");
    merge_container_env(merged, metadata);
    merge_object(merged, metadata, "remoteEnv");
    merge_object(merged, metadata, "portsAttributes");
    if merge_customizations {
        merge_object(merged, metadata, "customizations");
    }
    merge_last_value(merged, metadata, "containerUser");
    merge_last_value(merged, metadata, "entrypoint");
    merge_last_value(merged, metadata, "hostRequirements");
    merge_last_value(merged, metadata, "otherPortsAttributes");
    merge_last_value(merged, metadata, "overrideCommand");
    merge_last_value(merged, metadata, "remoteUser");
    merge_last_value(merged, metadata, "shutdownAction");
    merge_last_value(merged, metadata, "updateRemoteUserUID");
    merge_last_value(merged, metadata, "userEnvProbe");
    merge_last_value(merged, metadata, "waitFor");
    for key in [
        "onCreateCommand",
        "updateContentCommand",
        "postCreateCommand",
        "postStartCommand",
        "postAttachCommand",
    ] {
        merge_lifecycle_value(merged, metadata, key);
    }
}

fn merge_boolean_true(merged: &mut Map<String, Value>, metadata: &Value, key: &str) {
    if metadata.get(key).and_then(Value::as_bool) == Some(true) {
        merged.insert(key.to_string(), Value::Bool(true));
    }
}

fn merge_unique_array(merged: &mut Map<String, Value>, metadata: &Value, key: &str) {
    let Some(values) = metadata.get(key).and_then(Value::as_array) else {
        return;
    };
    let target = merged
        .entry(key.to_string())
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .expect("array field");
    for value in values {
        if !target.iter().any(|existing| existing == value) {
            target.push(value.clone());
        }
    }
}

fn merge_mounts(merged: &mut Map<String, Value>, metadata: &Value) {
    let Some(values) = metadata.get("mounts").and_then(Value::as_array) else {
        return;
    };
    let target = merged
        .entry("mounts".to_string())
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .expect("array field");
    for value in values {
        if let Some(target_path) = mount_target(value) {
            if let Some(index) = target.iter().position(|existing| {
                mount_target(existing).as_deref() == Some(target_path.as_str())
            }) {
                target.remove(index);
            }
            target.push(value.clone());
            continue;
        }
        if !target.iter().any(|existing| existing == value) {
            target.push(value.clone());
        }
    }
}

fn mount_target(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => mount_option_target(text),
        Value::Object(entries) => entries
            .get("target")
            .or_else(|| entries.get("destination"))
            .or_else(|| entries.get("dst"))
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    }
}

fn merge_object(merged: &mut Map<String, Value>, metadata: &Value, key: &str) {
    let Some(values) = metadata.get(key).and_then(Value::as_object) else {
        return;
    };
    let target = merged
        .entry(key.to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .expect("object field");
    target.extend(
        values
            .iter()
            .map(|(name, value)| (name.clone(), value.clone())),
    );
}

fn merge_container_env(merged: &mut Map<String, Value>, metadata: &Value) {
    let Some(values) = metadata.get("containerEnv").and_then(Value::as_object) else {
        return;
    };
    let target = merged
        .entry("containerEnv".to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .expect("object field");
    for (name, value) in values {
        let value = match (target.get(name).and_then(Value::as_str), value.as_str()) {
            (Some(previous), Some(value)) => {
                Value::String(replace_env_self_reference(value, name, previous))
            }
            _ => value.clone(),
        };
        target.insert(name.clone(), value);
    }
}

fn replace_env_self_reference(value: &str, name: &str, replacement: &str) -> String {
    let braced = format!("${{{name}}}");
    let plain = format!("${name}");
    let mut result = String::with_capacity(value.len() + replacement.len());
    let mut remainder = value;
    while let Some(index) = remainder.find('$') {
        result.push_str(&remainder[..index]);
        remainder = &remainder[index..];
        if let Some(rest) = remainder.strip_prefix(&braced) {
            result.push_str(replacement);
            remainder = rest;
        } else if remainder.starts_with(&plain)
            && remainder[plain.len()..]
                .chars()
                .next()
                .is_none_or(|character| !(character.is_ascii_alphanumeric() || character == '_'))
        {
            result.push_str(replacement);
            remainder = &remainder[plain.len()..];
        } else {
            result.push('$');
            remainder = &remainder[1..];
        }
    }
    result.push_str(remainder);
    result
}

fn merge_last_value(merged: &mut Map<String, Value>, metadata: &Value, key: &str) {
    if let Some(value) = metadata.get(key) {
        merged.insert(key.to_string(), value.clone());
    }
}

fn merge_lifecycle_value(merged: &mut Map<String, Value>, metadata: &Value, key: &str) {
    let Some(value) = metadata.get(key) else {
        return;
    };
    let combined = merged
        .get(key)
        .map(flatten_lifecycle_value)
        .unwrap_or_default()
        .into_iter()
        .chain(flatten_lifecycle_value(value))
        .collect::<Vec<_>>();
    if let Some(value) = lifecycle_value_from_flattened(combined) {
        merged.insert(key.to_string(), value);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{apply_feature_metadata, feature_metadata_entry, replace_env_self_reference};

    #[test]
    fn feature_metadata_entry_ignores_non_object_manifests() {
        assert_eq!(feature_metadata_entry(&json!(null)), json!({}));
    }

    #[test]
    fn apply_feature_metadata_merges_supported_fields_by_policy() {
        let configuration = json!({
            "image": "debian:bookworm",
            "remoteUser": "config-user",
            "customizations": {
                "codespaces": {
                    "openFiles": [
                        "README.md"
                    ]
                }
            },
            "postAttachCommand": "echo config"
        });
        let merged = apply_feature_metadata(
            &configuration,
            &[json!({
                "init": true,
                "privileged": true,
                "capAdd": ["SYS_PTRACE", "SYS_PTRACE"],
                "securityOpt": ["seccomp=unconfined", "seccomp=unconfined"],
                "forwardPorts": [3000, 3000],
                "mounts": [
                    "source=old,target=/cache,type=volume",
                    {
                        "type": "bind",
                        "source": "/new",
                        "target": "/cache"
                    },
                    true,
                    true,
                    false
                ],
                "containerEnv": {
                    "A": "1"
                },
                "remoteEnv": {
                    "B": "2"
                },
                "portsAttributes": {
                    "3000": {
                        "label": "web"
                    }
                },
                "customizations": {
                    "vscode": {
                        "extensions": ["feature.extension"]
                    }
                },
                "containerUser": "node",
                "entrypoint": "/entry.sh",
                "hostRequirements": {
                    "cpus": 2
                },
                "otherPortsAttributes": {
                    "onAutoForward": "silent"
                },
                "overrideCommand": false,
                "remoteUser": "feature-user",
                "shutdownAction": "stopContainer",
                "updateRemoteUserUID": false,
                "userEnvProbe": "loginShell",
                "waitFor": "postCreateCommand",
                "onCreateCommand": "echo one",
                "updateContentCommand": ["echo", "two"],
                "postCreateCommand": {
                    "first": "echo three"
                },
                "postStartCommand": true
            })],
            false,
        );

        assert_eq!(merged["image"], "debian:bookworm");
        assert_eq!(merged["init"], true);
        assert_eq!(merged["privileged"], true);
        assert_eq!(merged["capAdd"], json!(["SYS_PTRACE"]));
        assert_eq!(merged["securityOpt"], json!(["seccomp=unconfined"]));
        assert_eq!(merged["forwardPorts"], json!([3000]));
        assert_eq!(
            merged["mounts"],
            json!([
                {
                    "type": "bind",
                    "source": "/new",
                    "target": "/cache"
                },
                true,
                false
            ])
        );
        assert_eq!(merged["containerEnv"]["A"], "1");
        assert_eq!(merged["remoteEnv"]["B"], "2");
        assert_eq!(merged["portsAttributes"]["3000"]["label"], "web");
        assert_eq!(
            merged["customizations"]["codespaces"]["openFiles"],
            json!(["README.md"])
        );
        assert_eq!(
            merged["customizations"]["vscode"]["extensions"],
            json!(["feature.extension"])
        );
        assert_eq!(merged["containerUser"], "node");
        assert_eq!(merged["entrypoint"], "/entry.sh");
        assert_eq!(merged["hostRequirements"]["cpus"], 2);
        assert_eq!(merged["otherPortsAttributes"]["onAutoForward"], "silent");
        assert_eq!(merged["overrideCommand"], false);
        assert_eq!(merged["remoteUser"], "config-user");
        assert_eq!(merged["shutdownAction"], "stopContainer");
        assert_eq!(merged["updateRemoteUserUID"], false);
        assert_eq!(merged["userEnvProbe"], "loginShell");
        assert_eq!(merged["waitFor"], "postCreateCommand");
        assert_eq!(merged["onCreateCommand"], "echo one");
        assert_eq!(merged["updateContentCommand"], json!(["echo", "two"]));
        assert_eq!(merged["postCreateCommand"], "echo three");
        assert_eq!(merged["postAttachCommand"], "echo config");
        assert!(merged.get("postStartCommand").is_none());
    }

    #[test]
    fn apply_feature_metadata_preserves_sequential_container_env_updates() {
        let merged = apply_feature_metadata(
            &json!({}),
            &[
                json!({ "containerEnv": { "PATH": "/a:$PATH" } }),
                json!({ "containerEnv": { "PATH": "/b:${PATH}" } }),
            ],
            false,
        );

        assert_eq!(merged["containerEnv"]["PATH"], "/b:/a:$PATH");
        assert_eq!(
            replace_env_self_reference("$PATH:$PATH_SUFFIX:$", "PATH", "/previous"),
            "/previous:$PATH_SUFFIX:$"
        );
    }

    #[test]
    fn apply_feature_metadata_can_skip_feature_customizations() {
        let merged = apply_feature_metadata(
            &json!({}),
            &[json!({
                "customizations": {
                    "vscode": {
                        "extensions": ["feature.extension"]
                    }
                }
            })],
            true,
        );

        assert!(merged.get("customizations").is_none());
    }

    #[test]
    fn apply_feature_metadata_replaces_mounts_by_alternate_target_keys() {
        let merged = apply_feature_metadata(
            &json!({
                "image": "debian:bookworm"
            }),
            &[
                json!({
                    "mounts": [
                        {
                            "type": "volume",
                            "source": "old",
                            "destination": "/cache"
                        },
                        {
                            "type": "volume",
                            "source": "logs",
                            "dst": "/logs"
                        }
                    ]
                }),
                json!({
                    "mounts": [
                        {
                            "type": "bind",
                            "source": "/new-cache",
                            "target": "/cache"
                        },
                        {
                            "type": "bind",
                            "source": "/new-logs",
                            "destination": "/logs"
                        }
                    ]
                }),
            ],
            false,
        );

        assert_eq!(
            merged["mounts"],
            json!([
                {
                    "type": "bind",
                    "source": "/new-cache",
                    "target": "/cache"
                },
                {
                    "type": "bind",
                    "source": "/new-logs",
                    "destination": "/logs"
                }
            ])
        );
    }
}
