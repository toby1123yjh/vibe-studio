use std::path::{Path, PathBuf};

use super::{
    AgentCommandDefinition, AgentCommandError, AgentCommandFormat,
    AgentCommandProviderCapabilities, AgentCommandScope, markdown,
};
use crate::agent_tools::native_assets;

pub(super) fn capabilities() -> AgentCommandProviderCapabilities {
    AgentCommandProviderCapabilities {
        discoverable: true,
        creatable: true,
        supported_scopes: vec![AgentCommandScope::User, AgentCommandScope::Project],
        writable_formats: vec![AgentCommandFormat::OpencodeMarkdown],
    }
}

pub(super) fn managed_root(
    home: &Path,
    project: Option<&Path>,
    scope: AgentCommandScope,
) -> Result<PathBuf, AgentCommandError> {
    match scope {
        AgentCommandScope::User => {
            Ok(native_assets::opencode_asset_root_for(home).join("commands"))
        }
        AgentCommandScope::Project => Ok(project
            .ok_or_else(|| {
                AgentCommandError::InvalidRequest("project scope requires project_path".into())
            })?
            .join(".opencode/commands")),
    }
}

pub(super) fn discovery_roots(
    home: &Path,
    project: Option<&Path>,
    scope: AgentCommandScope,
) -> Result<Vec<PathBuf>, AgentCommandError> {
    let roots = match scope {
        AgentCommandScope::User => native_assets::opencode_config_roots(home),
        AgentCommandScope::Project => vec![
            project
                .ok_or_else(|| {
                    AgentCommandError::InvalidRequest("project scope requires project_path".into())
                })?
                .join(".opencode"),
        ],
    };
    Ok(roots
        .into_iter()
        .flat_map(|root| [root.join("commands"), root.join("command")])
        .collect())
}

pub(super) fn config_candidates(
    home: &Path,
    project: Option<&Path>,
    scope: AgentCommandScope,
) -> Result<Vec<PathBuf>, AgentCommandError> {
    match scope {
        AgentCommandScope::User => {
            let mut candidates: Vec<_> = native_assets::opencode_config_roots(home)
                .into_iter()
                .flat_map(|root| native_assets::opencode_config_candidates(&root, true))
                .collect();
            if let Some(path) = native_assets::opencode_custom_config(home) {
                if !candidates.contains(&path) {
                    candidates.push(path);
                }
            }
            Ok(candidates)
        }
        AgentCommandScope::Project => {
            let project = project.ok_or_else(|| {
                AgentCommandError::InvalidRequest("project scope requires project_path".into())
            })?;
            Ok(native_assets::opencode_config_candidates(project, false)
                .into_iter()
                .chain(native_assets::opencode_config_candidates(
                    &project.join(".opencode"),
                    false,
                ))
                .collect())
        }
    }
}

pub(super) fn parse(bytes: &[u8]) -> Result<AgentCommandDefinition, AgentCommandError> {
    let content = std::str::from_utf8(bytes)
        .map_err(|_| AgentCommandError::InvalidConfiguration("command file is not UTF-8".into()))?;
    let parsed = markdown::parse(content)?;
    Ok(AgentCommandDefinition::Opencode {
        description: markdown::string_field(parsed.frontmatter, "description")?,
        body: parsed.body.to_owned(),
    })
}

pub(super) fn render(
    source: Option<&str>,
    definition: &AgentCommandDefinition,
) -> Result<String, AgentCommandError> {
    let AgentCommandDefinition::Opencode { description, body } = definition else {
        return Err(AgentCommandError::InvalidRequest(
            "definition does not match OpenCode command format".into(),
        ));
    };
    // Only managed fields change. Native agent/model/subtask, placeholders,
    // shell substitutions and every unknown frontmatter field remain literal.
    markdown::render(source, &[("description", description.as_deref())], body)
}

pub(super) const LIMITATION: &str = "OpenCode Markdown commands are managed natively. Inline JSONC commands are shown read-only with separate provenance; precedence depends on native config-directory/inline layers. Legacy alias commands cannot be moved into disabled storage. Templates are never evaluated by Vibe Studio. ACP does not support /undo or /redo.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_command_edits_preserve_provider_fields_and_literal_templates() {
        let source = "---\ndescription: old\nagent: build\nmodel: vendor/model\nsubtask: true\n# retained\n---\n!`git status` $ARGUMENTS @README.md";
        let definition = AgentCommandDefinition::Opencode {
            description: Some("new".into()),
            body: "!`git status` $ARGUMENTS @README.md".into(),
        };
        let rendered = render(Some(source), &definition).unwrap();
        assert!(rendered.contains("agent: build"));
        assert!(rendered.contains("model: vendor/model"));
        assert!(rendered.contains("subtask: true"));
        assert!(rendered.contains("# retained"));
        assert!(rendered.ends_with("!`git status` $ARGUMENTS @README.md"));
        assert_eq!(parse(rendered.as_bytes()).unwrap(), definition);
    }
}
