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

    use super::{feature_object, feature_option_values_from_manifest, feature_options};

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

    #[test]
    fn feature_object_migrates_legacy_vscode_customizations() {
        let feature = feature_object(
            &json!({
                "id": "demo",
                "extensions": ["legacy.extension"],
                "settings": {
                    "legacy.setting": true
                },
                "customizations": {
                    "vscode": {
                        "extensions": ["existing.extension"],
                        "settings": {
                            "existing.setting": "value"
                        }
                    }
                }
            }),
            &json!({
                "enabled": true
            }),
            &json!({
                "enabled": false
            }),
        );

        assert_eq!(feature["included"], true);
        assert_eq!(feature["options"]["enabled"], true);
        assert_eq!(feature["value"]["enabled"], false);
        assert!(feature.get("extensions").is_none());
        assert!(feature.get("settings").is_none());
        assert_eq!(
            feature["customizations"]["vscode"]["extensions"],
            json!(["existing.extension", "legacy.extension"])
        );
        assert_eq!(
            feature["customizations"]["vscode"]["settings"],
            json!({
                "existing.setting": "value",
                "legacy.setting": true
            })
        );
    }

    #[test]
    fn feature_object_ignores_non_collection_legacy_customizations() {
        let feature = feature_object(
            &json!({
                "id": "demo",
                "extensions": "legacy.extension",
                "settings": ["legacy.setting"]
            }),
            &json!({}),
            &json!({}),
        );

        assert_eq!(feature["customizations"]["vscode"]["extensions"], json!([]));
        assert_eq!(feature["customizations"]["vscode"]["settings"], json!({}));
    }

    #[test]
    fn feature_object_migrates_single_legacy_customization_shapes() {
        let extensions = feature_object(
            &json!({
                "id": "demo",
                "extensions": ["legacy.extension"]
            }),
            &json!({}),
            &json!({}),
        );
        let settings = feature_object(
            &json!({
                "id": "demo",
                "settings": {
                    "legacy.setting": true
                }
            }),
            &json!({}),
            &json!({}),
        );

        assert_eq!(
            extensions["customizations"]["vscode"]["extensions"],
            json!(["legacy.extension"])
        );
        assert_eq!(
            settings["customizations"]["vscode"]["settings"],
            json!({
                "legacy.setting": true
            })
        );
    }

    #[test]
    fn feature_object_leaves_customizations_absent_without_legacy_fields() {
        let feature = feature_object(&json!({ "id": "demo" }), &json!({}), &json!({}));

        assert!(feature.get("customizations").is_none());
    }

    #[test]
    fn feature_options_merge_non_object_manifests_and_json_env_values() {
        assert_eq!(
            feature_options(
                &json!("not-an-object"),
                &json!({
                    "enabled": true
                })
            ),
            json!({
                "enabled": true
            })
        );

        let values = feature_option_values_from_manifest(
            &json!({
                "options": {
                    "array": { "default": ["a", "b"] },
                    "boolean": { "default": false },
                    "null": { "default": null },
                    "number": { "default": 42 },
                    "object": { "default": { "nested": true } }
                }
            }),
            &json!({}),
        );

        assert!(values.contains(&("ARRAY".to_string(), r#"["a","b"]"#.to_string())));
        assert!(values.contains(&("BOOLEAN".to_string(), "false".to_string())));
        assert!(values.contains(&("NULL".to_string(), String::new())));
        assert!(values.contains(&("NUMBER".to_string(), "42".to_string())));
        assert!(values.contains(&("OBJECT".to_string(), r#"{"nested":true}"#.to_string())));
    }
}
