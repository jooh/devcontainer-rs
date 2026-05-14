//! Mount parsing and normalization helpers for compose override files.

use serde_json::{Map, Number, Value};

use crate::runtime::context::{
    additional_mounts_for_workspace_target, workspace_mount_for_args, ResolvedConfig,
};
use crate::runtime::mounts::cli_mount_values;
use crate::runtime::mounts::split_mount_options;

pub(super) enum ComposeVolumeEntry {
    Short(String),
    Long(ComposeMountDefinition),
}

pub(super) struct ComposeNamedVolume {
    pub(super) name: String,
    pub(super) external: bool,
}

pub(super) struct ComposeMountDefinition {
    pub(super) fields: Map<String, Value>,
}

pub(super) fn compose_workspace_volume(
    resolved: &ResolvedConfig,
    args: &[String],
    remote_workspace_folder: &str,
) -> Option<ComposeVolumeEntry> {
    let mount = workspace_mount_for_args(resolved, remote_workspace_folder, args);
    let definition = compose_mount_definition_from_str(&mount)?;
    if definition.mount_type().unwrap_or("bind") != "bind" {
        return None;
    }
    definition
        .short_syntax()
        .map(ComposeVolumeEntry::Short)
        .or(Some(ComposeVolumeEntry::Long(definition)))
}

pub(super) fn compose_additional_volumes(
    resolved: &ResolvedConfig,
    args: &[String],
) -> Result<Vec<ComposeVolumeEntry>, String> {
    let mut volumes = Vec::new();
    if resolved.configuration.get("workspaceMount").is_none() {
        let remote_workspace_folder =
            crate::runtime::context::remote_workspace_folder_for_args(resolved, args);
        volumes.extend(
            additional_mounts_for_workspace_target(resolved, &remote_workspace_folder, args)
                .iter()
                .filter_map(|mount| compose_mount_definition_from_str(mount))
                .map(ComposeVolumeEntry::Long),
        );
    }
    volumes.extend(
        resolved
            .configuration
            .get("mounts")
            .and_then(Value::as_array)
            .map(|mounts| {
                mounts
                    .iter()
                    .filter_map(compose_mount_definition)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
    );
    volumes.extend(
        cli_mount_values(args)?
            .iter()
            .filter_map(|mount| compose_mount_definition_from_str(mount))
            .map(ComposeVolumeEntry::Long),
    );
    Ok(volumes)
}

pub(super) fn compose_named_volumes(volumes: &[ComposeVolumeEntry]) -> Vec<ComposeNamedVolume> {
    let mut named_volumes: Vec<ComposeNamedVolume> = Vec::new();
    for volume in volumes {
        let ComposeVolumeEntry::Long(definition) = volume else {
            continue;
        };
        if definition.mount_type().unwrap_or("bind") != "volume" {
            continue;
        }
        let Some(name) = definition
            .fields
            .get("source")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let external = definition
            .fields
            .get("volume")
            .and_then(Value::as_object)
            .and_then(|volume| volume.get("external"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if let Some(existing) = named_volumes
            .iter_mut()
            .find(|existing| existing.name == name)
        {
            existing.external |= external;
            continue;
        }
        named_volumes.push(ComposeNamedVolume {
            name: name.to_string(),
            external,
        });
    }
    named_volumes
}

fn compose_mount_definition(value: &Value) -> Option<ComposeVolumeEntry> {
    match value {
        Value::String(text) => {
            compose_mount_definition_from_str(text).map(ComposeVolumeEntry::Long)
        }
        Value::Object(entries) => {
            let mut fields = Map::new();
            fields.insert(
                "type".to_string(),
                Value::String(
                    entries
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("bind")
                        .to_string(),
                ),
            );
            if let Some(source) = entries
                .get("source")
                .or_else(|| entries.get("src"))
                .and_then(Value::as_str)
            {
                fields.insert("source".to_string(), Value::String(source.to_string()));
            }
            let target = entries
                .get("target")
                .or_else(|| entries.get("destination"))
                .or_else(|| entries.get("dst"))
                .and_then(Value::as_str)?;
            fields.insert("target".to_string(), Value::String(target.to_string()));
            if entries
                .get("readonly")
                .or_else(|| entries.get("readOnly"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                fields.insert("read_only".to_string(), Value::Bool(true));
            }
            if let Some(external) = entries.get("external").and_then(Value::as_bool) {
                insert_nested_mount_value(
                    &mut fields,
                    &["volume"],
                    "external",
                    Value::Bool(external),
                );
            }
            for (key, value) in entries {
                if matches!(
                    key.as_str(),
                    "type"
                        | "source"
                        | "src"
                        | "target"
                        | "destination"
                        | "dst"
                        | "readonly"
                        | "readOnly"
                        | "external"
                ) {
                    continue;
                }
                merge_mount_value(&mut fields, key, value.clone());
            }
            Some(ComposeVolumeEntry::Long(ComposeMountDefinition { fields }))
        }
        _ => None,
    }
}

fn compose_mount_definition_from_str(mount: &str) -> Option<ComposeMountDefinition> {
    let mut fields = Map::new();
    fields.insert("type".to_string(), Value::String("bind".to_string()));
    for option in split_mount_options(mount) {
        if option == "readonly" || option == "ro" {
            fields.insert("read_only".to_string(), Value::Bool(true));
            continue;
        }
        if let Some(value) = option.strip_prefix("type=") {
            fields.insert(
                "type".to_string(),
                Value::String(value.trim_matches('"').to_string()),
            );
        } else if let Some(value) = option
            .strip_prefix("source=")
            .or_else(|| option.strip_prefix("src="))
        {
            fields.insert(
                "source".to_string(),
                Value::String(value.trim_matches('"').to_string()),
            );
        } else if let Some(value) = option
            .strip_prefix("target=")
            .or_else(|| option.strip_prefix("destination="))
            .or_else(|| option.strip_prefix("dst="))
        {
            fields.insert(
                "target".to_string(),
                Value::String(value.trim_matches('"').to_string()),
            );
        } else if let Some(value) = option.strip_prefix("external=") {
            if let Some(external) = parse_mount_option_scalar(value).as_bool() {
                insert_nested_mount_value(
                    &mut fields,
                    &["volume"],
                    "external",
                    Value::Bool(external),
                );
            }
        } else if let Some((key, value)) = option.split_once('=') {
            let path = mount_option_key_path(key);
            if let Some((leaf, parents)) = path.split_last() {
                insert_nested_mount_value(
                    &mut fields,
                    parents,
                    leaf,
                    parse_mount_option_scalar(value),
                );
            }
        }
    }

    fields
        .contains_key("target")
        .then_some(ComposeMountDefinition { fields })
}

impl ComposeMountDefinition {
    pub(super) fn mount_type(&self) -> Option<&str> {
        self.fields.get("type").and_then(Value::as_str)
    }

    pub(super) fn short_syntax(&self) -> Option<String> {
        if self.mount_type().unwrap_or("bind") != "bind" {
            return None;
        }
        if self
            .fields
            .keys()
            .any(|key| !matches!(key.as_str(), "type" | "source" | "target" | "read_only"))
        {
            return None;
        }
        let source = self.fields.get("source").and_then(Value::as_str)?;
        let target = self.fields.get("target").and_then(Value::as_str)?;
        let mut volume = format!("{source}:{target}");
        if self
            .fields
            .get("read_only")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            volume.push_str(":ro");
        }
        Some(volume)
    }
}

fn mount_option_key_path(key: &str) -> Vec<&str> {
    match key {
        "bind-propagation" => vec!["bind", "propagation"],
        "volume-nocopy" => vec!["volume", "nocopy"],
        _ => key.split('.').collect(),
    }
}

fn parse_mount_option_scalar(value: &str) -> Value {
    let value = value.trim_matches('"');
    match value {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        _ => parse_mount_option_number(value).unwrap_or_else(|| Value::String(value.to_string())),
    }
}

fn parse_mount_option_number(value: &str) -> Option<Value> {
    if let Ok(number) = value.parse::<i64>() {
        return Some(Value::Number(number.into()));
    }
    if let Ok(number) = value.parse::<u64>() {
        return Some(Value::Number(number.into()));
    }
    value
        .parse::<f64>()
        .ok()
        .and_then(Number::from_f64)
        .map(Value::Number)
}

fn insert_nested_mount_value(
    fields: &mut Map<String, Value>,
    parents: &[&str],
    leaf: &str,
    value: Value,
) {
    if parents.is_empty() {
        fields.insert(leaf.to_string(), value);
        return;
    }

    let entry = fields
        .entry(parents[0].to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    let child = entry.as_object_mut().expect("object mount option");
    insert_nested_mount_value(child, &parents[1..], leaf, value);
}

fn merge_mount_value(fields: &mut Map<String, Value>, key: &str, value: Value) {
    if let Some(existing) = fields.get_mut(key) {
        merge_mount_scalar_or_object(existing, value);
        return;
    }

    fields.insert(key.to_string(), value);
}

fn merge_mount_scalar_or_object(existing: &mut Value, incoming: Value) {
    match (existing, incoming) {
        (Value::Object(existing), Value::Object(incoming)) => {
            for (key, value) in incoming {
                merge_mount_value(existing, &key, value);
            }
        }
        (existing, incoming) => *existing = incoming,
    }
}

pub(super) fn compose_environment(configuration: &Value) -> Option<Vec<(String, String)>> {
    let env = configuration
        .get("containerEnv")
        .and_then(Value::as_object)?
        .iter()
        .filter_map(|(key, value)| value.as_str().map(|text| (key.clone(), text.to_string())))
        .collect::<Vec<_>>();
    (!env.is_empty()).then_some(env)
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Map, Value};

    use super::{
        compose_mount_definition, compose_mount_definition_from_str, compose_named_volumes,
        insert_nested_mount_value, merge_mount_scalar_or_object, parse_mount_option_scalar,
        ComposeVolumeEntry,
    };

    fn long_definition(entry: Option<ComposeVolumeEntry>) -> Map<String, Value> {
        let Some(ComposeVolumeEntry::Long(definition)) = entry else {
            panic!("expected long compose mount definition");
        };
        definition.fields
    }

    #[test]
    fn compose_mount_definition_accepts_object_aliases_and_read_only() {
        let fields = long_definition(compose_mount_definition(&json!({
            "type": "volume",
            "src": "cache",
            "dst": "/cache",
            "readOnly": true,
            "external": true,
            "volume": {
                "labels": {
                    "owner": "devcontainer"
                }
            },
            "consistency": "cached"
        })));

        assert_eq!(fields.get("type"), Some(&json!("volume")));
        assert_eq!(fields.get("source"), Some(&json!("cache")));
        assert_eq!(fields.get("target"), Some(&json!("/cache")));
        assert_eq!(fields.get("read_only"), Some(&json!(true)));
        assert_eq!(fields.get("volume.external"), None);
        assert_eq!(
            fields.get("volume"),
            Some(&json!({
                "external": true,
                "labels": {
                    "owner": "devcontainer"
                }
            }))
        );
        assert!(compose_mount_definition(&json!(false)).is_none());

        let Some(ComposeVolumeEntry::Long(definition)) = compose_mount_definition(&json!({
            "type": "volume",
            "target": "/cache"
        })) else {
            panic!("expected long compose mount definition");
        };
        assert_eq!(definition.short_syntax(), None);
    }

    #[test]
    fn compose_mount_definition_from_str_preserves_extended_options() {
        let definition = compose_mount_definition_from_str(
            "type=volume,source=cache,target=/cache,external=true,volume-nocopy=true,\
             bind-propagation=rshared,retries=-2,limit=18446744073709551615,ratio=1.5",
        )
        .expect("string mount should parse");

        assert_eq!(definition.fields.get("type"), Some(&json!("volume")));
        assert_eq!(
            definition.fields.get("volume"),
            Some(&json!({
                "external": true,
                "nocopy": true
            }))
        );
        assert_eq!(
            definition.fields.get("bind"),
            Some(&json!({ "propagation": "rshared" }))
        );
        assert_eq!(definition.fields.get("retries"), Some(&json!(-2)));
        assert_eq!(
            definition.fields.get("limit").and_then(Value::as_u64),
            Some(u64::MAX)
        );
        assert_eq!(definition.fields.get("ratio"), Some(&json!(1.5)));
        assert_eq!(definition.short_syntax(), None);

        let readonly = compose_mount_definition_from_str("source=/host,target=/work,readonly")
            .expect("readonly bind mount should parse");
        assert_eq!(readonly.short_syntax(), Some("/host:/work:ro".to_string()));
    }

    #[test]
    fn compose_named_volumes_merges_duplicate_external_flags() {
        let local = compose_mount_definition_from_str("type=volume,source=cache,target=/cache")
            .expect("local named volume should parse");
        let external = compose_mount_definition_from_str(
            "type=volume,source=cache,target=/cache,external=true",
        )
        .expect("external named volume should parse");
        let anonymous = compose_mount_definition_from_str("type=volume,target=/anonymous")
            .expect("anonymous volume should parse");
        let bind = compose_mount_definition_from_str("source=/host,target=/work")
            .expect("bind mount should parse");

        let named = compose_named_volumes(&[
            ComposeVolumeEntry::Short("/host:/work".to_string()),
            ComposeVolumeEntry::Long(local),
            ComposeVolumeEntry::Long(external),
            ComposeVolumeEntry::Long(anonymous),
            ComposeVolumeEntry::Long(bind),
        ]);

        assert_eq!(named.len(), 1);
        assert_eq!(named[0].name, "cache");
        assert!(named[0].external);
    }

    #[test]
    fn nested_mount_values_replace_scalars_and_merge_objects() {
        let mut fields = Map::from_iter([("volume".to_string(), json!("scalar"))]);
        insert_nested_mount_value(&mut fields, &["volume"], "nocopy", json!(true));
        assert_eq!(fields.get("volume"), Some(&json!({ "nocopy": true })));

        let mut existing = json!("replace-me");
        merge_mount_scalar_or_object(&mut existing, json!({ "external": true }));
        assert_eq!(existing, json!({ "external": true }));

        assert_eq!(parse_mount_option_scalar("\"quoted\""), json!("quoted"));
    }
}
