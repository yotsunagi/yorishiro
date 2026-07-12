use serde::Serialize;
use utoipa::ToSchema;

use super::types::MetaSchemaDefinition;

/// 要件§2.4のバージョニング規則に基づく差分判定結果。
/// `is_breaking = true` の場合、呼び出し側は新versionの行をINSERTしなければならない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct VersioningDiff {
    pub is_breaking: bool,
    pub reasons: Vec<String>,
}

/// 非破壊：フィールド追加（任意項目）、説明変更、enum値追加
/// 破壊的：フィールド削除・改名・型変更・必須化、entity_type削除、relation_type変更
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

            // enum制約自体の撤廃（Some -> None）は非破壊的な拡張（widening）として扱う。
            // 破壊的なのは、enum制約が残ったまま既存の値が使えなくなるケースのみ。
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
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(value: serde_json::Value) -> MetaSchemaDefinition {
        serde_json::from_value(value).expect("valid metaschema json")
    }

    #[test]
    fn adding_optional_field_is_non_breaking() {
        let old = parse(json!({
            "name": "n",
            "entity_types": { "task": { "fields": { "title": { "type": "string", "required": true } } } }
        }));
        let new = parse(json!({
            "name": "n",
            "entity_types": { "task": { "fields": {
                "title": { "type": "string", "required": true },
                "note": { "type": "string" }
            } } }
        }));
        let d = diff(&old, &new);
        assert!(!d.is_breaking, "reasons: {:?}", d.reasons);
    }

    #[test]
    fn adding_enum_value_is_non_breaking() {
        let old = parse(json!({
            "name": "n",
            "entity_types": { "task": { "fields": {
                "status": { "type": "string", "enum": ["active"] }
            } } }
        }));
        let new = parse(json!({
            "name": "n",
            "entity_types": { "task": { "fields": {
                "status": { "type": "string", "enum": ["active", "done"] }
            } } }
        }));
        let d = diff(&old, &new);
        assert!(!d.is_breaking, "reasons: {:?}", d.reasons);
    }

    #[test]
    fn removing_field_is_breaking() {
        let old = parse(json!({
            "name": "n",
            "entity_types": { "task": { "fields": {
                "title": { "type": "string" }, "note": { "type": "string" }
            } } }
        }));
        let new = parse(json!({
            "name": "n",
            "entity_types": { "task": { "fields": { "title": { "type": "string" } } } }
        }));
        let d = diff(&old, &new);
        assert!(d.is_breaking);
    }

    #[test]
    fn making_field_required_is_breaking() {
        let old = parse(json!({
            "name": "n",
            "entity_types": { "task": { "fields": { "title": { "type": "string" } } } }
        }));
        let new = parse(json!({
            "name": "n",
            "entity_types": { "task": { "fields": { "title": { "type": "string", "required": true } } } }
        }));
        let d = diff(&old, &new);
        assert!(d.is_breaking);
    }

    #[test]
    fn changing_field_type_is_breaking() {
        let old = parse(json!({
            "name": "n",
            "entity_types": { "task": { "fields": { "done": { "type": "boolean" } } } }
        }));
        let new = parse(json!({
            "name": "n",
            "entity_types": { "task": { "fields": { "done": { "type": "string" } } } }
        }));
        let d = diff(&old, &new);
        assert!(d.is_breaking);
    }

    #[test]
    fn removing_entity_type_is_breaking() {
        let old = parse(json!({
            "name": "n",
            "entity_types": {
                "task": { "fields": { "title": { "type": "string" } } },
                "project": { "fields": { "title": { "type": "string" } } }
            }
        }));
        let new = parse(json!({
            "name": "n",
            "entity_types": { "task": { "fields": { "title": { "type": "string" } } } }
        }));
        let d = diff(&old, &new);
        assert!(d.is_breaking);
    }

    #[test]
    fn changing_relation_type_target_is_breaking() {
        let old = parse(json!({
            "name": "n",
            "entity_types": {
                "task": { "fields": {} }, "project": { "fields": {} }, "epic": { "fields": {} }
            },
            "relation_types": { "belongs_to": { "source": "task", "target": "project" } }
        }));
        let new = parse(json!({
            "name": "n",
            "entity_types": {
                "task": { "fields": {} }, "project": { "fields": {} }, "epic": { "fields": {} }
            },
            "relation_types": { "belongs_to": { "source": "task", "target": "epic" } }
        }));
        let d = diff(&old, &new);
        assert!(d.is_breaking);
    }

    #[test]
    fn adding_new_entity_type_and_relation_type_is_non_breaking() {
        let old = parse(json!({
            "name": "n",
            "entity_types": { "task": { "fields": {} } }
        }));
        let new = parse(json!({
            "name": "n",
            "entity_types": {
                "task": { "fields": {} },
                "project": { "fields": {} }
            },
            "relation_types": { "belongs_to": { "source": "task", "target": "project" } }
        }));
        let d = diff(&old, &new);
        assert!(!d.is_breaking, "reasons: {:?}", d.reasons);
    }

    #[test]
    fn adding_new_required_field_is_breaking() {
        let old = parse(json!({
            "name": "n",
            "entity_types": { "task": { "fields": { "title": { "type": "string" } } } }
        }));
        let new = parse(json!({
            "name": "n",
            "entity_types": { "task": { "fields": {
                "title": { "type": "string" },
                "priority": { "type": "integer", "required": true }
            } } }
        }));
        let d = diff(&old, &new);
        assert!(d.is_breaking, "reasons: {:?}", d.reasons);
    }

    #[test]
    fn renaming_relation_type_is_breaking() {
        let old = parse(json!({
            "name": "n",
            "entity_types": { "task": { "fields": {} }, "project": { "fields": {} } },
            "relation_types": { "belongs_to": { "source": "task", "target": "project" } }
        }));
        let new = parse(json!({
            "name": "n",
            "entity_types": { "task": { "fields": {} }, "project": { "fields": {} } },
            "relation_types": { "part_of": { "source": "task", "target": "project" } }
        }));
        let d = diff(&old, &new);
        assert!(d.is_breaking, "reasons: {:?}", d.reasons);
    }

    #[test]
    fn removing_enum_constraint_entirely_is_non_breaking() {
        let old = parse(json!({
            "name": "n",
            "entity_types": { "task": { "fields": {
                "status": { "type": "string", "enum": ["active", "done"] }
            } } }
        }));
        let new = parse(json!({
            "name": "n",
            "entity_types": { "task": { "fields": {
                "status": { "type": "string" }
            } } }
        }));
        let d = diff(&old, &new);
        assert!(!d.is_breaking, "reasons: {:?}", d.reasons);
    }

    #[test]
    fn description_only_change_is_non_breaking() {
        let old = parse(json!({
            "name": "n",
            "description": "old description",
            "entity_types": { "task": { "description": "old", "fields": { "title": { "type": "string" } } } }
        }));
        let new = parse(json!({
            "name": "n",
            "description": "new description",
            "entity_types": { "task": { "description": "new", "fields": { "title": { "type": "string" } } } }
        }));
        let d = diff(&old, &new);
        assert!(!d.is_breaking, "reasons: {:?}", d.reasons);
    }
}
