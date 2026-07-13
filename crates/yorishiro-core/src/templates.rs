use serde::Serialize;
use utoipa::ToSchema;

use crate::error::YorishiroError;
use crate::metaschema::MetaSchemaDefinition;

struct BuiltinTemplate {
    id: &'static str,
    source: &'static str,
}

const TEMPLATES: &[BuiltinTemplate] = &[BuiltinTemplate {
    id: "task-management",
    source: include_str!("../templates/task-management.json"),
}];

/// Summary of a built-in schema template, returned by `list_templates` so a caller can
/// pick a `template_id` without first fetching every template's full definition.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TemplateSummary {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

fn parse(template: &BuiltinTemplate) -> MetaSchemaDefinition {
    serde_json::from_str(template.source)
        .unwrap_or_else(|err| panic!("built-in template '{}' failed to parse: {err}", template.id))
}

pub fn list_templates() -> Vec<TemplateSummary> {
    TEMPLATES
        .iter()
        .map(|template| {
            let definition = parse(template);
            TemplateSummary {
                id: template.id.to_string(),
                name: definition.name,
                description: definition.description,
            }
        })
        .collect()
}

pub fn get_template(id: &str) -> Result<MetaSchemaDefinition, YorishiroError> {
    TEMPLATES
        .iter()
        .find(|template| template.id == id)
        .map(parse)
        .ok_or_else(|| YorishiroError::NotFound {
            message: format!("no template named '{id}'"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_the_built_in_task_management_template() {
        let templates = list_templates();
        assert!(templates.iter().any(|t| t.id == "task-management"));
    }

    #[test]
    fn fetches_a_template_by_id() {
        let definition = get_template("task-management").unwrap();
        assert_eq!(definition.name, "task-management");
        assert!(definition.entity_types.contains_key("task"));
        assert!(definition.entity_types.contains_key("project"));
    }

    #[test]
    fn reports_not_found_for_an_unknown_template_id() {
        let err = get_template("does-not-exist").unwrap_err();
        assert!(matches!(err, YorishiroError::NotFound { .. }));
    }
}
