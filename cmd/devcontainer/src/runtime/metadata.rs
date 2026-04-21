//! Container metadata serialization and merge helpers.

use serde_json::{Map, Value};

use crate::config::{flatten_lifecycle_value, lifecycle_value_from_flattened};

pub(crate) fn merged_container_metadata(metadata_label: Option<&str>) -> Value {
    let entries = metadata_entries(metadata_label);

    let mut merged = Map::new();
    merge_last_metadata_value(&entries, &mut merged, "waitFor");
    merge_last_metadata_value(&entries, &mut merged, "workspaceFolder");
    merge_last_metadata_value(&entries, &mut merged, "remoteUser");
    merge_last_metadata_value(&entries, &mut merged, "containerUser");
    merge_object_metadata_value(&entries, &mut merged, "remoteEnv");
    merge_object_metadata_value(&entries, &mut merged, "containerEnv");

    for key in [
        "initializeCommand",
        "onCreateCommand",
        "updateContentCommand",
        "postCreateCommand",
        "postStartCommand",
        "postAttachCommand",
    ] {
        if let Some(command_group) = merged_metadata_lifecycle_values(&entries, key) {
            merged.insert(key.to_string(), command_group);
        }
    }

    Value::Object(merged)
}

pub(crate) fn metadata_entries(metadata_label: Option<&str>) -> Vec<Value> {
    let Some(metadata_label) = metadata_label else {
        return Vec::new();
    };
    match serde_json::from_str::<Value>(metadata_label).ok() {
        Some(Value::Array(values)) => values,
        Some(Value::Object(entries)) => vec![Value::Object(entries)],
        _ => Vec::new(),
    }
}

pub(crate) fn serialized_container_metadata(
    configuration: &Value,
    remote_workspace_folder: &str,
    omit_config_remote_env_from_metadata: bool,
) -> Result<String, String> {
    let mut metadata = Map::new();
    for key in [
        "waitFor",
        "workspaceFolder",
        "remoteUser",
        "containerUser",
        "remoteEnv",
        "containerEnv",
        "initializeCommand",
        "onCreateCommand",
        "updateContentCommand",
        "postCreateCommand",
        "postStartCommand",
        "postAttachCommand",
    ] {
        if omit_config_remote_env_from_metadata && key == "remoteEnv" {
            continue;
        }
        if let Some(value) = configuration.get(key) {
            metadata.insert(key.to_string(), value.clone());
        }
    }
    metadata
        .entry("workspaceFolder".to_string())
        .or_insert_with(|| Value::String(remote_workspace_folder.to_string()));
    serde_json::to_string(&Value::Array(vec![Value::Object(metadata)]))
        .map_err(|error| format!("Failed to serialize container metadata: {error}"))
}

fn merge_last_metadata_value(entries: &[Value], merged: &mut Map<String, Value>, key: &str) {
    if let Some(value) = entries
        .iter()
        .filter_map(|entry| entry.get(key))
        .next_back()
    {
        merged.insert(key.to_string(), value.clone());
    }
}

fn merge_object_metadata_value(entries: &[Value], merged: &mut Map<String, Value>, key: &str) {
    let combined = entries
        .iter()
        .filter_map(|entry| entry.get(key).and_then(Value::as_object))
        .fold(Map::new(), |mut combined, value| {
            combined.extend(
                value
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone())),
            );
            combined
        });
    if !combined.is_empty() {
        merged.insert(key.to_string(), Value::Object(combined));
    }
}

fn merged_metadata_lifecycle_values(entries: &[Value], key: &str) -> Option<Value> {
    let commands = entries
        .iter()
        .filter_map(|entry| entry.get(key))
        .flat_map(flatten_lifecycle_value)
        .collect::<Vec<_>>();
    lifecycle_value_from_flattened(commands)
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::{merged_container_metadata, metadata_entries, serialized_container_metadata};
    #[test]
    fn merged_container_metadata_prefers_last_scalar_and_merges_objects() {
        let metadata = serde_json::to_string(&json!([
            {
                "workspaceFolder": "/first",
                "remoteEnv": {
                    "A": "1"
                }
            },
            {
                "workspaceFolder": "/second",
                "remoteEnv": {
                    "B": "2"
                }
            }
        ]))
        .expect("metadata json");

        let merged = merged_container_metadata(Some(&metadata));

        assert_eq!(merged["workspaceFolder"], "/second");
        assert_eq!(merged["remoteEnv"]["A"], "1");
        assert_eq!(merged["remoteEnv"]["B"], "2");
    }

    #[test]
    fn merged_container_metadata_flattens_multiple_lifecycle_entries() {
        let metadata = serde_json::to_string(&json!([
            {
                "postCreateCommand": "echo first"
            },
            {
                "postCreateCommand": {
                    "alpha": "echo second",
                    "beta": ["printf", "%s", "third"]
                }
            }
        ]))
        .expect("metadata json");

        let merged = merged_container_metadata(Some(&metadata));
        let commands = merged["postCreateCommand"]
            .as_object()
            .expect("expected flattened object");
        assert_eq!(commands.len(), 3);
    }

    #[test]
    fn metadata_entries_accepts_single_object_labels() {
        let entries = metadata_entries(Some(r#"{"postCreateCommand":"echo ready"}"#));

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["postCreateCommand"], "echo ready");
    }

    #[test]
    fn serialized_container_metadata_omits_remote_env_when_requested() {
        let metadata = serialized_container_metadata(
            &json!({
                "remoteEnv": {
                    "A": "1"
                },
                "remoteUser": "vscode"
            }),
            "/workspace",
            true,
        )
        .expect("metadata");
        let parsed: Value = serde_json::from_str(&metadata).expect("metadata json");
        let entries = parsed.as_array().expect("metadata array");

        assert_eq!(entries[0]["remoteUser"], "vscode");
        assert!(entries[0].get("remoteEnv").is_none(), "{parsed}");
    }

    #[test]
    fn serialized_container_metadata_wraps_single_metadata_entry_in_array() {
        let metadata = serialized_container_metadata(
            &json!({
                "remoteUser": "vscode"
            }),
            "/workspace",
            false,
        )
        .expect("metadata");
        let parsed: Value = serde_json::from_str(&metadata).expect("metadata json");
        let entries = parsed.as_array().expect("metadata array");

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["remoteUser"], "vscode");
        assert_eq!(entries[0]["workspaceFolder"], "/workspace");
    }
}
