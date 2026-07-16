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
