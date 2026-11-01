//! YAML parsing for Compose files without an unsafe libyaml dependency.

use saphyr::{LoadableYamlNode, MappingOwned, ScalarOwned, YamlOwned};

/// Inspect a mapping through Compose replacement tags.
pub(super) fn as_mapping(value: &YamlOwned) -> Option<&MappingOwned> {
    untag(value).as_mapping()
}

/// Inspect a sequence through Compose replacement tags.
pub(super) fn as_sequence(value: &YamlOwned) -> Option<&Vec<YamlOwned>> {
    untag(value).as_sequence()
}

/// Retain tags in the document while allowing callers to inspect their contents.
fn untag(mut value: &YamlOwned) -> &YamlOwned {
    while let YamlOwned::Tagged(_, inner) = value {
        value = inner;
    }
    value
}

/// Parse exactly one YAML document, treating an empty file as null.
pub(super) fn from_str(input: &str) -> Result<YamlOwned, String> {
    let mut documents = YamlOwned::load_from_str(input).map_err(|error| error.to_string())?;
    match documents.len() {
        0 => Ok(YamlOwned::Value(ScalarOwned::Null)),
        1 => Ok(documents.remove(0)),
        _ => Err("Compose configuration must contain a single YAML document".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::{as_mapping, as_sequence, from_str};

    /// Match the previous parser's tag-transparent collection accessors.
    #[test]
    fn collection_accessors_inspect_tagged_values() {
        let mapping = from_str("!override {image: alpine}").expect("tagged mapping");
        assert!(as_mapping(&mapping).is_some());
        assert!(as_sequence(&mapping).is_none());
        let sequence = from_str("!override [debug]").expect("tagged sequence");
        assert!(as_mapping(&sequence).is_none());
        assert_eq!(as_sequence(&sequence).expect("sequence").len(), 1);
    }

    /// Keep null documents and invalid input handling consistent with Compose parsing.
    #[test]
    fn accepts_empty_documents_and_rejects_invalid_or_multiple_documents() {
        assert!(from_str("").expect("empty document").is_null());
        assert!(from_str("# comment\n").expect("comment only").is_null());
        assert!(from_str("null").expect("null document").is_null());
        assert!(from_str("services: [").is_err());
        assert!(from_str("---\nservices: {}\n---\nservices: {}").is_err());
        assert!(from_str("services: {}\nservices: {}").is_err());
    }

    /// Resolve anchors without changing quoted scalars or Compose extension tags.
    #[test]
    fn preserves_aliases_scalars_and_compose_tags() {
        let document = from_str(
            "defaults: &defaults {image: 'alpine:3.20', user: '1000'}\nservices:\n  app: *defaults\n  debug: {profiles: !reset []}\nquoted: 'true'\nboolean: true\n",
        )
        .expect("Compose aliases and tags");
        let app = document
            .as_mapping_get("services")
            .and_then(|services| services.as_mapping_get("app"))
            .expect("aliased service");
        assert_eq!(
            app.as_mapping_get("image").and_then(|value| value.as_str()),
            Some("alpine:3.20")
        );
        assert_eq!(
            app.as_mapping_get("user").and_then(|value| value.as_str()),
            Some("1000")
        );
        assert_eq!(
            document
                .as_mapping_get("quoted")
                .and_then(|value| value.as_str()),
            Some("true")
        );
        assert_eq!(
            document
                .as_mapping_get("boolean")
                .and_then(|value| value.as_bool()),
            Some(true)
        );
    }
}
