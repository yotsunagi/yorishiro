use crate::error::{ValidationDetail, YorishiroError};

use super::types::{FieldTypeName, MetaSchemaDefinition};

/// Escapes `~`/`/` per RFC6901 before embedding a value as a JSON Pointer segment.
fn escape_pointer_segment(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

/// Validates the metaschema definition itself:
/// - entity_types / relation_types must have minimal internal consistency
/// - relation_types' source/target must reference keys existing in this
///   definition's entity_types
/// - array-type fields only allow items.type == "string" (MVP constraint)
/// - format is only valid for string fields; minimum/maximum are only valid
///   for number/integer fields and require minimum <= maximum
pub fn validate_definition(def: &MetaSchemaDefinition) -> Result<(), YorishiroError> {
    let mut details = Vec::new();

    if def.name.trim().is_empty() {
        details.push(ValidationDetail {
            field: "/name".into(),
            problem: "name must not be empty".into(),
        });
    }

    if def.entity_types.is_empty() {
        details.push(ValidationDetail {
            field: "/entity_types".into(),
            problem: "at least one entity type is required".into(),
        });
    }

    for (type_name, entity_type) in &def.entity_types {
        for (field_name, field) in &entity_type.fields {
            let field_path = format!(
                "/entity_types/{}/fields/{}",
                escape_pointer_segment(type_name),
                escape_pointer_segment(field_name)
            );

            if field.r#type == FieldTypeName::Array {
                match &field.items {
                    Some(items) if items.r#type == "string" => {}
                    Some(items) => details.push(ValidationDetail {
                        field: format!("{field_path}/items/type"),
                        problem: format!(
                            "array items.type must be 'string' in MVP, got '{}'",
                            items.r#type
                        ),
                    }),
                    None => details.push(ValidationDetail {
                        field: format!("{field_path}/items"),
                        problem: "array field requires items.type = 'string'".into(),
                    }),
                };
            }

            if let Some(format) = &field.format {
                if field.r#type != FieldTypeName::String {
                    details.push(ValidationDetail {
                        field: format!("{field_path}/format"),
                        problem: format!(
                            "format is only valid for string fields, but field type is {:?}",
                            field.r#type
                        ),
                    });
                } else if !matches!(format.as_str(), "date" | "date-time" | "uri") {
                    details.push(ValidationDetail {
                        field: format!("{field_path}/format"),
                        problem: format!(
                            "unsupported string format '{format}' (expected date / date-time / uri)"
                        ),
                    });
                }
            }

            let numeric_type =
                matches!(field.r#type, FieldTypeName::Number | FieldTypeName::Integer);
            if !numeric_type && (field.minimum.is_some() || field.maximum.is_some()) {
                details.push(ValidationDetail {
                    field: field_path.clone(),
                    problem: format!(
                        "minimum/maximum are only valid for number/integer fields, but field type is {:?}",
                        field.r#type
                    ),
                });
            }
            if let (Some(minimum), Some(maximum)) = (field.minimum, field.maximum)
                && minimum > maximum
            {
                details.push(ValidationDetail {
                    field: field_path.clone(),
                    problem: format!("minimum ({minimum}) must not exceed maximum ({maximum})"),
                });
            }
        }
    }

    for (relation_name, relation) in &def.relation_types {
        let relation_path = format!("/relation_types/{}", escape_pointer_segment(relation_name));

        if !def.entity_types.contains_key(&relation.source) {
            details.push(ValidationDetail {
                field: format!("{relation_path}/source"),
                problem: format!(
                    "source entity type '{}' is not defined in entity_types",
                    relation.source
                ),
            });
        }

        if !def.entity_types.contains_key(&relation.target) {
            details.push(ValidationDetail {
                field: format!("{relation_path}/target"),
                problem: format!(
                    "target entity type '{}' is not defined in entity_types",
                    relation.target
                ),
            });
        }
    }

    if details.is_empty() {
        Ok(())
    } else {
        Err(YorishiroError::ValidationFailed {
            message: format!("metaschema definition '{}' is invalid", def.name),
            details,
            hint: "Check the consistency between entity_types and relation_types".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(value: serde_json::Value) -> MetaSchemaDefinition {
        serde_json::from_value(value).expect("valid metaschema json")
    }

    #[test]
    fn accepts_well_formed_definition() {
        let def = parse(json!({
            "name": "task-management",
            "entity_types": {
                "project": { "fields": { "title": { "type": "string", "required": true } } },
                "task": { "fields": { "title": { "type": "string", "required": true } } }
            },
            "relation_types": {
                "belongs_to": { "source": "task", "target": "project" }
            }
        }));
        assert!(validate_definition(&def).is_ok());
    }

    #[test]
    fn rejects_unknown_relation_target() {
        let def = parse(json!({
            "name": "task-management",
            "entity_types": {
                "task": { "fields": { "title": { "type": "string" } } }
            },
            "relation_types": {
                "belongs_to": { "source": "task", "target": "project" }
            }
        }));
        let err = validate_definition(&def).unwrap_err();
        match err {
            YorishiroError::ValidationFailed { details, .. } => {
                assert!(details.iter().any(|d| d.field.contains("target")));
            }
            _ => panic!("expected ValidationFailed"),
        }
    }

    #[test]
    fn rejects_array_field_without_string_items() {
        let def = parse(json!({
            "name": "task-management",
            "entity_types": {
                "task": {
                    "fields": {
                        "tags": { "type": "array", "items": { "type": "number" } }
                    }
                }
            }
        }));
        assert!(validate_definition(&def).is_err());
    }

    #[test]
    fn rejects_empty_entity_types() {
        let def = parse(json!({ "name": "empty", "entity_types": {} }));
        assert!(validate_definition(&def).is_err());
    }

    #[test]
    fn rejects_format_on_non_string_field() {
        let def = parse(json!({
            "name": "task-management",
            "entity_types": {
                "task": { "fields": { "count": { "type": "integer", "format": "date" } } }
            }
        }));
        let err = validate_definition(&def).unwrap_err();
        match err {
            YorishiroError::ValidationFailed { details, .. } => {
                assert!(details.iter().any(|d| d.field.ends_with("/format")));
            }
            _ => panic!("expected ValidationFailed"),
        }
    }

    #[test]
    fn rejects_minimum_on_boolean_field() {
        let def = parse(json!({
            "name": "task-management",
            "entity_types": {
                "task": { "fields": { "done": { "type": "boolean", "minimum": 0 } } }
            }
        }));
        assert!(validate_definition(&def).is_err());
    }

    #[test]
    fn rejects_minimum_greater_than_maximum() {
        let def = parse(json!({
            "name": "task-management",
            "entity_types": {
                "task": { "fields": { "score": { "type": "integer", "minimum": 10, "maximum": 1 } } }
            }
        }));
        assert!(validate_definition(&def).is_err());
    }
}
