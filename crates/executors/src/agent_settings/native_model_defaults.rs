//! Read-only native inheritance for picker discovery. No CLI sessions or writes.

use std::{collections::HashMap, ffi::OsString};

use super::*;
use crate::{
    command::CmdOverrides, executors::oh_my_pi::oh_my_pi_agent_root_with_env,
    model_selector::NativeModelDefaults,
};

/// Native defaults belong to the same command, environment and directory as
/// execution. An arbitrary wrapper or extra CLI arguments may redirect native
/// authority in ways that file discovery cannot safely determine.
pub fn native_model_defaults_for(
    provider: DirectProvider,
    workdir: Option<&Path>,
    cmd: &CmdOverrides,
) -> Option<NativeModelDefaults> {
    if cmd.base_command_override.is_some()
        || cmd
            .additional_params
            .as_ref()
            .is_some_and(|params| !params.is_empty())
        || cmd.env.as_ref().is_some_and(|env| {
            env.keys().any(|key| {
                key.eq_ignore_ascii_case("NODE_OPTIONS") || key.eq_ignore_ascii_case("PATH")
            })
        })
    {
        return None;
    }
    let cwd = workdir
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok())?;
    if !cwd.is_absolute() || !cwd.is_dir() {
        return None;
    }
    native_defaults_with_environment(provider, &cwd, &|name| {
        profile_environment(cmd.env.as_ref(), name).or_else(|| std::env::var(name).ok())
    })
}

fn profile_environment(env: Option<&HashMap<String, String>>, name: &str) -> Option<String> {
    let env = env?;
    env.get(name).cloned().or_else(|| {
        cfg!(windows)
            .then(|| {
                env.iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case(name))
                    .map(|(_, value)| value.clone())
            })
            .flatten()
    })
}

fn configured_path(value: Option<String>, home: &Path, cwd: &Path) -> Option<PathBuf> {
    let value = value.filter(|value| !value.trim().is_empty())?;
    let path = if value == "~" {
        home.to_path_buf()
    } else if let Some(rest) = value
        .strip_prefix("~/")
        .or_else(|| value.strip_prefix("~\\"))
    {
        home.join(rest)
    } else {
        PathBuf::from(value)
    };
    Some(if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    })
}

fn native_defaults_with_environment(
    provider: DirectProvider,
    cwd: &Path,
    env: &impl Fn(&str) -> Option<String>,
) -> Option<NativeModelDefaults> {
    let home = env(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .or_else(dirs::home_dir)?;
    if !home.is_absolute() {
        return None;
    }
    let path = |name| configured_path(env(name), &home, cwd);
    let user_root = match provider {
        DirectProvider::Codex => path("CODEX_HOME").unwrap_or_else(|| home.join(".codex")),
        DirectProvider::ClaudeCode => {
            path("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude"))
        }
        DirectProvider::Gemini => path("GEMINI_CLI_HOME")
            .unwrap_or_else(|| home.clone())
            .join(".gemini"),
        DirectProvider::OhMyPi => {
            let root = oh_my_pi_agent_root_with_env(&home, |name| env(name).map(OsString::from));
            if root.is_absolute() {
                root
            } else {
                cwd.join(root)
            }
        }
        DirectProvider::Opencode => path("XDG_CONFIG_HOME")
            .unwrap_or_else(|| home.join(".config"))
            .join("opencode"),
        DirectProvider::DeepseekHarness => path("DSH_HOME").unwrap_or_else(|| home.join(".dsh")),
    };
    let manager = ProviderSettingsManager {
        provider: provider.into(),
        home_dir: home.clone(),
        user_root,
        project_path: Some(cwd.to_path_buf()),
    };
    let mut specs = match provider {
        DirectProvider::Opencode => opencode_defaults_files(&manager, env, cwd),
        DirectProvider::DeepseekHarness => manager.deepseek_config_file_specs(),
        _ => manager.file_specs(),
    };
    if provider == DirectProvider::ClaudeCode {
        specs.push(file_spec(
            "project_local_settings",
            cwd.join(".claude/settings.local.json"),
            NativeConfigFormat::Json,
            SettingScope::Project,
        ));
    }
    if provider == DirectProvider::Gemini {
        // The native system-defaults layer precedes user/project; the system
        // override layer comes last. Do not guess platform-specific locations.
        if let Some(path) = path("GEMINI_CLI_SYSTEM_DEFAULTS_PATH") {
            specs.insert(
                0,
                file_spec(
                    "system_defaults",
                    path,
                    NativeConfigFormat::Json,
                    SettingScope::User,
                ),
            );
        }
        if let Some(path) = path("GEMINI_CLI_SYSTEM_SETTINGS_PATH") {
            specs.push(file_spec(
                "system_settings",
                path,
                NativeConfigFormat::Json,
                SettingScope::User,
            ));
        }
    }
    let files: Vec<_> = specs
        .into_iter()
        // Credentials and .env secret maps are unnecessary to read model
        // defaults. Each native settings read is bounded, unlike transcripts.
        .filter(|spec| {
            !matches!(
                spec.id.as_str(),
                "user_auth"
                    | "user_credentials"
                    | "user_models"
                    | "user_env"
                    | "project_env"
                    | "home_env"
                    | "launch_environment"
            )
        })
        .map(|spec| {
            let bounded = spec.inline_content.as_ref().map_or_else(
                || fs::metadata(&spec.path).map_or(true, |meta| meta.len() <= 2 * 1024 * 1024),
                |content| content.len() <= 2 * 1024 * 1024,
            );
            bounded.then(|| read_native_file(spec))
        })
        .collect::<Option<Vec<_>>>()?;
    // Malformed native settings can invalidate inheritance. Keep it unknown
    // rather than presenting an older lower-priority value as effective.
    if files.iter().any(|file| file.error.is_some()) {
        return None;
    }
    let descriptors = manager.descriptors_for_files(&files);
    let setting = |key: &str| {
        descriptors
            .iter()
            .find(|descriptor| descriptor.key.id() == key)
            .and_then(|descriptor| {
                effective_setting(descriptor, &descriptors, &files, false).effective_value
            })
            .and_then(|value| {
                value
                    .as_str()
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_owned)
            })
    };
    let mut defaults = NativeModelDefaults {
        model_id: setting("common.model"),
        reasoning_id: setting("common.reasoning").or_else(|| setting("common.reasoning_effort")),
        ..Default::default()
    };
    match provider {
        DirectProvider::Codex => {
            if let Some(profile) = effective_string_at_path(&files, &["profile"]) {
                defaults.model_id =
                    effective_string_at_path(&files, &["profiles", &profile, "model"])
                        .or(defaults.model_id);
                defaults.reasoning_id = effective_string_at_path(
                    &files,
                    &["profiles", &profile, "model_reasoning_effort"],
                )
                .or(defaults.reasoning_id);
            }
        }
        DirectProvider::ClaudeCode => {
            defaults.model_id = env("ANTHROPIC_MODEL")
                .filter(|value| !value.trim().is_empty())
                .or_else(|| effective_string_at_path(&files, &["env", "ANTHROPIC_MODEL"]))
                .or_else(|| effective_string_at_path(&files, &["model"]));
            defaults.reasoning_id = effective_string_at_path(&files, &["effortLevel"]);
            for file in &files {
                if let Some(models) = file
                    .value
                    .as_ref()
                    .and_then(|value| value["modelSettings"].as_object())
                {
                    for (model, settings) in models {
                        if let Some(effort) = settings["effortLevel"]
                            .as_str()
                            .filter(|effort| !effort.trim().is_empty())
                        {
                            defaults
                                .model_reasoning
                                .insert(model.clone(), effort.to_string());
                        }
                    }
                }
            }
            if let Some(effort) = env("CLAUDE_CODE_EFFORT_LEVEL")
                .filter(|value| !value.trim().is_empty())
                .or_else(|| effective_string_at_path(&files, &["env", "CLAUDE_CODE_EFFORT_LEVEL"]))
            {
                defaults.reasoning_id = Some(effort);
                // Native environment effort overrides every per-model default.
                defaults.model_reasoning.clear();
            }
        }
        DirectProvider::Gemini => {
            defaults.model_id = effective_string_at_path(&files, &["model", "name"]);
            defaults.reasoning_id = None;
        }
        DirectProvider::OhMyPi => {
            // Verified native settings key; do not confuse RPC thinkingLevel
            // (runtime state) with persistent defaultThinkingLevel.
            defaults.reasoning_id = effective_string_at_path(&files, &["defaultThinkingLevel"]);
        }
        DirectProvider::DeepseekHarness => {
            // llm-deepseek owns only the API-key deepseek-official adapter.
            // Other ACP routes (including account and pi-ai providers) have
            // independent defaults. Bind this default to the exact model pair.
            let effort = defaults.reasoning_id.take();
            if effective_string_at_path(&files, &["acp", "config", "provider"]).as_deref()
                == Some("deepseek-official")
                && let Some(model) = defaults.model_id.as_ref()
                && let Some(effort) = effort
            {
                defaults.model_reasoning.insert(model.clone(), effort);
            }
        }
        DirectProvider::Opencode => {}
    }
    (defaults.model_id.is_some()
        || defaults.reasoning_id.is_some()
        || !defaults.model_reasoning.is_empty())
    .then_some(defaults)
}

fn opencode_defaults_files(
    manager: &ProviderSettingsManager,
    env: &impl Fn(&str) -> Option<String>,
    cwd: &Path,
) -> Vec<NativeFileSpec> {
    let mut files = Vec::new();
    manager.push_opencode_directory(
        &mut files,
        &manager.user_root,
        "user",
        SettingScope::User,
        true,
    );
    let path = |name| configured_path(env(name), &manager.home_dir, cwd);
    if let Some(path) = path("OPENCODE_CONFIG") {
        files.push(file_spec(
            "custom_config",
            path,
            NativeConfigFormat::Jsonc,
            SettingScope::User,
        ));
    }
    if env("OPENCODE_DISABLE_PROJECT_CONFIG").as_deref() != Some("1")
        && env("OPENCODE_DISABLE_PROJECT_CONFIG").as_deref() != Some("true")
    {
        let root = native_assets::deepseek_project_root(cwd);
        let ancestors: Vec<_> = cwd
            .ancestors()
            .take_while(|path| path.starts_with(root))
            .collect();
        for (index, ancestor) in ancestors.iter().rev().enumerate() {
            manager.push_opencode_directory(
                &mut files,
                ancestor,
                &format!("project_{index}"),
                SettingScope::Project,
                false,
            );
        }
        for (index, ancestor) in ancestors.iter().rev().enumerate() {
            manager.push_opencode_directory(
                &mut files,
                &ancestor.join(".opencode"),
                &format!("asset_{index}"),
                SettingScope::Project,
                false,
            );
        }
    }
    if let Some(root) = path("OPENCODE_CONFIG_DIR") {
        manager.push_opencode_directory(&mut files, &root, "custom_dir", SettingScope::User, false);
    }
    if let Some(content) =
        env("OPENCODE_CONFIG_CONTENT").filter(|content| !content.trim().is_empty())
    {
        let mut spec = file_spec(
            "inline_config",
            manager.user_root.join(".inline"),
            NativeConfigFormat::Jsonc,
            SettingScope::User,
        );
        spec.inline_content = Some(content);
        files.push(spec);
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn environment(home: &Path) -> HashMap<String, String> {
        HashMap::from([
            ("HOME".into(), home.display().to_string()),
            ("USERPROFILE".into(), home.display().to_string()),
        ])
    }

    fn discover(
        provider: DirectProvider,
        cwd: &Path,
        env: &HashMap<String, String>,
    ) -> Option<NativeModelDefaults> {
        native_defaults_with_environment(provider, cwd, &|name| env.get(name).cloned())
    }

    #[test]
    fn native_defaults_cover_all_six_providers_without_creating_files() {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().join("project");
        fs::create_dir(&cwd).unwrap();
        let fixtures = [
            (
                DirectProvider::Codex,
                ".codex/config.toml",
                "model = \"custom-codex\"\nmodel_reasoning_effort = \"future-effort\"\n",
                "custom-codex",
                Some("future-effort"),
            ),
            (
                DirectProvider::ClaudeCode,
                ".claude/settings.json",
                r#"{"model":"opus","effortLevel":"xhigh"}"#,
                "opus",
                Some("xhigh"),
            ),
            (
                DirectProvider::Gemini,
                ".gemini/settings.json",
                r#"{"model":{"name":"gemini-custom"},"modelConfigs":{"overrides":[{"match":{"model":"gemini-custom"},"modelConfig":{"generateContentConfig":{"thinkingConfig":{"thinkingBudget":123}}}}]}}"#,
                "gemini-custom",
                None,
            ),
            (
                DirectProvider::OhMyPi,
                ".omp/agent/config.yml",
                "modelRoles:\n  default: provider/custom-omp\ndefaultThinkingLevel: auto\n",
                "provider/custom-omp",
                Some("auto"),
            ),
            (
                DirectProvider::Opencode,
                ".config/opencode/opencode.jsonc",
                "{\n// retain comment\n\"model\":\"provider/custom-open\"\n}",
                "provider/custom-open",
                None,
            ),
            (
                DirectProvider::DeepseekHarness,
                ".dsh/profiles/acp/cordis.patch.yml",
                "- id: acp\n  config:\n    provider: deepseek-official\n    model: custom-dsh\n- id: llm-deepseek\n  config:\n    reasoningEffort: high\n",
                r#"["deepseek-official","custom-dsh"]"#,
                None,
            ),
        ];
        for (provider, file, content, model, effort) in fixtures {
            let home = root.path().join(provider.id());
            let path = home.join(file);
            write(&path, content);
            let before = fs::read(&path).unwrap();
            let defaults = discover(provider, &cwd, &environment(&home)).unwrap();
            assert_eq!(defaults.model_id.as_deref(), Some(model), "{provider:?}");
            assert_eq!(defaults.reasoning_id.as_deref(), effort, "{provider:?}");
            if provider == DirectProvider::DeepseekHarness {
                assert_eq!(defaults.model_reasoning[model], "high");
            }
            assert_eq!(fs::read(&path).unwrap(), before);
        }
        assert_eq!(fs::read_dir(cwd).unwrap().count(), 0);
    }

    #[test]
    fn claude_local_settings_and_exact_model_efforts_preserve_native_precedence() {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().join("project");
        let home = root.path().join("home");
        write(
            &home.join(".claude/settings.json"),
            r#"{"model":"opus","effortLevel":"low","modelSettings":{"exact/opus":{"effortLevel":"medium"},"other":{"effortLevel":"high"}}}"#,
        );
        write(
            &cwd.join(".claude/settings.json"),
            r#"{"model":"exact/opus","modelSettings":{"exact/opus":{"effortLevel":"xhigh"}}}"#,
        );
        write(
            &cwd.join(".claude/settings.local.json"),
            r#"{"effortLevel":"high","modelSettings":{"local/model":{"effortLevel":"low"}}}"#,
        );
        let mut env = environment(&home);
        let defaults = discover(DirectProvider::ClaudeCode, &cwd, &env).unwrap();
        assert_eq!(defaults.model_id.as_deref(), Some("exact/opus"));
        assert_eq!(defaults.reasoning_id.as_deref(), Some("high"));
        assert_eq!(defaults.model_reasoning["exact/opus"], "xhigh");
        assert_eq!(defaults.model_reasoning["other"], "high");
        assert_eq!(defaults.model_reasoning["local/model"], "low");
        assert!(!defaults.model_reasoning.contains_key("opus"));
        env.insert("CLAUDE_CODE_EFFORT_LEVEL".into(), "max".into());
        env.insert("ANTHROPIC_MODEL".into(), "environment/model".into());
        let launch = discover(DirectProvider::ClaudeCode, &cwd, &env).unwrap();
        assert_eq!(launch.model_id.as_deref(), Some("environment/model"));
        assert_eq!(launch.reasoning_id.as_deref(), Some("max"));
        assert!(launch.model_reasoning.is_empty());
    }

    #[test]
    fn codex_native_selected_profile_and_future_effort_remain_lossless() {
        let root = tempfile::tempdir().unwrap();
        write(
            &root.path().join(".codex/config.toml"),
            "model = \"base\"\nmodel_reasoning_effort = \"max\"\nprofile = \"native\"\n[profiles.native]\nmodel = \"selected\"\nmodel_reasoning_effort = \"ultra\"\n",
        );
        let defaults = discover(
            DirectProvider::Codex,
            root.path(),
            &environment(root.path()),
        )
        .unwrap();
        assert_eq!(defaults.model_id.as_deref(), Some("selected"));
        assert_eq!(defaults.reasoning_id.as_deref(), Some("ultra"));
        let descriptor = codex_descriptors("openai")
            .into_iter()
            .find(|descriptor| descriptor.key.id() == "common.reasoning")
            .unwrap();
        assert!(
            validate_setting_value(&descriptor, &Value::String("future-effort".into())).is_ok()
        );
    }

    #[test]
    fn profile_roots_and_inline_opencode_configuration_do_not_use_host_defaults() {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().join("project");
        fs::create_dir(&cwd).unwrap();
        let mut env = environment(root.path());
        write(
            &root.path().join(".claude/settings.json"),
            r#"{"model":"wrong-host-model"}"#,
        );
        write(
            &root.path().join("custom-claude/settings.json"),
            r#"{"model":"child-model"}"#,
        );
        env.insert("CLAUDE_CONFIG_DIR".into(), "../custom-claude".into());
        assert_eq!(
            discover(DirectProvider::ClaudeCode, &cwd, &env)
                .unwrap()
                .model_id
                .as_deref(),
            Some("child-model")
        );
        write(
            &root.path().join("other-config/opencode/opencode.json"),
            r#"{"model":"child-config/model"}"#,
        );
        env.insert("XDG_CONFIG_HOME".into(), "../other-config".into());
        assert_eq!(
            discover(DirectProvider::Opencode, &cwd, &env)
                .unwrap()
                .model_id
                .as_deref(),
            Some("child-config/model")
        );
        env.insert(
            "OPENCODE_CONFIG_CONTENT".into(),
            r#"{"model":"inline/model"}"#.into(),
        );
        assert_eq!(
            discover(DirectProvider::Opencode, &cwd, &env)
                .unwrap()
                .model_id
                .as_deref(),
            Some("inline/model")
        );
    }

    #[test]
    fn omp_profile_resolver_is_shared_and_uses_the_child_environment() {
        let root = tempfile::tempdir().unwrap();
        write(
            &root.path().join(".omp/agent/config.yml"),
            "modelRoles:\n  default: wrong/host\n",
        );
        write(
            &root.path().join(".omp/profiles/work/agent/config.yml"),
            "modelRoles:\n  default: right/child\ndefaultThinkingLevel: low\n",
        );
        let mut env = environment(root.path());
        env.insert("OMP_PROFILE".into(), "work".into());
        let defaults = discover(DirectProvider::OhMyPi, root.path(), &env).unwrap();
        assert_eq!(defaults.model_id.as_deref(), Some("right/child"));
        assert_eq!(defaults.reasoning_id.as_deref(), Some("low"));
    }

    #[test]
    fn dsh_later_plugin_config_replaces_the_whole_profile_config() {
        let root = tempfile::tempdir().unwrap();
        write(
            &root.path().join(".dsh/profiles/acp/cordis.patch.yml"),
            "- id: acp\n  config:\n    provider: route\n    model: old\n- id: llm-deepseek\n  config:\n    reasoningEffort: high\n",
        );
        write(
            &root.path().join(".dsh/cordis.patch.yml"),
            "- id: llm-deepseek\n  config:\n    apiKeyEnv: CUSTOM_KEY\n",
        );
        let defaults = discover(
            DirectProvider::DeepseekHarness,
            root.path(),
            &environment(root.path()),
        )
        .unwrap();
        assert_eq!(defaults.model_id.as_deref(), Some(r#"["route","old"]"#));
        assert_eq!(defaults.reasoning_id, None);
    }

    #[test]
    fn dsh_effort_is_bound_to_the_exact_deepseek_official_model() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join(".dsh/cordis.patch.yml");
        for provider in [
            "deepseek-official",
            "deepseek-account",
            "pi-ai/openai",
            "custom-route",
        ] {
            write(
                &home,
                &format!(
                    "- id: acp\n  config:\n    provider: {provider}\n    model: selected-model\n- id: llm-deepseek\n  config:\n    reasoningEffort: high\n"
                ),
            );
            let defaults = discover(
                DirectProvider::DeepseekHarness,
                root.path(),
                &environment(root.path()),
            )
            .unwrap();
            assert!(defaults.reasoning_id.is_none());
            if provider == "deepseek-official" {
                assert_eq!(
                    defaults.model_reasoning[r#"["deepseek-official","selected-model"]"#],
                    "high"
                );
            } else {
                assert!(defaults.model_reasoning.is_empty(), "{provider}");
            }
        }
    }

    #[test]
    fn dsh_model_discovery_specs_exclude_credentials_and_environment_documents() {
        let root = tempfile::tempdir().unwrap();
        let manager = ProviderSettingsManager {
            provider: AgentSettingsProvider::DeepseekHarness,
            home_dir: root.path().to_path_buf(),
            user_root: root.path().join("native-dsh"),
            project_path: Some(root.path().to_path_buf()),
        };
        let specs = manager.deepseek_config_file_specs();
        assert_eq!(specs.len(), 2);
        assert!(
            specs
                .iter()
                .all(|spec| spec.dialect == NativeDialect::Cordis)
        );
        assert!(
            specs
                .iter()
                .all(|spec| spec.path.file_name().unwrap() == "cordis.patch.yml")
        );
        // A malformed/oversized credential document must not invalidate model
        // discovery: it is outside the configuration-only read boundary.
        write(
            &manager.user_root.join(".credentials.yaml"),
            &"!unsupported ".repeat(180_000),
        );
        write(
            &manager.user_root.join("cordis.patch.yml"),
            "- id: acp\n  config:\n    provider: deepseek-official\n    model: model\n",
        );
        let mut env = environment(root.path());
        env.insert("DSH_HOME".into(), manager.user_root.display().to_string());
        assert_eq!(
            discover(DirectProvider::DeepseekHarness, root.path(), &env)
                .unwrap()
                .model_id
                .as_deref(),
            Some(r#"["deepseek-official","model"]"#)
        );
    }

    #[test]
    fn missing_invalid_and_redirected_native_authority_remain_unknown() {
        let root = tempfile::tempdir().unwrap();
        let env = environment(root.path());
        for provider in DirectProvider::ALL {
            assert!(discover(provider, root.path(), &env).is_none());
        }
        write(
            &root.path().join(".claude/settings.json"),
            r#"{"model":"user-model"}"#,
        );
        write(&root.path().join(".claude/settings.local.json"), "{");
        assert!(discover(DirectProvider::ClaudeCode, root.path(), &env).is_none());
        let cmd = CmdOverrides {
            base_command_override: Some("arbitrary-wrapper".into()),
            env: Some(env),
            ..Default::default()
        };
        assert!(
            native_model_defaults_for(DirectProvider::ClaudeCode, Some(root.path()), &cmd)
                .is_none()
        );
    }

    #[test]
    fn oversized_higher_priority_configuration_is_unknown_instead_of_inheriting() {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().join("project");
        write(
            &root.path().join(".claude/settings.json"),
            r#"{"model":"lower-priority"}"#,
        );
        let oversized = format!(
            r#"{{"unknown":"{}","model":"higher-priority"}}"#,
            "x".repeat(2 * 1024 * 1024)
        );
        write(&cwd.join(".claude/settings.local.json"), &oversized);
        assert!(discover(DirectProvider::ClaudeCode, &cwd, &environment(root.path())).is_none());
        assert_eq!(
            fs::read_to_string(cwd.join(".claude/settings.local.json")).unwrap(),
            oversized
        );
    }
}
