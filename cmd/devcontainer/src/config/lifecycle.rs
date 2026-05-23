//! Shared helpers for flattening and rebuilding lifecycle command values.

use serde_json::Value;

pub(crate) fn flatten_lifecycle_value(value: &Value) -> Vec<Value> {
    match value {
        Value::String(_) | Value::Array(_) => vec![value.clone()],
        Value::Object(entries) => entries.values().flat_map(flatten_lifecycle_value).collect(),
        _ => Vec::new(),
    }
}

pub(crate) fn lifecycle_value_from_flattened(values: Vec<Value>) -> Option<Value> {
    match values.len() {
        0 => None,
        1 => values.into_iter().next(),
        _ => Some(Value::Object(
            values
                .into_iter()
                .enumerate()
                .map(|(index, value)| (index.to_string(), value))
                .collect(),
        )),
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for lifecycle value helpers.

    use serde_json::json;

    use super::{flatten_lifecycle_value, lifecycle_value_from_flattened};

    #[test]
    fn lifecycle_helpers_flatten_and_rebuild_objects() {
        let flattened = flatten_lifecycle_value(&json!({
            "alpha": "echo first",
            "beta": ["printf", "%s", "second"],
        }));

        let rebuilt = lifecycle_value_from_flattened(flattened).expect("rebuilt");
        assert!(rebuilt.is_object());
        assert_eq!(rebuilt.as_object().expect("object").len(), 2);
    }

    #[test]
    fn lifecycle_helpers_ignore_non_command_values_and_preserve_single_values() {
        assert!(flatten_lifecycle_value(&json!(false)).is_empty());
        assert_eq!(lifecycle_value_from_flattened(Vec::new()), None);
        assert_eq!(
            lifecycle_value_from_flattened(vec![json!("echo once")]),
            Some(json!("echo once"))
        );
    }
}
