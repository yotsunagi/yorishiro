use serde_json::{Map, Value, json};

use super::types::{EntityTypeDef, FieldTypeName};

/// §9投影仕様：EntityTypeDefから、entityの`data`検証・MCP inputSchema生成の
/// 両方に使えるJSON Schemaを生成する。生成元はメタスキーマのみ（アダプタは結果を受け取るだけ）。
pub fn entity_type_to_json_schema(entity_type: &EntityTypeDef) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();

    for (field_name, field) in &entity_type.fields {
        properties.insert(field_name.clone(), field_to_json_schema(field));
        if field.required {
            required.push(Value::String(field_name.clone()));
        }
    }

    let mut schema = json!({
        "type": "object",
        "properties": properties,
    });

    if !required.is_empty() {
        schema["required"] = Value::Array(required);
    }

    schema
}

fn field_to_json_schema(field: &super::types::FieldDef) -> Value {
    let mut schema = Map::new();

    let type_str = match field.r#type {
        FieldTypeName::String => "string",
        FieldTypeName::Number => "number",
        FieldTypeName::Integer => "integer",
        FieldTypeName::Boolean => "boolean",
        FieldTypeName::Array => "array",
    };
    schema.insert("type".into(), Value::String(type_str.into()));

    if let Some(enum_values) = &field.enum_values {
        schema.insert(
            "enum".into(),
            Value::Array(enum_values.iter().cloned().map(Value::String).collect()),
        );
    }
    if let Some(format) = &field.format {
        schema.insert("format".into(), Value::String(format.clone()));
    }
    if let Some(minimum) = field.minimum {
        schema.insert("minimum".into(), json!(minimum));
    }
    if let Some(maximum) = field.maximum {
        schema.insert("maximum".into(), json!(maximum));
    }
    if let Some(default) = &field.default {
        schema.insert("default".into(), default.clone());
    }
    if matches!(field.r#type, FieldTypeName::Array)
        && let Some(items) = &field.items
    {
        schema.insert("items".into(), json!({ "type": items.r#type }));
    }

    Value::Object(schema)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metaschema::types::MetaSchemaDefinition;
    use serde_json::json;

    #[test]
    fn projects_required_and_enum_fields() {
        let def: MetaSchemaDefinition = serde_json::from_value(json!({
            "name": "task-management",
            "entity_types": {
                "project": {
                    "fields": {
                        "title": { "type": "string", "required": true, "x-embed": true },
                        "status": { "type": "string", "enum": ["active", "done"], "required": true }
                    }
                }
            }
        }))
        .unwrap();

        let schema = entity_type_to_json_schema(&def.entity_types["project"]);
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["properties"]["title"]["type"], "string");
        assert_eq!(schema["properties"]["status"]["enum"][0], "active");

        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("title")));
        assert!(required.contains(&json!("status")));
    }

    #[test]
    fn projects_array_field_with_string_items() {
        let def: MetaSchemaDefinition = serde_json::from_value(json!({
            "name": "task-management",
            "entity_types": {
                "task": {
                    "fields": {
                        "tags": { "type": "array", "items": { "type": "string" } }
                    }
                }
            }
        }))
        .unwrap();

        let schema = entity_type_to_json_schema(&def.entity_types["task"]);
        assert_eq!(schema["properties"]["tags"]["type"], "array");
        assert_eq!(schema["properties"]["tags"]["items"]["type"], "string");
    }
}
