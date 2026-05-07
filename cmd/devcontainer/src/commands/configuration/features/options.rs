//! Feature option merging and feature-object shaping helpers.

use serde_json::{Map, Value};

use crate::commands::common;

pub(super) fn feature_object(manifest: &Value, options: &Value, value: &Value) -> Value {
    let mut feature = manifest.as_object().cloned().unwrap_or_default();
    feature.insert("options".to_string(), options.clone());
    feature.insert("value".to_string(), value.clone());
    feature.insert("included".to_string(), Value::Bool(true));
    migrate_legacy_customizations(&mut feature);
    Value::Object(feature)
}

pub(super) fn feature_options(manifest: &Value, value: &Value) -> Value {
    Value::Object(merged_feature_options(manifest, value))
}

pub(super) fn feature_option_values_from_manifest(
    manifest: &Value,
    value: &Value,
) -> Vec<(String, String)> {
    merged_feature_options(manifest, value)
        .into_iter()
        .map(|(key, value)| {
            (
                common::feature_option_env_name(&key),
                json_value_to_env(&value),
            )
        })
        .collect()
}

fn merged_feature_options(manifest: &Value, value: &Value) -> Map<String, Value> {
    let mut merged = Map::new();
    if let Some(options) = manifest.get("options").and_then(Value::as_object) {
        for (key, option) in options {
            if let Some(default) = option.get("default") {
                merged.insert(key.clone(), default.clone());
            }
        }
    }
    if let Some(overrides) = value.as_object() {
        merged.extend(
            overrides
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
    }
    merged
}

fn migrate_legacy_customizations(feature: &mut Map<String, Value>) {
    let extensions = feature.remove("extensions");
    let settings = feature.remove("settings");
    if extensions.is_none() && settings.is_none() {
        return;
    }

    let customizations = feature
        .entry("customizations".to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .expect("customizations object");
    let vscode = customizations
        .entry("vscode".to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .expect("vscode customizations object");
    if let Some(extensions) = extensions {
        let target = vscode
            .entry("extensions".to_string())
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .expect("extensions array");
        if let Some(values) = extensions.as_array() {
            target.extend(values.iter().cloned());
        }
    }
    if let Some(settings) = settings {
        let target = vscode
            .entry("settings".to_string())
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .expect("settings object");
        if let Some(values) = settings.as_object() {
            target.extend(
                values
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
        }
    }
}

fn json_value_to_env(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(boolean) => boolean.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.clone(),
        _ => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::commands::common;

    use super::feature_option_values_from_manifest;

    #[test]
    fn feature_option_env_names_match_upstream_safe_id_cases() {
        assert_eq!(
            common::feature_option_env_name("option-name"),
            "OPTION_NAME"
        );
        assert_eq!(
            common::feature_option_env_name("option1-name-with_dashes-"),
            "OPTION1_NAME_WITH_DASHES_"
        );
        assert_eq!(
            common::feature_option_env_name("myOptionName"),
            "MYOPTIONNAME"
        );
        assert_eq!(common::feature_option_env_name("1name"), "_NAME");
        assert_eq!(
            common::feature_option_env_name("12345_option-name"),
            "_OPTION_NAME"
        );
    }

    #[test]
    fn feature_option_values_use_safe_env_names_for_defaults_and_overrides() {
        let manifest = json!({
            "id": "demo",
            "options": {
                "1name": { "type": "string", "default": "default-value" },
                "option-name": { "type": "string", "default": "default-option" }
            }
        });
        let values = feature_option_values_from_manifest(
            &manifest,
            &json!({
                "1name": "override-value"
            }),
        );

        assert!(values.contains(&("_NAME".to_string(), "override-value".to_string())));
        assert!(values.contains(&("OPTION_NAME".to_string(), "default-option".to_string())));
        assert!(!values.iter().any(|(key, _)| key == "1NAME"));
    }
}
