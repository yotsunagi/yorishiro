use serde::Serialize;
use utoipa::ToSchema;

use super::types::MetaSchemaDefinition;

/// Diff result describing whether a metaschema change is backward compatible.
/// When `is_breaking = true`, the caller must INSERT a new version row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct VersioningDiff {
    pub is_breaking: bool,
    pub reasons: Vec<String>,
}

/// Non-breaking: adding an optional field, description changes, adding enum values.
/// Breaking: removing/renaming a field, changing its type, making it required,
/// removing an entity_type, changing a relation_type.
pub fn diff(old: &MetaSchemaDefinition, new: &MetaSchemaDefinition) -> VersioningDiff {
    let mut reasons = Vec::new();

    for (type_name, old_entity_type) in &old.entity_types {
        let Some(new_entity_type) = new.entity_types.get(type_name) else {
            reasons.push(format!("entity_type '{type_name}' was removed"));
            continue;
        };

        for (field_name, old_field) in &old_entity_type.fields {
            let Some(new_field) = new_entity_type.fields.get(field_name) else {
                reasons.push(format!(
                    "field '{field_name}' was removed from entity_type '{type_name}' (or renamed)"
                ));
                continue;
            };

            if old_field.r#type != new_field.r#type {
                reasons.push(format!(
                    "field '{type_name}.{field_name}' changed type from {:?} to {:?}",
                    old_field.r#type, new_field.r#type
                ));
            }

            if !old_field.required && new_field.required {
                reasons.push(format!("field '{type_name}.{field_name}' became required"));
            }

            if let (Some(old_items), Some(new_items)) = (&old_field.items, &new_field.items)
                && old_items.r#type != new_items.r#type
            {
                reasons.push(format!(
                    "field '{type_name}.{field_name}' array items type changed from '{}' to '{}'",
                    old_items.r#type, new_items.r#type
                ));
            }

            // Removing the enum constraint entirely (Some -> None) is treated
            // as a non-breaking widening. Only the case where the constraint
            // remains but an existing value is no longer allowed is breaking.
            if let Some(old_enum) = &old_field.enum_values
                && let Some(new_enum) = &new_field.enum_values
            {
                for value in old_enum {
                    if !new_enum.contains(value) {
                        reasons.push(format!(
                            "field '{type_name}.{field_name}' enum value '{value}' was removed"
                        ));
                    }
                }
            }
        }

        for (field_name, new_field) in &new_entity_type.fields {
            if !old_entity_type.fields.contains_key(field_name) && new_field.required {
                reasons.push(format!(
                    "new field '{type_name}.{field_name}' was added as required"
                ));
            }
        }
    }

    for (relation_name, old_relation) in &old.relation_types {
        match new.relation_types.get(relation_name) {
            None => reasons.push(format!("relation_type '{relation_name}' was removed")),
            Some(new_relation) => {
                if old_relation.source != new_relation.source
                    || old_relation.target != new_relation.target
                {
                    reasons.push(format!(
                        "relation_type '{relation_name}' source/target changed"
                    ));
                }
            }
        }
    }

    VersioningDiff {
        is_breaking: !reasons.is_empty(),
        reasons,
    }
}

#[cfg(test)]
mod tests;
