use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct MetaSchemaDefinition {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub entity_types: BTreeMap<String, EntityTypeDef>,
    #[serde(default)]
    pub relation_types: BTreeMap<String, RelationTypeDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct EntityTypeDef {
    #[serde(default)]
    pub description: Option<String>,
    pub fields: BTreeMap<String, FieldDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RelationTypeDef {
    pub source: String,
    pub target: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FieldTypeName {
    String,
    Number,
    Integer,
    Boolean,
    Array,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ArrayItems {
    pub r#type: String,
}

/// Field definition, restricted to the MVP types (string/number/integer/boolean/array).
/// Unknown `x-` attributes are preserved via `extra` (flattened) so that older clients
/// don't drop fields they don't recognize yet.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FieldDef {
    pub r#type: FieldTypeName,
    #[serde(default)]
    pub required: bool,
    #[serde(default, rename = "enum", skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>)]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<ArrayItems>,
    #[serde(
        default,
        rename = "x-embed",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub x_embed: bool,
    #[serde(default, rename = "x-ui", skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>)]
    pub x_ui: Option<Value>,
    #[serde(flatten)]
    #[schema(value_type = Object)]
    pub extra: serde_json::Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn preserves_known_and_unknown_x_attributes() {
        let field: FieldDef = serde_json::from_value(json!({
            "type": "string",
            "x-embed": true,
            "x-ui": { "widget": "select", "options": ["a", "b"] },
            "x-custom-client-hint": { "anything": 1 }
        }))
        .unwrap();

        assert!(field.x_embed);
        assert_eq!(
            field.x_ui,
            Some(json!({ "widget": "select", "options": ["a", "b"] }))
        );
        assert_eq!(
            field.extra.get("x-custom-client-hint"),
            Some(&json!({ "anything": 1 }))
        );

        let roundtripped = serde_json::to_value(&field).unwrap();
        assert_eq!(
            roundtripped["x-custom-client-hint"],
            json!({ "anything": 1 })
        );
    }
}
