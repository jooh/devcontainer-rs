//! YAML rendering helpers for compose override files.

use serde_json::{Map, Value};

use super::override_mounts::{ComposeMountDefinition, ComposeNamedVolume};

pub(super) fn escape_compose_label(label: &str) -> String {
    label.replace('\'', "''").replace('$', "$$")
}

pub(super) fn escape_compose_scalar(value: &str) -> String {
    value.replace('\'', "''").replace('$', "$$")
}

pub(super) fn render_compose_volume_entry(definition: &ComposeMountDefinition) -> String {
    render_yaml_mapping_list_entry(&definition.fields)
}

pub(super) fn render_compose_string_sequence(values: &[String]) -> Result<String, String> {
    Ok(serde_json::to_string(values).expect("compose string sequences must serialize to JSON"))
}

pub(super) fn render_named_volume_entry(entry: &ComposeNamedVolume) -> String {
    let mut rendered = format!("  {}:\n", entry.name);
    if entry.external {
        rendered.push_str("    external: true\n");
    }
    rendered
}

fn render_yaml_mapping_list_entry(entries: &Map<String, Value>) -> String {
    let mut rendered = String::new();
    let mut iter = entries.iter();
    if let Some((key, value)) = iter.next() {
        rendered.push_str(&render_yaml_key_value(key, value, 6, "- "));
    }
    for (key, value) in iter {
        rendered.push_str(&render_yaml_key_value(key, value, 8, ""));
    }
    rendered
}

fn render_yaml_key_value(key: &str, value: &Value, indent: usize, prefix: &str) -> String {
    let padding = " ".repeat(indent);
    match value {
        Value::Object(entries) => {
            let mut rendered = format!("{padding}{prefix}{key}:\n");
            let nested_indent = indent + prefix.len() + 2;
            for (nested_key, nested_value) in entries {
                rendered.push_str(&render_yaml_key_value(
                    nested_key,
                    nested_value,
                    nested_indent,
                    "",
                ));
            }
            rendered
        }
        Value::Array(values) => {
            let mut rendered = format!("{padding}{prefix}{key}:\n");
            let nested_indent = indent + prefix.len() + 2;
            for nested_value in values {
                rendered.push_str(&render_yaml_sequence_item(nested_value, nested_indent));
            }
            rendered
        }
        Value::String(text) => format!(
            "{padding}{prefix}{key}: '{}'\n",
            escape_compose_scalar(text)
        ),
        Value::Bool(boolean) => format!(
            "{padding}{prefix}{key}: {}\n",
            if *boolean { "true" } else { "false" }
        ),
        Value::Number(number) => format!("{padding}{prefix}{key}: {number}\n"),
        Value::Null => format!("{padding}{prefix}{key}: null\n"),
    }
}

fn render_yaml_sequence_item(value: &Value, indent: usize) -> String {
    let padding = " ".repeat(indent);
    match value {
        Value::Object(entries) => {
            let mut rendered = String::new();
            let mut iter = entries.iter();
            if let Some((key, value)) = iter.next() {
                rendered.push_str(&render_yaml_key_value(key, value, indent, "- "));
            }
            for (key, value) in iter {
                rendered.push_str(&render_yaml_key_value(key, value, indent + 2, ""));
            }
            rendered
        }
        Value::Array(values) => {
            let mut rendered = format!("{padding}-\n");
            for nested_value in values {
                rendered.push_str(&render_yaml_sequence_item(nested_value, indent + 2));
            }
            rendered
        }
        Value::String(text) => format!("{padding}- '{}'\n", escape_compose_scalar(text)),
        Value::Bool(boolean) => {
            format!("{padding}- {}\n", if *boolean { "true" } else { "false" })
        }
        Value::Number(number) => format!("{padding}- {number}\n"),
        Value::Null => format!("{padding}- null\n"),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Map, Value};

    use super::super::override_mounts::{ComposeMountDefinition, ComposeNamedVolume};
    use super::{
        escape_compose_label, escape_compose_scalar, render_compose_string_sequence,
        render_compose_volume_entry, render_named_volume_entry, render_yaml_key_value,
        render_yaml_sequence_item,
    };

    #[test]
    fn compose_scalar_escaping_covers_quotes_and_dollars() {
        assert_eq!(escape_compose_label("it's $HOME"), "it''s $$HOME");
        assert_eq!(
            escape_compose_scalar("can't ${EXPAND}"),
            "can''t $${EXPAND}"
        );
        assert_eq!(
            render_compose_string_sequence(&["one".to_string(), "two".to_string()])
                .expect("sequence"),
            r#"["one","two"]"#
        );
    }

    #[test]
    fn render_volume_entries_support_long_and_named_shapes() {
        let mut fields = Map::new();
        fields.insert("type".to_string(), Value::String("volume".to_string()));
        fields.insert("source".to_string(), Value::String("cache".to_string()));
        fields.insert("target".to_string(), Value::String("/cache".to_string()));
        fields.insert("read_only".to_string(), Value::Bool(true));
        fields.insert("uid".to_string(), json!(1000));
        fields.insert("optional".to_string(), Value::Null);
        fields.insert(
            "volume".to_string(),
            json!({
                "nocopy": true,
                "labels": ["one", "two"],
                "driver_opts": {
                    "o": "addr='host'"
                }
            }),
        );

        let rendered = render_compose_volume_entry(&ComposeMountDefinition { fields });

        assert!(rendered.contains("- optional: null"), "{rendered}");
        assert!(rendered.contains("type: 'volume'"), "{rendered}");
        assert!(rendered.contains("read_only: true"), "{rendered}");
        assert!(rendered.contains("uid: 1000"), "{rendered}");
        assert!(rendered.contains("labels:"), "{rendered}");
        assert!(rendered.contains("- 'one'"), "{rendered}");
        assert!(rendered.contains("driver_opts:"), "{rendered}");
        assert!(rendered.contains("o: 'addr=''host'''"), "{rendered}");

        assert_eq!(
            render_named_volume_entry(&ComposeNamedVolume {
                name: "cache".to_string(),
                external: true,
            }),
            "  cache:\n    external: true\n"
        );
        assert_eq!(
            render_named_volume_entry(&ComposeNamedVolume {
                name: "scratch".to_string(),
                external: false,
            }),
            "  scratch:\n"
        );
        assert_eq!(
            render_compose_volume_entry(&ComposeMountDefinition { fields: Map::new() }),
            ""
        );
    }

    #[test]
    fn render_yaml_helpers_cover_nested_arrays_and_scalars() {
        let rendered = render_yaml_key_value(
            "root",
            &json!({
                "child": [
                    { "name": "one", "enabled": true },
                    ["nested", null, 42],
                    false
                ]
            }),
            2,
            "",
        );

        assert!(rendered.contains("  root:"), "{rendered}");
        assert!(rendered.contains("    child:"), "{rendered}");
        assert!(rendered.contains("      - enabled: true"), "{rendered}");
        assert!(rendered.contains("        name: 'one'"), "{rendered}");
        assert!(rendered.contains("      -"), "{rendered}");
        assert!(rendered.contains("        - 'nested'"), "{rendered}");
        assert!(rendered.contains("        - null"), "{rendered}");
        assert!(rendered.contains("        - 42"), "{rendered}");
        assert!(rendered.contains("      - false"), "{rendered}");

        assert_eq!(render_yaml_sequence_item(&json!(null), 4), "    - null\n");
        assert_eq!(
            render_yaml_key_value("enabled", &json!(false), 2, ""),
            "  enabled: false\n"
        );
        assert_eq!(render_yaml_sequence_item(&json!(true), 4), "    - true\n");
        assert_eq!(render_yaml_sequence_item(&json!(false), 4), "    - false\n");
        assert_eq!(render_yaml_sequence_item(&json!(7), 4), "    - 7\n");
        assert_eq!(render_yaml_sequence_item(&json!({}), 4), "");
    }
}
