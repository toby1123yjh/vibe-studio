use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use uuid::Uuid;
use walkdir::WalkDir;

use super::{claude, codex, gemini, oh_my_pi, opencode, *};
use crate::agent_tools::native_assets;
#[derive(Debug, Clone)]
pub struct AgentCommandService {
    home_dir: PathBuf,
    disabled_root: PathBuf,
}

impl AgentCommandService {
    pub fn new(home_dir: PathBuf, disabled_root: PathBuf) -> Self {
        Self {
            home_dir,
            disabled_root,
        }
    }

    pub fn from_system() -> Result<Self, AgentCommandError> {
        let home_dir = dirs::home_dir().ok_or_else(|| {
            AgentCommandError::InvalidRequest("could not determine the user home directory".into())
        })?;
        Ok(Self::new(
            home_dir,
            workspace_utils::assets::asset_dir().join("agent-commands/disabled/v1"),
        ))
    }

    pub fn manager(
        &self,
        provider: AgentCommandProvider,
        project_path: Option<&Path>,
    ) -> ProviderCommandAssetManager {
        ProviderCommandAssetManager::new(
            provider,
            self.home_dir.clone(),
            project_path.map(Path::to_path_buf),
            self.disabled_root.clone(),
        )
    }

    pub fn discover(&self, project_path: Option<&Path>) -> AgentCommandInventoryView {
        let mut providers = Vec::new();
        let mut errors = Vec::new();
        for provider in AgentCommandProvider::ALL {
            match self.manager(provider, project_path).discover() {
                Ok(inventory) => providers.push(inventory),
                Err(error) => {
                    tracing::warn!(provider = provider.id(), %error, "failed to discover command inventory");
                    errors.push(AgentCommandProviderError {
                        provider,
                        message: "command inventory could not be loaded".into(),
                    });
                    providers.push(AgentCommandProviderInventoryView {
                        provider,
                        installed: false,
                        capabilities: provider_capabilities(provider),
                        items: Vec::new(),
                        limitations: provider_limitations(provider),
                        errors: vec!["command inventory could not be loaded".into()],
                    });
                }
            }
        }
        AgentCommandInventoryView { providers, errors }
    }

    pub fn create(
        &self,
        request: CreateAgentCommandRequest,
    ) -> Result<AgentCommandView, AgentCommandError> {
        validate_locator(&request.target)?;
        ensure_definition_provider(&request.target, &request.definition)?;
        let manager = self.manager_for_locator(&request.target);
        let existing = manager.find(&request.target).ok();
        if let Some(existing) = &existing {
            if !request.replace {
                return Err(AgentCommandError::Collision(request.target.name));
            }
            let expected = request.expected_revision.as_deref().ok_or_else(|| {
                AgentCommandError::InvalidRequest(
                    "replace requires the current target revision".into(),
                )
            })?;
            ensure_revision(&existing.revision, expected)?;
        }
        let definition = request
            .definition
            .resolve(existing.as_ref().map(|item| &item.definition))?;
        manager
            .create(
                request.target,
                definition,
                request.replace,
                request.expected_revision.as_deref(),
            )
            .map(Into::into)
    }

    pub fn update(
        &self,
        request: UpdateAgentCommandRequest,
    ) -> Result<AgentCommandView, AgentCommandError> {
        validate_locator(&request.target)?;
        ensure_definition_provider(&request.target, &request.definition)?;
        let manager = self.manager_for_locator(&request.target);
        let current = manager.find(&request.target)?;
        ensure_revision(&current.revision, &request.expected_revision)?;
        let definition = request.definition.resolve(Some(&current.definition))?;
        manager
            .update(request.target, request.expected_revision, definition)
            .map(Into::into)
    }

    pub fn remove(&self, request: RemoveAgentCommandRequest) -> Result<(), AgentCommandError> {
        validate_locator(&request.target)?;
        self.manager_for_locator(&request.target).remove(request)
    }

    pub fn set_enabled(
        &self,
        request: ToggleAgentCommandRequest,
    ) -> Result<AgentCommandView, AgentCommandError> {
        validate_locator(&request.target)?;
        self.manager_for_locator(&request.target)
            .set_enabled(request)
            .map(Into::into)
    }

    fn manager_for_locator(&self, locator: &AgentCommandLocator) -> ProviderCommandAssetManager {
        self.manager(
            locator.provider,
            locator.project_path.as_deref().map(Path::new),
        )
    }
}

fn ensure_definition_provider(
    target: &AgentCommandLocator,
    definition: &AgentCommandWriteDefinition,
) -> Result<(), AgentCommandError> {
    if target.provider != definition.provider() {
        return Err(AgentCommandError::InvalidRequest(
            "command definition does not belong to the target provider".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct ProviderCommandAssetManager {
    provider: AgentCommandProvider,
    home_dir: PathBuf,
    project_path: Option<PathBuf>,
    disabled_root: PathBuf,
}

impl ProviderCommandAssetManager {
    pub fn new(
        provider: AgentCommandProvider,
        home_dir: PathBuf,
        project_path: Option<PathBuf>,
        disabled_root: PathBuf,
    ) -> Self {
        Self {
            provider,
            home_dir,
            project_path,
            disabled_root,
        }
    }

    pub const fn provider(&self) -> AgentCommandProvider {
        self.provider
    }

    pub fn is_installed(&self) -> bool {
        self.provider_root().exists()
            || workspace_utils::shell::resolve_executable_path_blocking(self.provider.executable())
                .is_some()
    }

    pub fn discover(&self) -> Result<AgentCommandProviderInventoryView, AgentCommandError> {
        let mut items = Vec::new();
        let mut errors = Vec::new();
        for scope in provider_capabilities(self.provider).supported_scopes {
            if scope == AgentCommandScope::Project && self.project_path.is_none() {
                continue;
            }
            if let Err(error) = self.discover_managed(scope, &mut items) {
                tracing::warn!(provider = self.provider.id(), scope = scope.id(), %error, "failed to discover managed commands");
                errors.push(format!("{} commands could not be loaded", scope.id()));
            }
            if self.provider == AgentCommandProvider::OhMyPi
                && let Err(error) = self.discover_omp_executable(scope, &mut items)
            {
                tracing::warn!(provider = self.provider.id(), scope = scope.id(), %error, "failed to discover executable commands");
                errors.push(format!(
                    "{} executable commands could not be loaded",
                    scope.id()
                ));
            }
            if let Err(error) = self.discover_disabled(scope, &mut items) {
                tracing::warn!(provider = self.provider.id(), scope = scope.id(), %error, "failed to discover disabled commands");
                errors.push(format!(
                    "{} disabled commands could not be loaded",
                    scope.id()
                ));
            }
        }
        items.sort_by(|left, right| {
            (
                left.scope.id(),
                left.name.as_str(),
                left.native_path.as_path(),
            )
                .cmp(&(
                    right.scope.id(),
                    right.name.as_str(),
                    right.native_path.as_path(),
                ))
        });
        Ok(AgentCommandProviderInventoryView {
            provider: self.provider,
            installed: self.is_installed(),
            capabilities: provider_capabilities(self.provider),
            items: items.into_iter().map(Into::into).collect(),
            limitations: provider_limitations(self.provider),
            errors,
        })
    }

    pub fn find(&self, locator: &AgentCommandLocator) -> Result<AgentCommand, AgentCommandError> {
        let mut inventory = Vec::new();
        self.discover_managed(locator.scope, &mut inventory)?;
        if self.provider == AgentCommandProvider::OhMyPi {
            self.discover_omp_executable(locator.scope, &mut inventory)?;
        }
        self.discover_disabled(locator.scope, &mut inventory)?;
        inventory
            .into_iter()
            .find(|item| {
                item.scope == locator.scope
                    && item.name == locator.name
                    && locator
                        .installation_id
                        .as_deref()
                        .is_none_or(|id| id == installation_id(item))
            })
            .ok_or_else(|| AgentCommandError::NotFound(locator.name.clone()))
    }

    fn create(
        &self,
        target: AgentCommandLocator,
        definition: AgentCommandDefinition,
        replace: bool,
        expected_revision: Option<&str>,
    ) -> Result<AgentCommand, AgentCommandError> {
        if !self.is_installed() {
            return Err(AgentCommandError::Unsupported(format!(
                "{} is not installed",
                self.provider.id()
            )));
        }
        let format = definition_format(&definition)?;
        let path = self.target_path(target.scope, &target.name, format)?;
        if let Ok(existing) = self.find(&target) {
            if !replace {
                return Err(AgentCommandError::Collision(target.name));
            }
            let expected = expected_revision.ok_or_else(|| {
                AgentCommandError::InvalidRequest(
                    "replace requires the current target revision".into(),
                )
            })?;
            ensure_revision(&existing.revision, expected)?;
            return self.update(target, expected.to_owned(), definition);
        }
        if path.exists() {
            return Err(AgentCommandError::Collision(target.name));
        }
        let rendered = render_definition(format, None, &definition)?;
        atomic_create(&path, rendered.as_bytes())?;
        let mut observed = target;
        observed.installation_id = None;
        self.find(&observed).map_err(|error| {
            AgentCommandError::VerificationFailed(format!(
                "created command was not observable: {error}"
            ))
        })
    }

    fn update(
        &self,
        target: AgentCommandLocator,
        expected_revision: String,
        definition: AgentCommandDefinition,
    ) -> Result<AgentCommand, AgentCommandError> {
        let current = self.find(&target)?;
        ensure_revision(&current.revision, &expected_revision)?;
        if current.state != AgentCommandState::Enabled || !current.capabilities.editable {
            return Err(AgentCommandError::Unsupported(
                "only enabled prompt commands can be edited".into(),
            ));
        }
        if definition_format(&definition)? != current.format {
            return Err(AgentCommandError::InvalidRequest(
                "command definition format does not match the installation".into(),
            ));
        }
        let source = read_command_bytes(&current.native_path)?;
        let source = String::from_utf8(source).map_err(|_| {
            AgentCommandError::InvalidConfiguration("command file is not UTF-8".into())
        })?;
        let rendered = render_definition(current.format, Some(&source), &definition)?;
        atomic_replace(
            &current.native_path,
            rendered.as_bytes(),
            &expected_revision,
        )?;
        let mut observed = target;
        observed.installation_id = None;
        self.find(&observed).map_err(|error| {
            AgentCommandError::VerificationFailed(format!(
                "updated command was not observable: {error}"
            ))
        })
    }

    fn remove(&self, request: RemoveAgentCommandRequest) -> Result<(), AgentCommandError> {
        let current = self.find(&request.target)?;
        ensure_revision(&current.revision, &request.expected_revision)?;
        if !current.capabilities.removable {
            return Err(AgentCommandError::Unsupported(
                "this provider command is read-only".into(),
            ));
        }
        ensure_safe_file(&current.native_path)?;
        ensure_revision(
            &hash_file(&current.native_path)?,
            &request.expected_revision,
        )?;
        fs::remove_file(&current.native_path)?;
        if self.find(&request.target).is_ok() {
            return Err(AgentCommandError::VerificationFailed(
                "removed command is still discoverable".into(),
            ));
        }
        Ok(())
    }

    fn set_enabled(
        &self,
        request: ToggleAgentCommandRequest,
    ) -> Result<AgentCommand, AgentCommandError> {
        let current = self.find(&request.target)?;
        ensure_revision(&current.revision, &request.expected_revision)?;
        if !current.capabilities.toggleable {
            return Err(AgentCommandError::Unsupported(
                "this provider command cannot be toggled".into(),
            ));
        }
        if (current.state == AgentCommandState::Enabled) == request.enabled {
            return Ok(current);
        }
        if request.enabled {
            self.restore_disabled(&current)?;
        } else {
            self.disable(&current)?;
        }
        let expected_state = if request.enabled {
            AgentCommandState::Enabled
        } else {
            AgentCommandState::Disabled
        };
        let mut observed = request.target;
        observed.installation_id = None;
        let mut items = Vec::new();
        self.discover_managed(observed.scope, &mut items)?;
        self.discover_disabled(observed.scope, &mut items)?;
        items
            .into_iter()
            .find(|item| item.name == observed.name && item.state == expected_state)
            .ok_or_else(|| {
                AgentCommandError::VerificationFailed(
                    "toggled command was not observable in the requested state".into(),
                )
            })
    }

    fn provider_root(&self) -> PathBuf {
        match self.provider {
            AgentCommandProvider::Codex => self.home_dir.join(".codex"),
            AgentCommandProvider::ClaudeCode => self.home_dir.join(".claude"),
            AgentCommandProvider::Gemini => self.home_dir.join(".gemini"),
            AgentCommandProvider::OhMyPi => self.home_dir.join(".omp"),
            AgentCommandProvider::Opencode => native_assets::opencode_root_for(&self.home_dir),
            AgentCommandProvider::DeepseekHarness => {
                native_assets::deepseek_home_for(&self.home_dir)
            }
        }
    }

    fn project_root(&self) -> Result<&Path, AgentCommandError> {
        let path = self.project_path.as_deref().ok_or_else(|| {
            AgentCommandError::InvalidRequest("project scope requires project_path".into())
        })?;
        if !path.is_absolute() {
            return Err(AgentCommandError::UnsafePath(
                "project_path must be absolute".into(),
            ));
        }
        if !path.is_dir() {
            return Err(AgentCommandError::InvalidRequest(
                "project directory does not exist".into(),
            ));
        }
        ensure_directory_not_symlink(path)?;
        Ok(path)
    }

    pub(super) fn managed_root(
        &self,
        scope: AgentCommandScope,
    ) -> Result<PathBuf, AgentCommandError> {
        let project_root = if scope == AgentCommandScope::Project {
            Some(self.project_root()?)
        } else {
            None
        };
        match self.provider {
            AgentCommandProvider::Codex => codex::managed_root(&self.home_dir, scope),
            AgentCommandProvider::ClaudeCode => {
                claude::managed_root(&self.home_dir, project_root, scope)
            }
            AgentCommandProvider::Gemini => {
                gemini::managed_root(&self.home_dir, project_root, scope)
            }
            AgentCommandProvider::OhMyPi => {
                oh_my_pi::managed_root(&self.home_dir, project_root, scope)
            }
            AgentCommandProvider::Opencode => {
                opencode::managed_root(&self.home_dir, project_root, scope)
            }
            AgentCommandProvider::DeepseekHarness => Err(AgentCommandError::Unsupported(
                "DeepSeek Harness ACP has no native slash-command assets".into(),
            )),
        }
    }

    pub(super) fn executable_root(
        &self,
        scope: AgentCommandScope,
    ) -> Result<PathBuf, AgentCommandError> {
        let project_root = if scope == AgentCommandScope::Project {
            Some(self.project_root()?)
        } else {
            None
        };
        oh_my_pi::executable_root(&self.home_dir, project_root, scope)
    }

    fn managed_format(&self) -> Result<AgentCommandFormat, AgentCommandError> {
        match self.provider {
            AgentCommandProvider::Codex => Ok(AgentCommandFormat::CodexLegacyMarkdown),
            AgentCommandProvider::ClaudeCode => Ok(AgentCommandFormat::ClaudeMarkdown),
            AgentCommandProvider::Gemini => Ok(AgentCommandFormat::GeminiToml),
            AgentCommandProvider::OhMyPi => Ok(AgentCommandFormat::OhMyPiPromptMarkdown),
            AgentCommandProvider::Opencode => Ok(AgentCommandFormat::OpencodeMarkdown),
            AgentCommandProvider::DeepseekHarness => Err(AgentCommandError::Unsupported(
                "DeepSeek Harness ACP has no native slash-command assets".into(),
            )),
        }
    }

    fn target_path(
        &self,
        scope: AgentCommandScope,
        name: &str,
        format: AgentCommandFormat,
    ) -> Result<PathBuf, AgentCommandError> {
        if !provider_capabilities(self.provider)
            .writable_formats
            .contains(&format)
        {
            return Err(AgentCommandError::Unsupported(
                "the provider does not support this command format".into(),
            ));
        }
        let relative = relative_path_for_name(self.provider, name, format)?;
        Ok(self.managed_root(scope)?.join(relative))
    }

    fn discover_managed(
        &self,
        scope: AgentCommandScope,
        output: &mut Vec<AgentCommand>,
    ) -> Result<(), AgentCommandError> {
        let canonical_root = self.managed_root(scope)?;
        let roots = if self.provider == AgentCommandProvider::Opencode {
            opencode::discovery_roots(&self.home_dir, self.project_path.as_deref(), scope)?
        } else {
            vec![canonical_root.clone()]
        };
        for root in roots {
            let format = self.managed_format()?;
            if !root.exists() {
                continue;
            }
            ensure_directory_not_symlink(&root)?;
            let walker = if matches!(
                self.provider,
                AgentCommandProvider::Codex
                    | AgentCommandProvider::OhMyPi
                    | AgentCommandProvider::Opencode
            ) {
                WalkDir::new(&root).max_depth(1)
            } else {
                WalkDir::new(&root)
            };
            for entry in walker.follow_links(false) {
                let entry = entry.map_err(|_| {
                    AgentCommandError::InvalidConfiguration(
                        "failed to scan command directory".into(),
                    )
                })?;
                if entry.file_type().is_symlink() {
                    continue;
                }
                if ensure_no_link_components(entry.path()).is_err() {
                    continue;
                }
                if !entry.file_type().is_file()
                    || entry.path().extension().and_then(|value| value.to_str())
                        != Some(format.extension())
                {
                    continue;
                }
                let relative = entry.path().strip_prefix(&root).map_err(|_| {
                    AgentCommandError::UnsafePath("command escaped its provider root".into())
                })?;
                let name = display_name(self.provider, relative)?;
                let mut item =
                    read_enabled_item(self.provider, scope, name, format, entry.path(), relative);
                if root != canonical_root {
                    item.capabilities.toggleable = false;
                }
                output.push(item);
            }
        }
        if self.provider == AgentCommandProvider::Opencode {
            self.discover_opencode_inline(scope, output)?;
        }
        Ok(())
    }

    fn discover_opencode_inline(
        &self,
        scope: AgentCommandScope,
        output: &mut Vec<AgentCommand>,
    ) -> Result<(), AgentCommandError> {
        for path in
            opencode::config_candidates(&self.home_dir, self.project_path.as_deref(), scope)?
        {
            if !path.is_file() {
                continue;
            }
            ensure_safe_file(&path)?;
            let bytes = read_command_bytes(&path)?;
            let source = std::str::from_utf8(&bytes).map_err(|_| {
                AgentCommandError::InvalidConfiguration("native config is not UTF-8".into())
            })?;
            let value =
                jsonc_parser::parse_to_serde_value(source, &jsonc_parser::ParseOptions::default())
                    .map_err(|_| {
                        AgentCommandError::InvalidConfiguration(
                            "native command config is invalid".into(),
                        )
                    })?
                    .unwrap_or_default();
            let Some(commands) = value.get("command").and_then(serde_json::Value::as_object) else {
                continue;
            };
            for (name, native) in commands {
                if validate_name(self.provider, name).is_err() {
                    continue;
                }
                let definition = native
                    .get("template")
                    .and_then(serde_json::Value::as_str)
                    .map(|body| AgentCommandDefinition::Opencode {
                        description: native
                            .get("description")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string),
                        body: body.to_string(),
                    })
                    .unwrap_or(AgentCommandDefinition::Invalid {
                        content_configured: true,
                    });
                output.push(AgentCommand {
                    provider: self.provider,
                    scope,
                    name: name.clone(),
                    state: if matches!(&definition, AgentCommandDefinition::Invalid { .. }) {
                        AgentCommandState::Error
                    } else {
                        AgentCommandState::Enabled
                    },
                    format: AgentCommandFormat::OpencodeInline,
                    capabilities: AgentCommandCapabilities::read_only(),
                    revision: hash_bytes(&bytes),
                    definition,
                    native_path: path.clone(),
                    relative_path: PathBuf::new(),
                    error: None,
                });
            }
        }
        Ok(())
    }

    fn discover_omp_executable(
        &self,
        scope: AgentCommandScope,
        output: &mut Vec<AgentCommand>,
    ) -> Result<(), AgentCommandError> {
        let root = self.executable_root(scope)?;
        if !root.exists() {
            return Ok(());
        }
        ensure_directory_not_symlink(&root)?;
        for entry in fs::read_dir(&root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() || entry.file_name().to_string_lossy().starts_with('.')
            {
                continue;
            }
            let command_dir = entry.path();
            if ensure_directory_not_symlink(&command_dir).is_err() {
                continue;
            }
            let entrypoint = ["index.ts", "index.js", "index.mjs", "index.cjs"]
                .into_iter()
                .map(|file| command_dir.join(file))
                .find(|path| ensure_safe_file(path).is_ok());
            let Some(entrypoint) = entrypoint else {
                continue;
            };
            let relative = entrypoint
                .strip_prefix(&root)
                .map_err(|_| {
                    AgentCommandError::UnsafePath("command escaped its provider root".into())
                })?
                .to_path_buf();
            let name = entry.file_name().to_string_lossy().to_string();
            output.push(AgentCommand {
                provider: self.provider,
                scope,
                name,
                state: AgentCommandState::Unsupported,
                format: AgentCommandFormat::OhMyPiExecutableModule,
                capabilities: AgentCommandCapabilities::read_only(),
                revision: hash_file(&entrypoint).unwrap_or_default(),
                definition: AgentCommandDefinition::OhMyPiExecutable {
                    entrypoint_configured: true,
                },
                native_path: entrypoint,
                relative_path: relative,
                error: Some(
                    "Executable TypeScript commands are discovered read-only; Vibe Studio does not execute or expose their source.".into(),
                ),
            });
        }
        Ok(())
    }

    fn disabled_dir(&self, scope: AgentCommandScope) -> PathBuf {
        self.disabled_root.join(self.provider.id()).join(scope.id())
    }

    fn discover_disabled(
        &self,
        scope: AgentCommandScope,
        output: &mut Vec<AgentCommand>,
    ) -> Result<(), AgentCommandError> {
        let root = self.disabled_dir(scope);
        if !root.exists() {
            return Ok(());
        }
        ensure_directory_not_symlink(&root)?;
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            if !entry.file_type()?.is_file()
                || entry.path().extension().and_then(|value| value.to_str()) != Some("json")
            {
                continue;
            }
            match read_disabled_record(&entry.path()) {
                Ok(record) if record.provider == self.provider && record.scope == scope => {
                    let content = BASE64.decode(&record.content_base64).map_err(|error| {
                        AgentCommandError::InvalidConfiguration(format!(
                            "disabled command content is invalid base64: {error}"
                        ))
                    })?;
                    let definition = parse_definition(record.format, &content).unwrap_or(
                        AgentCommandDefinition::Invalid {
                            content_configured: !content.is_empty(),
                        },
                    );
                    output.push(AgentCommand {
                        provider: record.provider,
                        scope: record.scope,
                        name: record.name,
                        state: AgentCommandState::Disabled,
                        format: record.format,
                        capabilities: AgentCommandCapabilities::managed(),
                        revision: hash_file(&entry.path())?,
                        definition,
                        native_path: entry.path(),
                        relative_path: safe_relative_path(&record.relative_path)?,
                        error: None,
                    });
                }
                Ok(_) => {}
                Err(_error) => output.push(AgentCommand {
                    provider: self.provider,
                    scope,
                    name: entry.file_name().to_string_lossy().to_string(),
                    state: AgentCommandState::Error,
                    format: self.managed_format()?,
                    capabilities: AgentCommandCapabilities::read_only(),
                    revision: hash_file(&entry.path()).unwrap_or_default(),
                    definition: AgentCommandDefinition::Invalid {
                        content_configured: entry
                            .metadata()
                            .is_ok_and(|metadata| metadata.len() > 0),
                    },
                    native_path: entry.path(),
                    relative_path: PathBuf::new(),
                    error: Some("disabled command record is invalid".into()),
                }),
            }
        }
        Ok(())
    }

    fn disable(&self, current: &AgentCommand) -> Result<(), AgentCommandError> {
        ensure_safe_file(&current.native_path)?;
        let content = read_command_bytes(&current.native_path)?;
        ensure_revision(&hash_bytes(&content), &current.revision)?;
        let record = DisabledCommandRecord {
            version: DISABLED_STORE_VERSION,
            provider: current.provider,
            scope: current.scope,
            name: current.name.clone(),
            format: current.format,
            relative_path: portable_path(&current.relative_path),
            content_base64: BASE64.encode(content),
        };
        let manifest = self
            .disabled_dir(current.scope)
            .join(format!("{}.json", Uuid::new_v4()));
        atomic_write_json(&manifest, &record)?;
        if let Err(error) = ensure_revision(&hash_file(&current.native_path)?, &current.revision) {
            let _ = fs::remove_file(&manifest);
            return Err(error);
        }
        if let Err(error) = fs::remove_file(&current.native_path) {
            let _ = fs::remove_file(&manifest);
            return Err(error.into());
        }
        Ok(())
    }

    fn restore_disabled(&self, current: &AgentCommand) -> Result<(), AgentCommandError> {
        ensure_safe_file(&current.native_path)?;
        ensure_revision(&hash_file(&current.native_path)?, &current.revision)?;
        let record = read_disabled_record(&current.native_path)?;
        if record.provider != self.provider || record.scope != current.scope {
            return Err(AgentCommandError::InvalidConfiguration(
                "disabled command ownership does not match the request".into(),
            ));
        }
        let relative = safe_relative_path(&record.relative_path)?;
        let root = self.managed_root(record.scope)?;
        let target = root.join(&relative);
        if target.exists() {
            return Err(AgentCommandError::Collision(record.name));
        }
        let content = BASE64.decode(&record.content_base64).map_err(|error| {
            AgentCommandError::InvalidConfiguration(format!(
                "disabled command content is invalid base64: {error}"
            ))
        })?;
        parse_definition(record.format, &content)?;
        atomic_create(&target, &content)?;
        if let Err(error) = ensure_revision(&hash_file(&current.native_path)?, &current.revision) {
            remove_if_revision(&target, &hash_bytes(&content));
            return Err(error);
        }
        if let Err(error) = fs::remove_file(&current.native_path) {
            remove_if_revision(&target, &hash_bytes(&content));
            return Err(error.into());
        }
        Ok(())
    }
}

fn remove_if_revision(path: &Path, expected_revision: &str) {
    if hash_file(path).is_ok_and(|revision| revision == expected_revision) {
        let _ = fs::remove_file(path);
    }
}

fn provider_limitations(provider: AgentCommandProvider) -> Vec<String> {
    vec![
        match provider {
            AgentCommandProvider::Codex => codex::LIMITATION,
            AgentCommandProvider::ClaudeCode => claude::LIMITATION,
            AgentCommandProvider::Gemini => gemini::LIMITATION,
            AgentCommandProvider::OhMyPi => oh_my_pi::LIMITATION,
            AgentCommandProvider::Opencode => opencode::LIMITATION,
            AgentCommandProvider::DeepseekHarness => "DeepSeek Harness ACP does not expose native slash commands or a user command-file directory. Ordinary prompt text is supported; no command assets are invented.",
        }
        .into(),
    ]
}

fn validate_locator(locator: &AgentCommandLocator) -> Result<(), AgentCommandError> {
    validate_name(locator.provider, &locator.name)?;
    if !provider_capabilities(locator.provider)
        .supported_scopes
        .contains(&locator.scope)
    {
        return Err(AgentCommandError::Unsupported(format!(
            "{} does not support {} command scope",
            locator.provider.id(),
            locator.scope.id()
        )));
    }
    if locator.scope == AgentCommandScope::Project {
        let project = locator.project_path.as_deref().ok_or_else(|| {
            AgentCommandError::InvalidRequest("project scope requires project_path".into())
        })?;
        if !Path::new(project).is_absolute() {
            return Err(AgentCommandError::UnsafePath(
                "project_path must be absolute".into(),
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_name(
    provider: AgentCommandProvider,
    name: &str,
) -> Result<(), AgentCommandError> {
    let segments: Vec<_> = name.split(':').collect();
    if name.is_empty()
        || (matches!(
            provider,
            AgentCommandProvider::Codex
                | AgentCommandProvider::OhMyPi
                | AgentCommandProvider::Opencode
                | AgentCommandProvider::DeepseekHarness
        ) && segments.len() != 1)
        || segments.iter().any(|segment| {
            segment.is_empty()
                || !segment.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
                })
        })
    {
        return Err(AgentCommandError::UnsafePath(
            "command names use ASCII letters, numbers, '-' and '_'; OpenCode uses flat names, while Claude Code and Gemini allow ':' namespaces"
                .into(),
        ));
    }
    Ok(())
}

fn relative_path_for_name(
    provider: AgentCommandProvider,
    name: &str,
    format: AgentCommandFormat,
) -> Result<PathBuf, AgentCommandError> {
    validate_name(provider, name)?;
    let mut path = PathBuf::new();
    let segments: Vec<_> = name.split(':').collect();
    for segment in &segments[..segments.len() - 1] {
        path.push(segment);
    }
    path.push(format!(
        "{}.{}",
        segments.last().copied().unwrap_or_default(),
        format.extension()
    ));
    Ok(path)
}

fn display_name(
    provider: AgentCommandProvider,
    relative: &Path,
) -> Result<String, AgentCommandError> {
    let mut segments = Vec::new();
    for component in relative.components() {
        let Component::Normal(value) = component else {
            return Err(AgentCommandError::UnsafePath(
                "command path contains unsafe components".into(),
            ));
        };
        let value = value.to_str().ok_or_else(|| {
            AgentCommandError::UnsafePath("command path is not valid UTF-8".into())
        })?;
        segments.push(value.to_owned());
    }
    let last = segments
        .last_mut()
        .ok_or_else(|| AgentCommandError::UnsafePath("command path has no filename".into()))?;
    *last = Path::new(last)
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| AgentCommandError::UnsafePath("invalid command filename".into()))?
        .to_owned();
    let name = if provider == AgentCommandProvider::OhMyPi {
        last.clone()
    } else {
        segments.join(":")
    };
    validate_name(provider, &name)?;
    Ok(name)
}

fn definition_format(
    definition: &AgentCommandDefinition,
) -> Result<AgentCommandFormat, AgentCommandError> {
    match definition {
        AgentCommandDefinition::CodexLegacy { .. } => Ok(AgentCommandFormat::CodexLegacyMarkdown),
        AgentCommandDefinition::ClaudeCode { .. } => Ok(AgentCommandFormat::ClaudeMarkdown),
        AgentCommandDefinition::Gemini { .. } => Ok(AgentCommandFormat::GeminiToml),
        AgentCommandDefinition::OhMyPiPrompt { .. } => Ok(AgentCommandFormat::OhMyPiPromptMarkdown),
        AgentCommandDefinition::Opencode { .. } => Ok(AgentCommandFormat::OpencodeMarkdown),
        AgentCommandDefinition::OhMyPiExecutable { .. } => {
            Ok(AgentCommandFormat::OhMyPiExecutableModule)
        }
        AgentCommandDefinition::Invalid { .. } => Err(AgentCommandError::InvalidConfiguration(
            "invalid command content cannot be written".into(),
        )),
    }
}

fn read_enabled_item(
    provider: AgentCommandProvider,
    scope: AgentCommandScope,
    name: String,
    format: AgentCommandFormat,
    path: &Path,
    relative_path: &Path,
) -> AgentCommand {
    let revision = hash_file(path).unwrap_or_default();
    match read_command_bytes(path).and_then(|bytes| parse_definition(format, &bytes)) {
        Ok(definition) => AgentCommand {
            provider,
            scope,
            name,
            state: AgentCommandState::Enabled,
            format,
            capabilities: AgentCommandCapabilities::managed(),
            revision,
            definition,
            native_path: path.to_path_buf(),
            relative_path: relative_path.to_path_buf(),
            error: None,
        },
        Err(_error) => AgentCommand {
            provider,
            scope,
            name,
            state: AgentCommandState::Error,
            format,
            capabilities: AgentCommandCapabilities::read_only(),
            revision,
            definition: AgentCommandDefinition::Invalid {
                content_configured: path.metadata().is_ok_and(|metadata| metadata.len() > 0),
            },
            native_path: path.to_path_buf(),
            relative_path: relative_path.to_path_buf(),
            error: Some("command definition is invalid".into()),
        },
    }
}

fn parse_definition(
    format: AgentCommandFormat,
    bytes: &[u8],
) -> Result<AgentCommandDefinition, AgentCommandError> {
    match format {
        AgentCommandFormat::CodexLegacyMarkdown => codex::parse(bytes),
        AgentCommandFormat::ClaudeMarkdown => claude::parse(bytes),
        AgentCommandFormat::GeminiToml => gemini::parse(bytes),
        AgentCommandFormat::OhMyPiPromptMarkdown => oh_my_pi::parse_prompt(bytes),
        AgentCommandFormat::OpencodeMarkdown => opencode::parse(bytes),
        AgentCommandFormat::OpencodeInline => Err(AgentCommandError::Unsupported(
            "inline OpenCode commands are discovery-only".into(),
        )),
        AgentCommandFormat::OhMyPiExecutableModule => {
            Ok(AgentCommandDefinition::OhMyPiExecutable {
                entrypoint_configured: !bytes.is_empty(),
            })
        }
    }
}

fn render_definition(
    format: AgentCommandFormat,
    current_source: Option<&str>,
    definition: &AgentCommandDefinition,
) -> Result<String, AgentCommandError> {
    match format {
        AgentCommandFormat::CodexLegacyMarkdown => codex::render(current_source, definition),
        AgentCommandFormat::ClaudeMarkdown => claude::render(current_source, definition),
        AgentCommandFormat::GeminiToml => gemini::render(current_source, definition),
        AgentCommandFormat::OhMyPiPromptMarkdown => {
            oh_my_pi::render_prompt(current_source, definition)
        }
        AgentCommandFormat::OpencodeMarkdown => opencode::render(current_source, definition),
        AgentCommandFormat::OpencodeInline => Err(AgentCommandError::Unsupported(
            "inline OpenCode commands are discovery-only".into(),
        )),
        AgentCommandFormat::OhMyPiExecutableModule => Err(AgentCommandError::Unsupported(
            "executable Oh My Pi commands are read-only".into(),
        )),
    }
}
