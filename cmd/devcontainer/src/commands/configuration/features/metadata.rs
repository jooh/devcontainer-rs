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
    merge_object(merged, metadata, "containerEnv");
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

    use super::{apply_feature_metadata, feature_metadata_entry};

    #[test]
    fn feature_metadata_entry_returns_empty_for_non_object() {
        assert_eq!(feature_metadata_entry(&json!(["not", "object"])), json!({}));
    }

    #[test]
    fn feature_metadata_mounts_replace_by_destination_aliases() {
        let merged = apply_feature_metadata(
            &json!({
                "name": "demo"
            }),
            &[
                json!({
                    "mounts": [
                        "type=bind,source=/old,target=/workspace",
                        { "type": "volume", "target": "/cache", "source": "old-cache" },
                        { "type": "volume", "dst": "/logs", "source": "old-logs" }
                    ]
                }),
                json!({
                    "mounts": [
                        { "type": "volume", "destination": "/workspace", "source": "new-workspace" },
                        "type=volume,source=new-cache,target=/cache",
                        { "type": "volume", "target": "/data", "source": "new-data" }
                    ]
                }),
            ],
            false,
        );

        assert_eq!(merged["name"], "demo");
        assert_eq!(merged["mounts"].as_array().expect("mounts").len(), 4);
        assert!(merged["mounts"]
            .as_array()
            .expect("mounts")
            .iter()
            .any(
                |mount| mount["destination"] == "/workspace" && mount["source"] == "new-workspace"
            ));
        assert!(merged["mounts"]
            .as_array()
            .expect("mounts")
            .iter()
            .any(|mount| mount == "type=volume,source=new-cache,target=/cache"));
        assert!(merged["mounts"]
            .as_array()
            .expect("mounts")
            .iter()
            .any(|mount| mount["dst"] == "/logs"));
    }

    #[test]
    fn feature_metadata_mounts_deduplicate_entries_without_targets() {
        let merged = apply_feature_metadata(
            &json!({}),
            &[
                json!({
                    "mounts": [
                        { "type": "volume", "source": "cache" },
                        { "type": "volume", "source": "cache" },
                        42
                    ]
                }),
                json!({
                    "mounts": [
                        42,
                        { "type": "volume", "source": "other" }
                    ]
                }),
            ],
            false,
        );

        assert_eq!(
            merged["mounts"],
            json!([
                { "type": "volume", "source": "cache" },
                42,
                { "type": "volume", "source": "other" }
            ])
        );
    }

    #[test]
    fn feature_metadata_last_value_keys_replace_previous_entries() {
        let merged = apply_feature_metadata(
            &json!({
                "remoteUser": "config-user"
            }),
            &[
                json!({
                    "remoteUser": "first-user",
                    "containerUser": "feature-user"
                }),
                json!({
                    "remoteUser": "second-user"
                }),
            ],
            false,
        );

        assert_eq!(merged["remoteUser"], "config-user");
        assert_eq!(merged["containerUser"], "feature-user");
    }
}
