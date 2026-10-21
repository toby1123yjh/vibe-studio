use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    time::SystemTime,
};

use chrono::{DateTime, TimeZone, Utc};
use db::models::coding_agent_turn::ResumableAgentSession;
use executors::executors::oh_my_pi::oh_my_pi_agent_root;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

const MAX_NATIVE_SCAN_FILES: usize = 500;
const SESSION_FILE_METADATA_LINES: usize = 100;
const TITLE_MAX_CHARS: usize = 100;
const PREVIEW_ENTRY_MAX_CHARS: usize = 1_200;
const PREVIEW_MAX_ENTRIES_PER_TURN: usize = 4;
const GEMINI_MAX_PROJECT_DIRS: usize = 100;
const OMP_MAX_SESSION_DIRS: usize = 100;
pub const DEFAULT_NATIVE_SESSION_PREVIEW_TURNS: usize = 20;
pub const MAX_NATIVE_SESSION_PREVIEW_TURNS: usize = 50;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum NativeSessionDiscoveryState {
    Supported,
    Unsupported,
}

pub fn native_history_discovery_supported(executor: &str) -> bool {
    matches!(
        executor
            .trim()
            .replace('-', "_")
            .to_ascii_uppercase()
            .as_str(),
        "CODEX" | "CLAUDE_CODE" | "GEMINI" | "OH_MY_PI"
    )
}

pub fn native_session_discovery_state(executor: &str) -> NativeSessionDiscoveryState {
    if native_history_discovery_supported(executor) {
        NativeSessionDiscoveryState::Supported
    } else {
        NativeSessionDiscoveryState::Unsupported
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct NativeSessionPreviewEntry {
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub timestamp: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct NativeAgentSessionPreview {
    pub agent_session_id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub last_used_at: Option<DateTime<Utc>>,
    pub entries: Vec<NativeSessionPreviewEntry>,
    pub truncated: bool,
    pub turn_limit: usize,
}

#[derive(Debug, Default, Clone)]
struct NativeSessionDraft {
    agent_session_id: String,
    forked_from_id: Option<String>,
    title: Option<String>,
    title_source: Option<NativeTitleSource>,
    last_used_at: Option<DateTime<Utc>>,
    cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum NativeTitleSource {
    SessionContent = 0,
    Prompt = 1,
    ExplicitTitle = 2,
}

impl NativeSessionDraft {
    fn update_title(&mut self, title: Option<String>, source: NativeTitleSource) {
        if let Some(title) = title.and_then(normalize_title) {
            let should_replace = self
                .title_source
                .is_none_or(|existing_source| source > existing_source);

            if should_replace {
                self.title = Some(title);
                self.title_source = Some(source);
            }
        }
    }

    fn update_time(&mut self, timestamp: Option<DateTime<Utc>>) {
        let Some(timestamp) = timestamp else {
            return;
        };

        if self
            .last_used_at
            .is_none_or(|existing| timestamp > existing)
        {
            self.last_used_at = Some(timestamp);
        }
    }

    fn update_cwd(&mut self, cwd: Option<PathBuf>) {
        if let Some(cwd) = cwd {
            self.cwd = Some(cwd);
        }
    }
}

pub fn list_native_resumable_agent_sessions(
    executor: &str,
    since: DateTime<Utc>,
    limit: usize,
    workspace_scope: &[PathBuf],
    allow_unknown_cwd: bool,
) -> Vec<ResumableAgentSession> {
    let normalized_executor = executor.trim().replace('-', "_").to_ascii_uppercase();

    let drafts = match normalized_executor.as_str() {
        "CODEX" => list_codex_sessions(),
        "CLAUDE_CODE" => list_claude_sessions(),
        "GEMINI" => list_gemini_sessions(),
        "OH_MY_PI" => list_oh_my_pi_sessions(),
        _ => return Vec::new(),
    };

    finalize_sessions(drafts, since, limit, workspace_scope, allow_unknown_cwd)
}

pub fn get_native_agent_session_preview(
    executor: &str,
    session_id: &str,
    turn_limit: usize,
    workspace_scope: &[PathBuf],
    allow_unknown_cwd: bool,
) -> Option<NativeAgentSessionPreview> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return None;
    }

    let turn_limit = turn_limit.clamp(1, MAX_NATIVE_SESSION_PREVIEW_TURNS);
    let normalized_executor = executor.trim().replace('-', "_").to_ascii_uppercase();
    let (drafts, entries) = match normalized_executor.as_str() {
        "CODEX" => (
            list_codex_sessions(),
            read_codex_preview_entries(session_id),
        ),
        "CLAUDE_CODE" => (
            list_claude_sessions(),
            read_claude_preview_entries(session_id),
        ),
        "GEMINI" => (
            list_gemini_sessions(),
            read_gemini_preview_entries(session_id),
        ),
        "OH_MY_PI" => (
            list_oh_my_pi_sessions(),
            read_oh_my_pi_preview_entries(session_id),
        ),
        _ => return None,
    };

    let draft = drafts.get(session_id)?;
    if !matches_workspace_scope(draft.cwd.as_deref(), workspace_scope, allow_unknown_cwd) {
        return None;
    }

    let (entries, truncated) = limit_preview_entries(entries, turn_limit);
    Some(NativeAgentSessionPreview {
        agent_session_id: draft.agent_session_id.clone(),
        title: draft
            .title
            .clone()
            .unwrap_or_else(|| "Untitled session".to_string()),
        last_used_at: draft.last_used_at.clone(),
        entries,
        truncated,
        turn_limit,
    })
}

fn list_codex_sessions() -> HashMap<String, NativeSessionDraft> {
    let Some(codex_home) = executors::executors::codex::codex_home() else {
        return HashMap::new();
    };

    let mut drafts = HashMap::new();

    read_codex_session_index(&codex_home.join("session_index.jsonl"), &mut drafts);
    read_codex_session_files(&codex_home.join("sessions"), &mut drafts);
    read_codex_history(&codex_home.join("history.jsonl"), &mut drafts);

    collapse_codex_fork_chains(drafts)
}

fn read_codex_history(path: &Path, drafts: &mut HashMap<String, NativeSessionDraft>) {
    for value in read_jsonl_values(path, None) {
        let Some(session_id) = string_field(&value, "session_id") else {
            continue;
        };

        let timestamp = datetime_field(&value, "ts");
        let title = string_field(&value, "text").map(ToOwned::to_owned);
        // Codex history.jsonl is prompt-level history. It can enrich known
        // sessions, but must not create one picker item per prompt.
        update_existing_draft(
            drafts,
            session_id,
            timestamp,
            title,
            NativeTitleSource::Prompt,
            None,
        );
    }
}

fn read_codex_session_index(path: &Path, drafts: &mut HashMap<String, NativeSessionDraft>) {
    for value in read_jsonl_values(path, None) {
        let Some(session_id) = string_field(&value, "id")
            .or_else(|| string_field(&value, "session_id"))
            .or_else(|| string_field(&value, "thread_id"))
        else {
            continue;
        };

        let timestamp =
            datetime_field(&value, "updated_at").or_else(|| datetime_field(&value, "created_at"));
        let title = string_field(&value, "thread_name")
            .or_else(|| string_field(&value, "title"))
            .map(ToOwned::to_owned);
        update_draft(
            drafts,
            session_id,
            timestamp,
            title,
            NativeTitleSource::ExplicitTitle,
            None,
        );
    }
}

fn read_codex_session_files(root: &Path, drafts: &mut HashMap<String, NativeSessionDraft>) {
    for (path, modified_at) in collect_jsonl_files(root, 4)
        .into_iter()
        .take(MAX_NATIVE_SCAN_FILES)
    {
        let mut session_id = None;
        let mut forked_from_id = None;
        let mut cwd = None;
        let mut title = None;

        for value in read_jsonl_values(&path, Some(SESSION_FILE_METADATA_LINES)) {
            if session_id.is_none() {
                if let Some(id) = codex_session_id_from_value(&value) {
                    session_id = Some(id.to_owned());
                    forked_from_id = codex_forked_from_id_from_value(&value).map(ToOwned::to_owned);
                }
            }

            if cwd.is_none() {
                cwd = value
                    .pointer("/payload/cwd")
                    .and_then(Value::as_str)
                    .or_else(|| string_field(&value, "cwd"))
                    .map(PathBuf::from);
            }

            if title.is_none() {
                let payload = value.get("payload").unwrap_or(&value);
                let role = string_field(payload, "role")
                    .or_else(|| payload.pointer("/message/role").and_then(Value::as_str));
                if role.is_none() || role == Some("user") {
                    let candidate = payload
                        .pointer("/message/content")
                        .or_else(|| payload.get("content"))
                        .and_then(text_from_content_value);
                    if candidate
                        .as_deref()
                        .is_some_and(is_codex_synthetic_user_content)
                    {
                        continue;
                    }
                    title = candidate;
                }
            }

            if session_id.is_some() && cwd.is_some() && title.is_some() {
                break;
            }
        }

        let Some(session_id) = session_id else {
            continue;
        };

        update_draft(
            drafts,
            &session_id,
            Some(modified_at),
            title,
            NativeTitleSource::SessionContent,
            cwd,
        );
        if let Some(draft) = drafts.get_mut(&session_id) {
            draft.forked_from_id = forked_from_id;
        }
    }
}

fn collapse_codex_fork_chains(
    drafts: HashMap<String, NativeSessionDraft>,
) -> HashMap<String, NativeSessionDraft> {
    let parents = drafts
        .iter()
        .filter_map(|(thread_id, draft)| {
            draft
                .forked_from_id
                .as_ref()
                .map(|parent_id| (thread_id.clone(), parent_id.clone()))
        })
        .collect::<HashMap<_, _>>();
    let mut groups = HashMap::<String, Vec<(String, NativeSessionDraft)>>::new();

    for (thread_id, draft) in drafts {
        let root_id = codex_root_thread_id(&thread_id, &parents);
        groups.entry(root_id).or_default().push((thread_id, draft));
    }

    groups
        .into_values()
        .filter_map(|mut members| {
            let parent_ids = members
                .iter()
                .filter_map(|(_, draft)| draft.forked_from_id.clone())
                .collect::<HashSet<_>>();
            let latest_thread_id = members
                .iter()
                .filter(|(thread_id, _)| !parent_ids.contains(thread_id))
                .max_by(|(left_id, left), (right_id, right)| {
                    left.last_used_at
                        .cmp(&right.last_used_at)
                        .then_with(|| left_id.cmp(right_id))
                })
                .or_else(|| {
                    members.iter().max_by(|(left_id, left), (right_id, right)| {
                        left.last_used_at
                            .cmp(&right.last_used_at)
                            .then_with(|| left_id.cmp(right_id))
                    })
                })?
                .0
                .clone();
            let last_used_at = members
                .iter()
                .filter_map(|(_, draft)| draft.last_used_at)
                .max();
            let cwd = members
                .iter()
                .filter_map(|(thread_id, draft)| {
                    draft
                        .cwd
                        .as_ref()
                        .map(|cwd| (draft.last_used_at, thread_id, cwd))
                })
                .max_by(|(left_time, left_id, _), (right_time, right_id, _)| {
                    left_time
                        .cmp(right_time)
                        .then_with(|| left_id.cmp(right_id))
                })
                .map(|(_, _, cwd)| cwd.clone());

            members.sort_by(|(left_id, left), (right_id, right)| {
                codex_thread_depth(left_id, &parents)
                    .cmp(&codex_thread_depth(right_id, &parents))
                    .then_with(|| left.last_used_at.cmp(&right.last_used_at))
                    .then_with(|| left_id.cmp(right_id))
            });

            let mut merged = NativeSessionDraft {
                agent_session_id: latest_thread_id.clone(),
                last_used_at,
                cwd,
                ..NativeSessionDraft::default()
            };
            for (_, draft) in members {
                if let Some(source) = draft.title_source {
                    merged.update_title(draft.title, source);
                }
            }

            Some((latest_thread_id, merged))
        })
        .collect()
}

fn codex_root_thread_id(thread_id: &str, parents: &HashMap<String, String>) -> String {
    let mut current = thread_id.to_string();
    let mut visited = HashSet::new();

    loop {
        if !visited.insert(current.clone()) {
            return visited.into_iter().min().unwrap_or(current);
        }
        let Some(parent_id) = parents.get(&current) else {
            return current;
        };
        current = parent_id.clone();
    }
}

fn codex_thread_depth(thread_id: &str, parents: &HashMap<String, String>) -> usize {
    let mut current = thread_id;
    let mut visited = HashSet::new();
    let mut depth = 0;

    while visited.insert(current.to_string()) {
        let Some(parent_id) = parents.get(current) else {
            break;
        };
        current = parent_id;
        depth += 1;
    }

    depth
}

fn read_codex_preview_entries(session_id: &str) -> Vec<NativeSessionPreviewEntry> {
    let Some(codex_home) = executors::executors::codex::codex_home() else {
        return Vec::new();
    };
    let Some(path) = find_codex_session_file(&codex_home.join("sessions"), session_id) else {
        return Vec::new();
    };

    read_codex_preview_entries_from_file(&path)
}

fn find_codex_session_file(root: &Path, session_id: &str) -> Option<PathBuf> {
    collect_jsonl_files(root, 4)
        .into_iter()
        .take(MAX_NATIVE_SCAN_FILES)
        .find_map(|(path, _)| codex_session_file_matches(&path, session_id).then_some(path))
}

fn codex_session_file_matches(path: &Path, session_id: &str) -> bool {
    read_jsonl_values(path, Some(SESSION_FILE_METADATA_LINES))
        .iter()
        .any(|value| codex_session_id_from_value(value) == Some(session_id))
}

fn read_codex_preview_entries_from_file(path: &Path) -> Vec<NativeSessionPreviewEntry> {
    read_jsonl_values(path, None)
        .into_iter()
        .filter_map(|value| codex_preview_entry_from_value(&value))
        .collect()
}

fn codex_preview_entry_from_value(value: &Value) -> Option<NativeSessionPreviewEntry> {
    let payload = value.get("payload").unwrap_or(value);
    let role = string_field(payload, "role")
        .or_else(|| payload.pointer("/message/role").and_then(Value::as_str))
        .or_else(|| string_field(value, "role"))
        .or_else(|| value.pointer("/message/role").and_then(Value::as_str))?;
    let role = normalize_preview_role(role)?;
    let content = payload
        .get("content")
        .or_else(|| payload.pointer("/message/content"))
        .or_else(|| value.get("content"))
        .or_else(|| value.pointer("/message/content"))
        .and_then(|content| text_from_content_value_with_separator(content, "\n"))?;
    let content = normalize_preview_content(content)?;
    if role == "user" && is_codex_synthetic_user_content(&content) {
        return None;
    }
    let timestamp = datetime_field(value, "timestamp")
        .or_else(|| datetime_field(value, "ts"))
        .or_else(|| datetime_field(payload, "timestamp"))
        .or_else(|| datetime_field(payload, "ts"));

    Some(NativeSessionPreviewEntry {
        role,
        content,
        timestamp,
    })
}

fn list_claude_sessions() -> HashMap<String, NativeSessionDraft> {
    let Some(claude_home) = home_dir().map(|home| home.join(".claude")) else {
        return HashMap::new();
    };

    let mut drafts = HashMap::new();

    read_claude_project_files(&claude_home.join("projects"), &mut drafts);
    read_claude_history(&claude_home.join("history.jsonl"), &mut drafts);

    drafts
}

fn read_claude_history(path: &Path, drafts: &mut HashMap<String, NativeSessionDraft>) {
    for value in read_jsonl_values(path, None) {
        let Some(session_id) =
            string_field(&value, "sessionId").or_else(|| string_field(&value, "session_id"))
        else {
            continue;
        };

        let timestamp =
            datetime_field(&value, "timestamp").or_else(|| datetime_field(&value, "ts"));
        let title = string_field(&value, "display")
            .map(|title| (title.to_string(), NativeTitleSource::ExplicitTitle))
            .or_else(|| {
                string_field(&value, "prompt")
                    .map(|title| (title.to_string(), NativeTitleSource::Prompt))
            });
        let cwd = string_field(&value, "cwd")
            .or_else(|| {
                string_field(&value, "project").filter(|project| Path::new(project).is_absolute())
            })
            .map(PathBuf::from);

        let (title, title_source) = title.unwrap_or((String::new(), NativeTitleSource::Prompt));
        // Claude global history is prompt-level metadata. Project JSONL files
        // are the candidate-creating source for native resume.
        update_existing_draft(
            drafts,
            session_id,
            timestamp,
            (!title.is_empty()).then_some(title),
            title_source,
            cwd,
        );
    }
}

fn read_claude_project_files(root: &Path, drafts: &mut HashMap<String, NativeSessionDraft>) {
    for (path, modified_at) in collect_jsonl_files(root, 2)
        .into_iter()
        .take(MAX_NATIVE_SCAN_FILES)
    {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if file_name.starts_with("agent-") {
            continue;
        }

        let mut session_id = path
            .file_stem()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned);
        let mut cwd = None;
        let mut title = None;
        let mut timestamp = Some(modified_at);

        for value in read_jsonl_values(&path, Some(SESSION_FILE_METADATA_LINES)) {
            if session_id.is_none() {
                session_id = claude_session_id_from_value(&value).map(ToOwned::to_owned);
            }

            if cwd.is_none() {
                cwd = string_field(&value, "cwd").map(PathBuf::from);
            }

            if title.is_none() {
                title = value
                    .get("message")
                    .and_then(|message| {
                        (message.get("role").and_then(Value::as_str) == Some("user"))
                            .then(|| message.get("content"))
                            .flatten()
                    })
                    .and_then(text_from_content_value);
            }

            if let Some(parsed_timestamp) = datetime_field(&value, "timestamp") {
                timestamp = Some(parsed_timestamp);
            }

            if session_id.is_some() && cwd.is_some() && title.is_some() {
                break;
            }
        }

        let Some(session_id) = session_id else {
            continue;
        };

        update_draft(
            drafts,
            &session_id,
            timestamp,
            title,
            NativeTitleSource::SessionContent,
            cwd,
        );
    }
}

fn read_claude_preview_entries(session_id: &str) -> Vec<NativeSessionPreviewEntry> {
    let Some(claude_home) = home_dir().map(|home| home.join(".claude")) else {
        return Vec::new();
    };
    let Some(path) = find_claude_project_file(&claude_home.join("projects"), session_id) else {
        return Vec::new();
    };

    read_claude_preview_entries_from_file(&path)
}

fn find_claude_project_file(root: &Path, session_id: &str) -> Option<PathBuf> {
    collect_jsonl_files(root, 2)
        .into_iter()
        .take(MAX_NATIVE_SCAN_FILES)
        .find_map(|(path, _)| {
            let file_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if file_name.starts_with("agent-") {
                return None;
            }

            if path.file_stem().and_then(|name| name.to_str()) == Some(session_id)
                || claude_session_file_matches(&path, session_id)
            {
                Some(path)
            } else {
                None
            }
        })
}

fn claude_session_file_matches(path: &Path, session_id: &str) -> bool {
    read_jsonl_values(path, Some(SESSION_FILE_METADATA_LINES))
        .iter()
        .any(|value| claude_session_id_from_value(value) == Some(session_id))
}

fn read_claude_preview_entries_from_file(path: &Path) -> Vec<NativeSessionPreviewEntry> {
    read_jsonl_values(path, None)
        .into_iter()
        .filter_map(|value| claude_preview_entry_from_value(&value))
        .collect()
}

fn claude_preview_entry_from_value(value: &Value) -> Option<NativeSessionPreviewEntry> {
    let message = value.get("message");
    let role = message
        .and_then(|message| string_field(message, "role"))
        .or_else(|| string_field(value, "type"))
        .or_else(|| string_field(value, "role"))?;
    let role = normalize_preview_role(role)?;
    let content = message
        .and_then(|message| message.get("content"))
        .or_else(|| value.get("content"))
        .and_then(|content| text_from_content_value_with_separator(content, "\n"))?;
    let content = normalize_preview_content(content)?;
    let timestamp = datetime_field(value, "timestamp").or_else(|| datetime_field(value, "ts"));

    Some(NativeSessionPreviewEntry {
        role,
        content,
        timestamp,
    })
}

// Gemini and Oh My Pi keep provider-owned JSONL transcripts. These readers are
// deliberately separate from the Codex/Claude readers: the formats are not a
// shared transcript contract and malformed/foreign files must fail closed.
fn list_gemini_sessions() -> HashMap<String, NativeSessionDraft> {
    let Some(root) = home_dir().map(|home| home.join(".gemini").join("tmp")) else {
        return HashMap::new();
    };
    list_gemini_sessions_from_root(&root)
}

fn list_gemini_sessions_from_root(root: &Path) -> HashMap<String, NativeSessionDraft> {
    let mut drafts = HashMap::new();
    let Ok(entries) = fs::read_dir(root) else {
        return drafts;
    };

    for project in entries
        .flatten()
        .filter_map(|entry| {
            entry
                .file_type()
                .ok()
                .filter(|kind| kind.is_dir())
                .map(|_| entry.path())
        })
        .take(GEMINI_MAX_PROJECT_DIRS)
    {
        let Some(cwd) = read_project_root(&project) else {
            continue;
        };
        read_gemini_project_files(&project.join("chats"), &cwd, &mut drafts);
    }

    drafts
}

fn read_project_root(project_dir: &Path) -> Option<PathBuf> {
    let root = fs::read_to_string(project_dir.join(".project_root")).ok()?;
    let root = root.trim();
    (!root.is_empty()).then(|| PathBuf::from(root))
}

fn read_gemini_project_files(
    chats_dir: &Path,
    cwd: &Path,
    drafts: &mut HashMap<String, NativeSessionDraft>,
) {
    for (path, modified_at) in collect_jsonl_files(chats_dir, 1)
        .into_iter()
        .take(MAX_NATIVE_SCAN_FILES)
    {
        let values = read_jsonl_values(&path, Some(SESSION_FILE_METADATA_LINES));
        let Some(header) = values.first() else {
            continue;
        };
        let Some(session_id) = string_field(header, "sessionId")
            .or_else(|| string_field(header, "session_id"))
            .filter(|id| !id.trim().is_empty())
        else {
            continue;
        };

        let timestamp = datetime_field(header, "lastUpdated")
            .or_else(|| datetime_field(header, "startTime"))
            .or(Some(modified_at));
        let title = values
            .iter()
            .filter_map(gemini_message_from_value)
            .find_map(|(role, content, _)| {
                (role == "user" && !is_gemini_synthetic_context(&content)).then_some(content)
            });

        update_draft(
            drafts,
            session_id,
            timestamp,
            title,
            NativeTitleSource::SessionContent,
            Some(cwd.to_path_buf()),
        );
    }
}

fn read_gemini_preview_entries(session_id: &str) -> Vec<NativeSessionPreviewEntry> {
    let Some(root) = home_dir().map(|home| home.join(".gemini").join("tmp")) else {
        return Vec::new();
    };
    let Some(path) = find_gemini_session_file(&root, session_id) else {
        return Vec::new();
    };
    read_gemini_preview_entries_from_file(&path)
}

fn find_gemini_session_file(root: &Path, session_id: &str) -> Option<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return None;
    };
    entries
        .flatten()
        .filter_map(|entry| {
            entry
                .file_type()
                .ok()
                .filter(|kind| kind.is_dir())
                .map(|_| entry.path())
        })
        .take(GEMINI_MAX_PROJECT_DIRS)
        .filter_map(|project| read_project_root(&project).map(|_| project))
        .flat_map(|project| collect_jsonl_files(&project.join("chats"), 1))
        .take(MAX_NATIVE_SCAN_FILES)
        .find_map(|(path, _)| {
            read_jsonl_values(&path, Some(1))
                .first()
                .and_then(|header| {
                    string_field(header, "sessionId").or_else(|| string_field(header, "session_id"))
                })
                .filter(|id| *id == session_id)
                .map(|_| path)
        })
}

fn read_gemini_preview_entries_from_file(path: &Path) -> Vec<NativeSessionPreviewEntry> {
    read_jsonl_values(path, Some(20_000))
        .into_iter()
        .filter_map(|value| {
            let (role, content, timestamp) = gemini_message_from_value(&value)?;
            if is_gemini_synthetic_context(&content) {
                return None;
            }
            Some(NativeSessionPreviewEntry {
                role,
                content: normalize_preview_content(content)?,
                timestamp,
            })
        })
        .collect()
}

fn gemini_message_from_value(value: &Value) -> Option<(String, String, Option<DateTime<Utc>>)> {
    let role = string_field(value, "type")
        .or_else(|| string_field(value, "role"))
        .or_else(|| value.pointer("/message/role").and_then(Value::as_str))?;
    let role = match role.trim().to_ascii_lowercase().as_str() {
        "user" => "user",
        "gemini" | "assistant" | "model" => "assistant",
        _ => return None,
    };
    let content = value
        .get("content")
        .or_else(|| {
            value
                .get("message")
                .and_then(|message| message.get("content"))
        })
        .and_then(|content| text_from_content_value_with_separator(content, "\n"))?;
    Some((
        role.to_string(),
        content,
        datetime_field(value, "timestamp").or_else(|| datetime_field(value, "ts")),
    ))
}

fn is_gemini_synthetic_context(content: &str) -> bool {
    content.trim_start().starts_with("<session_context>")
}

fn list_oh_my_pi_sessions() -> HashMap<String, NativeSessionDraft> {
    let Some(root) = oh_my_pi_sessions_root() else {
        return HashMap::new();
    };
    list_oh_my_pi_sessions_from_root(&root)
}

fn oh_my_pi_sessions_root() -> Option<PathBuf> {
    oh_my_pi_agent_root().map(|root| root.join("sessions"))
}

fn list_oh_my_pi_sessions_from_root(root: &Path) -> HashMap<String, NativeSessionDraft> {
    let mut drafts = HashMap::new();
    for (path, modified_at) in collect_jsonl_files(root, 2)
        .into_iter()
        .take(MAX_NATIVE_SCAN_FILES * OMP_MAX_SESSION_DIRS)
    {
        let values = read_jsonl_values(&path, Some(SESSION_FILE_METADATA_LINES));
        let Some(header) = omp_session_header(&values) else {
            continue;
        };
        let Some(session_id) = string_field(header, "id").filter(|id| !id.trim().is_empty()) else {
            continue;
        };
        let cwd = string_field(header, "cwd").map(PathBuf::from);
        let timestamp = datetime_field(header, "timestamp").or(Some(modified_at));
        let title = string_field(header, "title")
            .filter(|title| !title.trim().is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| {
                values
                    .iter()
                    .filter_map(omp_message_from_value)
                    .find_map(|(role, content, _)| (role == "user").then_some(content))
            });
        update_draft(
            &mut drafts,
            session_id,
            timestamp,
            title,
            NativeTitleSource::ExplicitTitle,
            cwd,
        );
    }
    drafts
}

fn read_oh_my_pi_preview_entries(session_id: &str) -> Vec<NativeSessionPreviewEntry> {
    let Some(root) = oh_my_pi_sessions_root() else {
        return Vec::new();
    };
    let Some(path) = find_oh_my_pi_session_file(&root, session_id) else {
        return Vec::new();
    };
    read_jsonl_values(&path, Some(20_000))
        .into_iter()
        .filter_map(|value| {
            let (role, content, timestamp) = omp_message_from_value(&value)?;
            Some(NativeSessionPreviewEntry {
                role,
                content: normalize_preview_content(content)?,
                timestamp,
            })
        })
        .collect()
}

fn find_oh_my_pi_session_file(root: &Path, session_id: &str) -> Option<PathBuf> {
    collect_jsonl_files(root, 2)
        .into_iter()
        .take(MAX_NATIVE_SCAN_FILES * OMP_MAX_SESSION_DIRS)
        .find_map(|(path, _)| {
            let values = read_jsonl_values(&path, Some(2));
            omp_session_header(&values)
                .and_then(|header| string_field(header, "id"))
                .filter(|id| *id == session_id)
                .map(|_| path)
        })
}

fn omp_session_header(values: &[Value]) -> Option<&Value> {
    values
        .iter()
        .take(2)
        .find(|value| string_field(value, "type") == Some("session"))
}

fn omp_message_from_value(value: &Value) -> Option<(String, String, Option<DateTime<Utc>>)> {
    if string_field(value, "type") != Some("message") {
        return None;
    }
    let message = value.get("message").unwrap_or(value);
    let role = string_field(message, "role")?;
    let role = match role.trim().to_ascii_lowercase().as_str() {
        "user" => "user",
        "assistant" => "assistant",
        _ => return None,
    };
    let content = message
        .get("content")
        .and_then(|content| text_from_content_value_with_separator(content, "\n"))?;
    Some((
        role.to_string(),
        content,
        datetime_field(value, "timestamp").or_else(|| datetime_field(message, "timestamp")),
    ))
}

fn update_draft(
    drafts: &mut HashMap<String, NativeSessionDraft>,
    session_id: &str,
    timestamp: Option<DateTime<Utc>>,
    title: Option<String>,
    title_source: NativeTitleSource,
    cwd: Option<PathBuf>,
) {
    let draft = drafts
        .entry(session_id.to_string())
        .or_insert_with(|| NativeSessionDraft {
            agent_session_id: session_id.to_string(),
            ..NativeSessionDraft::default()
        });

    draft.update_time(timestamp);
    draft.update_title(title, title_source);
    draft.update_cwd(cwd);
}

fn update_existing_draft(
    drafts: &mut HashMap<String, NativeSessionDraft>,
    session_id: &str,
    timestamp: Option<DateTime<Utc>>,
    title: Option<String>,
    title_source: NativeTitleSource,
    cwd: Option<PathBuf>,
) {
    if let Some(draft) = drafts.get_mut(session_id) {
        draft.update_time(timestamp);
        draft.update_title(title, title_source);
        draft.update_cwd(cwd);
    }
}

fn codex_session_id_from_value(value: &Value) -> Option<&str> {
    value
        .pointer("/payload/session_id")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/payload/id").and_then(Value::as_str))
        .or_else(|| value.pointer("/payload/thread_id").and_then(Value::as_str))
        .or_else(|| string_field(value, "session_id"))
        .or_else(|| string_field(value, "id"))
        .or_else(|| string_field(value, "thread_id"))
}

fn codex_forked_from_id_from_value(value: &Value) -> Option<&str> {
    value
        .pointer("/payload/forked_from_id")
        .and_then(Value::as_str)
        .or_else(|| string_field(value, "forked_from_id"))
}

fn is_codex_synthetic_user_content(content: &str) -> bool {
    let content = content.trim_start();
    content.starts_with("<environment_context>")
        || content.starts_with("<permissions instructions>")
        || content.starts_with("<collaboration_mode>")
        || content.starts_with("<skills_instructions>")
}

fn claude_session_id_from_value(value: &Value) -> Option<&str> {
    string_field(value, "sessionId").or_else(|| string_field(value, "session_id"))
}

fn limit_preview_entries(
    entries: Vec<NativeSessionPreviewEntry>,
    turn_limit: usize,
) -> (Vec<NativeSessionPreviewEntry>, bool) {
    let turn_limit = turn_limit.max(1);
    let mut entries = entries
        .into_iter()
        .filter_map(|entry| {
            let role = normalize_preview_role(&entry.role)?;
            let content = normalize_preview_content(entry.content)?;
            Some(NativeSessionPreviewEntry {
                role,
                content,
                timestamp: entry.timestamp,
            })
        })
        .collect::<Vec<_>>();

    if entries.is_empty() {
        return (entries, false);
    }

    let mut user_turns = 0;
    let mut start_index = 0;
    for (index, entry) in entries.iter().enumerate().rev() {
        if entry.role == "user" {
            user_turns += 1;
            if user_turns == turn_limit {
                start_index = index;
                break;
            }
        }
    }

    let mut truncated = start_index > 0;
    if start_index > 0 {
        entries = entries.split_off(start_index);
    }

    let max_entries = turn_limit
        .saturating_mul(PREVIEW_MAX_ENTRIES_PER_TURN)
        .max(1);
    if entries.len() > max_entries {
        truncated = true;
        entries = entries.split_off(entries.len() - max_entries);
    }

    (entries, truncated)
}

fn finalize_sessions(
    drafts: HashMap<String, NativeSessionDraft>,
    since: DateTime<Utc>,
    limit: usize,
    workspace_scope: &[PathBuf],
    allow_unknown_cwd: bool,
) -> Vec<ResumableAgentSession> {
    let mut sessions = drafts
        .into_values()
        .filter_map(|draft| {
            let last_used_at = draft.last_used_at?;
            if last_used_at < since
                || !matches_workspace_scope(
                    draft.cwd.as_deref(),
                    workspace_scope,
                    allow_unknown_cwd,
                )
            {
                return None;
            }

            Some(ResumableAgentSession {
                agent_session_id: draft.agent_session_id,
                title: draft
                    .title
                    .unwrap_or_else(|| "Untitled session".to_string()),
                last_used_at,
            })
        })
        .collect::<Vec<_>>();

    sessions.sort_by(|a, b| b.last_used_at.cmp(&a.last_used_at));
    sessions.truncate(limit);
    sessions
}

fn matches_workspace_scope(
    cwd: Option<&Path>,
    workspace_scope: &[PathBuf],
    allow_unknown_cwd: bool,
) -> bool {
    if workspace_scope.is_empty() {
        return true;
    }

    let Some(cwd) = cwd else {
        return allow_unknown_cwd;
    };

    let cwd = normalize_path_for_match(cwd);
    workspace_scope
        .iter()
        .map(|path| normalize_path_for_match(path))
        .any(|scope| is_equal_or_descendant(&cwd, &scope) || is_equal_or_descendant(&scope, &cwd))
}

fn is_equal_or_descendant(path: &str, ancestor: &str) -> bool {
    if path == ancestor {
        return true;
    }

    path.strip_prefix(ancestor)
        .is_some_and(|suffix| suffix.starts_with('/'))
}

fn normalize_path_for_match(path: &Path) -> String {
    let normalized = path
        .to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase();

    // Windows canonical paths use the verbatim prefix, while native agent
    // history usually records an ordinary drive or UNC path. Compare them
    // lexically so history remains discoverable even if a directory is absent.
    if let Some(unc_path) = normalized.strip_prefix("//?/unc/") {
        format!("//{unc_path}")
    } else {
        normalized
            .strip_prefix("//?/")
            .unwrap_or(&normalized)
            .to_string()
    }
}

fn collect_jsonl_files(root: &Path, max_depth: usize) -> Vec<(PathBuf, DateTime<Utc>)> {
    let mut files = Vec::new();
    collect_jsonl_files_inner(root, 0, max_depth, &mut files);
    files.sort_by(|a, b| b.1.cmp(&a.1));
    files
}

fn collect_jsonl_files_inner(
    path: &Path,
    depth: usize,
    max_depth: usize,
    files: &mut Vec<(PathBuf, DateTime<Utc>)>,
) {
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };

        if metadata.is_dir() {
            if depth < max_depth {
                collect_jsonl_files_inner(&path, depth + 1, max_depth, files);
            }
            continue;
        }

        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }

        let modified_at = metadata
            .modified()
            .map(DateTime::<Utc>::from)
            .unwrap_or_else(|_| DateTime::<Utc>::from(SystemTime::UNIX_EPOCH));
        files.push((path, modified_at));
    }
}

fn read_jsonl_values(path: &Path, max_lines: Option<usize>) -> Vec<Value> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };

    let mut values = Vec::new();
    for (idx, line) in BufReader::new(file).lines().enumerate() {
        if max_lines.is_some_and(|max| idx >= max) {
            break;
        }

        let Ok(line) = line else {
            continue;
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        match serde_json::from_str::<Value>(line) {
            Ok(value) => values.push(value),
            Err(err) => {
                tracing::debug!(
                    "Skipping malformed native agent history line in {}: {}",
                    path.display(),
                    err
                );
            }
        }
    }

    values
}

fn string_field<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value.get(field).and_then(Value::as_str)
}

fn datetime_field(value: &Value, field: &str) -> Option<DateTime<Utc>> {
    value.get(field).and_then(parse_datetime_value)
}

fn text_from_content_value(content: &Value) -> Option<String> {
    text_from_content_value_with_separator(content, " ")
}

fn text_from_content_value_with_separator(content: &Value, separator: &str) -> Option<String> {
    if let Some(text) = content.as_str() {
        return Some(text.to_string());
    }

    let texts = content
        .as_array()?
        .iter()
        .filter_map(|part| {
            let part_type = part.get("type").and_then(Value::as_str);
            if !is_text_content_part(part_type) {
                return None;
            }
            part.get("text")
                .or_else(|| part.get("content"))
                .and_then(Value::as_str)
        })
        .collect::<Vec<_>>();

    if texts.is_empty() {
        None
    } else {
        Some(texts.join(separator))
    }
}

fn is_text_content_part(part_type: Option<&str>) -> bool {
    matches!(
        part_type,
        None | Some("text" | "input_text" | "output_text")
    )
}

fn normalize_title(title: String) -> Option<String> {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        return None;
    }

    let truncated = title.chars().take(TITLE_MAX_CHARS).collect::<String>();
    if title.chars().count() > TITLE_MAX_CHARS {
        Some(format!("{}...", truncated))
    } else {
        Some(truncated)
    }
}

fn normalize_preview_role(role: &str) -> Option<String> {
    match role.trim().to_ascii_lowercase().as_str() {
        "user" => Some("user".to_string()),
        "assistant" => Some("assistant".to_string()),
        _ => None,
    }
}

fn normalize_preview_content(content: String) -> Option<String> {
    let normalized = content
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();

    if normalized.is_empty() {
        return None;
    }

    let truncated = normalized
        .chars()
        .take(PREVIEW_ENTRY_MAX_CHARS)
        .collect::<String>();
    if normalized.chars().count() > PREVIEW_ENTRY_MAX_CHARS {
        Some(format!("{}...", truncated))
    } else {
        Some(truncated)
    }
}

fn parse_datetime(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|timestamp| timestamp.with_timezone(&Utc))
}

fn parse_datetime_value(value: &Value) -> Option<DateTime<Utc>> {
    if let Some(timestamp) = value.as_str() {
        return parse_datetime(timestamp);
    }

    if let Some(timestamp) = value.as_i64() {
        return parse_unix_timestamp_i128(timestamp.into());
    }

    if let Some(timestamp) = value.as_u64() {
        return parse_unix_timestamp_i128(i128::from(timestamp));
    }

    value.as_f64().and_then(parse_unix_timestamp_f64)
}

fn parse_unix_timestamp_i128(timestamp: i128) -> Option<DateTime<Utc>> {
    const MILLIS_THRESHOLD: i128 = 10_000_000_000;

    let (seconds, nanos) = if timestamp <= -MILLIS_THRESHOLD || timestamp >= MILLIS_THRESHOLD {
        let seconds = timestamp.div_euclid(1_000);
        let millis = timestamp.rem_euclid(1_000);
        (seconds, (millis * 1_000_000) as u32)
    } else {
        (timestamp, 0)
    };

    let seconds = i64::try_from(seconds).ok()?;
    Utc.timestamp_opt(seconds, nanos).single()
}

fn parse_unix_timestamp_f64(timestamp: f64) -> Option<DateTime<Utc>> {
    const MILLIS_THRESHOLD: f64 = 10_000_000_000.0;

    if !timestamp.is_finite() {
        return None;
    }

    let timestamp = if timestamp.abs() >= MILLIS_THRESHOLD {
        timestamp / 1_000.0
    } else {
        timestamp
    };
    let seconds = timestamp.floor();

    if seconds < i64::MIN as f64 || seconds > i64::MAX as f64 {
        return None;
    }

    let mut seconds = seconds as i64;
    let mut nanos = ((timestamp - seconds as f64) * 1_000_000_000.0).round() as u32;
    if nanos == 1_000_000_000 {
        seconds = seconds.checked_add(1)?;
        nanos = 0;
    }

    Utc.timestamp_opt(seconds, nanos).single()
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .or_else(|| {
            let drive = std::env::var_os("HOMEDRIVE")?;
            let path = std::env::var_os("HOMEPATH")?;
            Some(PathBuf::from(format!(
                "{}{}",
                drive.to_string_lossy(),
                path.to_string_lossy()
            )))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_malformed_jsonl_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("history.jsonl");
        fs::write(
            &path,
            "{\"session_id\":\"one\",\"ts\":\"2026-07-06T00:00:00Z\",\"text\":\"Hello\"}\nnot-json\n",
        )
        .expect("write jsonl");

        let values = read_jsonl_values(&path, None);

        assert_eq!(values.len(), 1);
        assert_eq!(string_field(&values[0], "session_id"), Some("one"));
    }

    #[test]
    fn parses_numeric_unix_timestamps() {
        assert_eq!(
            parse_datetime_value(&serde_json::json!(1_783_340_712)),
            Utc.timestamp_opt(1_783_340_712, 0).single()
        );
        assert_eq!(
            parse_datetime_value(&serde_json::json!(1_783_340_712_123i64)),
            Utc.timestamp_opt(1_783_340_712, 123_000_000).single()
        );
    }

    #[test]
    fn codex_history_updates_existing_session_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("history.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"session_id\":\"one\",\"ts\":1783340712,\"text\":\"Hello\"}\n",
                "{\"session_id\":\"prompt-only\",\"ts\":1783340713,\"text\":\"Should not create\"}\n",
            ),
        )
        .expect("write jsonl");

        let mut drafts = HashMap::new();
        update_draft(
            &mut drafts,
            "one",
            None,
            None,
            NativeTitleSource::SessionContent,
            None,
        );
        read_codex_history(&path, &mut drafts);

        let draft = drafts.get("one").expect("draft");
        assert_eq!(
            draft.last_used_at,
            Utc.timestamp_opt(1_783_340_712, 0).single()
        );
        assert_eq!(draft.title.as_deref(), Some("Hello"));
        assert!(!drafts.contains_key("prompt-only"));
    }

    #[test]
    fn codex_many_history_prompts_keep_one_session_candidate() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index_path = dir.path().join("session_index.jsonl");
        let history_path = dir.path().join("history.jsonl");
        fs::write(
            &index_path,
            "{\"id\":\"one\",\"created_at\":\"2026-07-06T00:00:00Z\"}\n",
        )
        .expect("write index");
        fs::write(
            &history_path,
            concat!(
                "{\"session_id\":\"one\",\"ts\":\"2026-07-06T00:00:01Z\",\"text\":\"first prompt\"}\n",
                "{\"session_id\":\"one\",\"ts\":\"2026-07-06T00:00:02Z\",\"text\":\"second prompt\"}\n",
                "{\"session_id\":\"prompt-only\",\"ts\":\"2026-07-06T00:00:03Z\",\"text\":\"should not create\"}\n",
            ),
        )
        .expect("write history");

        let mut drafts = HashMap::new();
        read_codex_session_index(&index_path, &mut drafts);
        read_codex_history(&history_path, &mut drafts);
        let sessions = finalize_sessions(
            drafts,
            Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0)
                .single()
                .expect("timestamp"),
            10,
            &[],
            true,
        );

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].agent_session_id, "one");
        assert_eq!(sessions[0].title, "first prompt");
        assert_eq!(
            sessions[0].last_used_at,
            Utc.with_ymd_and_hms(2026, 7, 6, 0, 0, 2).single().unwrap()
        );
    }

    #[test]
    fn codex_index_and_session_file_merge_into_one_candidate() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sessions_dir = dir.path().join("sessions");
        fs::create_dir(&sessions_dir).expect("create sessions dir");
        let index_path = dir.path().join("session_index.jsonl");
        let session_path = sessions_dir.join("rollout.jsonl");
        fs::write(
            &index_path,
            "{\"id\":\"one\",\"thread_name\":\"Explicit thread title\",\"updated_at\":\"2026-07-06T00:00:00Z\"}\n",
        )
        .expect("write index");
        fs::write(
            &session_path,
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"session_id\":\"one\",\"cwd\":\"C:/repo\"}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"Message title\"}]}}\n",
            ),
        )
        .expect("write session");

        let mut drafts = HashMap::new();
        read_codex_session_index(&index_path, &mut drafts);
        read_codex_session_files(&sessions_dir, &mut drafts);

        assert_eq!(drafts.len(), 1);
        let draft = drafts.get("one").expect("draft");
        assert_eq!(draft.title.as_deref(), Some("Explicit thread title"));
        assert_eq!(draft.cwd.as_deref(), Some(Path::new("C:/repo")));
    }

    #[test]
    fn codex_fork_chain_collapses_to_latest_leaf() {
        let root_time = Utc.with_ymd_and_hms(2026, 7, 6, 0, 0, 0).single().unwrap();
        let child_time = Utc.with_ymd_and_hms(2026, 7, 6, 0, 1, 0).single().unwrap();
        let leaf_time = Utc.with_ymd_and_hms(2026, 7, 6, 0, 2, 0).single().unwrap();
        let mut drafts = HashMap::new();

        update_draft(
            &mut drafts,
            "root",
            Some(root_time),
            Some("First prompt".to_string()),
            NativeTitleSource::SessionContent,
            Some(PathBuf::from("C:/repo/root")),
        );
        update_draft(
            &mut drafts,
            "child",
            Some(child_time),
            Some("Second prompt".to_string()),
            NativeTitleSource::SessionContent,
            Some(PathBuf::from("C:/repo/child")),
        );
        drafts.get_mut("child").unwrap().forked_from_id = Some("root".to_string());
        update_draft(
            &mut drafts,
            "leaf",
            Some(leaf_time),
            Some("Third prompt".to_string()),
            NativeTitleSource::SessionContent,
            Some(PathBuf::from("C:/repo/leaf")),
        );
        drafts.get_mut("leaf").unwrap().forked_from_id = Some("child".to_string());

        let collapsed = collapse_codex_fork_chains(drafts);

        assert_eq!(collapsed.len(), 1);
        let draft = collapsed.get("leaf").expect("latest leaf draft");
        assert_eq!(draft.agent_session_id, "leaf");
        assert_eq!(draft.title.as_deref(), Some("First prompt"));
        assert_eq!(draft.last_used_at, Some(leaf_time));
        assert_eq!(draft.cwd.as_deref(), Some(Path::new("C:/repo/leaf")));
    }

    #[test]
    fn codex_ignores_synthetic_user_context_in_title_and_preview() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rollout.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"session_id\":\"native-session\",\"cwd\":\"C:/repo\"}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"<environment_context>injected</environment_context>\"}]}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"Real prompt\"}]}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Real answer\"}]}}\n",
            ),
        )
        .expect("write session");

        let mut drafts = HashMap::new();
        read_codex_session_files(dir.path(), &mut drafts);
        let draft = drafts.get("native-session").expect("draft");
        assert_eq!(draft.title.as_deref(), Some("Real prompt"));

        let entries = read_codex_preview_entries_from_file(&path);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].content, "Real prompt");
        assert_eq!(entries[1].content, "Real answer");
    }

    #[test]
    fn claude_history_updates_existing_session_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("history.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"sessionId\":\"one\",\"timestamp\":\"2026-07-06T00:00:00Z\",\"display\":\"Display title\",\"cwd\":\"C:/repo\"}\n",
                "{\"sessionId\":\"prompt-only\",\"timestamp\":\"2026-07-06T00:00:01Z\",\"prompt\":\"Should not create\"}\n",
            ),
        )
        .expect("write jsonl");

        let mut drafts = HashMap::new();
        update_draft(
            &mut drafts,
            "one",
            None,
            Some("Project file title".to_string()),
            NativeTitleSource::SessionContent,
            None,
        );
        read_claude_history(&path, &mut drafts);

        let draft = drafts.get("one").expect("draft");
        assert_eq!(
            draft.last_used_at,
            Utc.with_ymd_and_hms(2026, 7, 6, 0, 0, 0).single()
        );
        assert_eq!(draft.title.as_deref(), Some("Display title"));
        assert_eq!(draft.cwd.as_deref(), Some(Path::new("C:/repo")));
        assert!(!drafts.contains_key("prompt-only"));
    }

    #[test]
    fn claude_project_file_creates_candidate_and_preview() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("native-session.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"type\":\"user\",\"sessionId\":\"native-session\",\"cwd\":\"C:/repo\",\"timestamp\":\"2026-07-06T00:00:00Z\",\"message\":{\"role\":\"user\",\"content\":\"hello\"}}\n",
                "{\"type\":\"assistant\",\"sessionId\":\"native-session\",\"timestamp\":\"2026-07-06T00:00:01Z\",\"message\":{\"role\":\"assistant\",\"content\":\"hi\"}}\n",
            ),
        )
        .expect("write jsonl");

        let mut drafts = HashMap::new();
        read_claude_project_files(dir.path(), &mut drafts);
        let draft = drafts.get("native-session").expect("draft");
        assert_eq!(draft.title.as_deref(), Some("hello"));
        assert_eq!(draft.cwd.as_deref(), Some(Path::new("C:/repo")));

        let entries = read_claude_preview_entries_from_file(&path);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].content, "hello");
        assert_eq!(entries[1].content, "hi");
    }

    #[test]
    fn unsupported_provider_returns_no_native_sessions_or_preview() {
        let since = Utc
            .with_ymd_and_hms(2026, 7, 1, 0, 0, 0)
            .single()
            .expect("timestamp");

        assert!(
            list_native_resumable_agent_sessions("unsupported", since, 10, &[], true).is_empty()
        );
        assert!(
            get_native_agent_session_preview("unsupported", "session", 20, &[], true).is_none()
        );
    }

    #[test]
    fn codex_session_files_accept_payload_session_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rollout.jsonl");
        fs::write(
            &path,
            "{\"timestamp\":\"2026-07-06T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"session_id\":\"native-session\",\"cwd\":\"C:/repo\"}}\n",
        )
        .expect("write jsonl");

        let mut drafts = HashMap::new();
        read_codex_session_files(dir.path(), &mut drafts);

        let draft = drafts.get("native-session").expect("draft");
        assert_eq!(draft.cwd.as_deref(), Some(Path::new("C:/repo")));
    }

    #[test]
    fn gemini_project_reader_requires_header_and_filters_synthetic_context() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path().join("project");
        let chats = project.join("chats");
        fs::create_dir_all(&chats).expect("create chats dir");
        fs::write(project.join(".project_root"), "C:/repo\n").expect("write project root");
        let path = chats.join("session-1.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"sessionId\":\"gemini-1\",\"startTime\":\"2026-07-06T00:00:00Z\",\"lastUpdated\":\"2026-07-06T00:01:00Z\",\"kind\":\"main\"}\n",
                "{\"type\":\"user\",\"content\":[{\"text\":\"<session_context>injected</session_context>\"}]}\n",
                "{\"type\":\"user\",\"content\":[{\"text\":\"Implement the feature\"}]}\n",
                "{\"type\":\"gemini\",\"content\":[{\"text\":\"Done\"}]}\n",
                "{\"$set\":{\"lastUpdated\":\"2026-07-06T00:02:00Z\"}}\n",
            ),
        )
        .expect("write gemini session");

        let drafts = list_gemini_sessions_from_root(dir.path());
        let draft = drafts.get("gemini-1").expect("gemini draft");
        assert_eq!(draft.cwd.as_deref(), Some(Path::new("C:/repo")));
        assert_eq!(draft.title.as_deref(), Some("Implement the feature"));

        let entries = read_gemini_preview_entries_from_file(&path);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].role, "user");
        assert_eq!(entries[0].content, "Implement the feature");
        assert_eq!(entries[1].role, "assistant");

        let malformed = chats.join("foreign.jsonl");
        fs::write(&malformed, "{\"type\":\"user\",\"content\":\"foreign\"}\n")
            .expect("write malformed session");
        assert_eq!(list_gemini_sessions_from_root(dir.path()).len(), 1);
    }

    #[test]
    fn oh_my_pi_reader_uses_session_header_scope_and_preview() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path().join("-repo");
        fs::create_dir_all(&project).expect("create project dir");
        let path = project.join("2026-07-06T000000Z_native-1.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"type\":\"session\",\"id\":\"native-1\",\"cwd\":\"C:/repo\",\"title\":\"Existing task\",\"timestamp\":\"2026-07-06T00:00:00Z\"}\n",
                "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Start\"}]}}\n",
                "{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Answer\"}]}}\n",
            ),
        )
        .expect("write omp session");

        let drafts = list_oh_my_pi_sessions_from_root(dir.path());
        let draft = drafts.get("native-1").expect("omp draft");
        assert_eq!(draft.cwd.as_deref(), Some(Path::new("C:/repo")));
        assert_eq!(draft.title.as_deref(), Some("Existing task"));

        let path = find_oh_my_pi_session_file(dir.path(), "native-1").expect("find omp session");
        let entries = read_jsonl_values(&path, Some(20_000))
            .into_iter()
            .filter_map(|value| {
                let (role, content, timestamp) = omp_message_from_value(&value)?;
                Some(NativeSessionPreviewEntry {
                    role,
                    content: normalize_preview_content(content)?,
                    timestamp,
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].content, "Start");
        assert_eq!(entries[1].role, "assistant");

        fs::write(
            project.join("foreign.jsonl"),
            "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"foreign\"}}\n",
        )
        .expect("write foreign file");
        assert_eq!(list_oh_my_pi_sessions_from_root(dir.path()).len(), 1);
    }

    #[test]
    fn workspace_scope_matches_nested_paths_and_keeps_unknown_cwd() {
        let scope = vec![PathBuf::from("C:/repo")];

        assert!(matches_workspace_scope(
            Some(Path::new("C:/repo/packages/app")),
            &scope,
            true
        ));
        assert!(!matches_workspace_scope(
            Some(Path::new("C:/other/packages/app")),
            &scope,
            true
        ));
        assert!(matches_workspace_scope(None, &scope, true));
        assert!(!matches_workspace_scope(None, &scope, false));
    }

    #[test]
    fn workspace_scope_requires_path_boundaries() {
        let scope = vec![PathBuf::from("C:/repo")];

        assert!(matches_workspace_scope(
            Some(Path::new("C:/repo")),
            &scope,
            false
        ));
        assert!(!matches_workspace_scope(
            Some(Path::new("C:/repo-child")),
            &scope,
            false
        ));
        assert!(!matches_workspace_scope(
            Some(Path::new("C:/repo2/nested")),
            &scope,
            false
        ));
    }

    #[test]
    fn workspace_scope_matches_windows_verbatim_drive_paths() {
        let scope = vec![PathBuf::from(r"\\?\F:\Mydev2023\serverSpace\")];

        assert!(matches_workspace_scope(
            Some(Path::new("f:/mydev2023/serverSpace")),
            &scope,
            false
        ));
        assert!(matches_workspace_scope(
            Some(Path::new(r"F:\Mydev2023\serverSpace\nested")),
            &scope,
            false
        ));
        assert!(matches_workspace_scope(
            Some(Path::new(r"\\?\F:\Mydev2023\serverSpace")),
            &[PathBuf::from("F:/Mydev2023/serverSpace")],
            false
        ));
        assert!(!matches_workspace_scope(
            Some(Path::new(r"F:\Mydev2023\serverSpace-other")),
            &scope,
            false
        ));
        assert!(!matches_workspace_scope(
            Some(Path::new(r"G:\Mydev2023\serverSpace")),
            &scope,
            false
        ));
        assert!(!matches_workspace_scope(None, &scope, false));
    }

    #[test]
    fn workspace_scope_matches_windows_verbatim_unc_paths() {
        let scope = vec![PathBuf::from(r"\\?\UNC\Server\Share\repo")];

        assert!(matches_workspace_scope(
            Some(Path::new(r"\\server\share\repo")),
            &scope,
            false
        ));
        assert!(matches_workspace_scope(
            Some(Path::new("//server/share/repo/nested")),
            &scope,
            false
        ));
        assert!(matches_workspace_scope(
            Some(Path::new(r"\\?\UNC\Server\Share\repo")),
            &[PathBuf::from(r"\\server\share\repo")],
            false
        ));
        assert!(!matches_workspace_scope(
            Some(Path::new(r"\\server\share\repo-other")),
            &scope,
            false
        ));
        assert!(!matches_workspace_scope(
            Some(Path::new(r"\\other-server\share\repo")),
            &scope,
            false
        ));
    }

    #[test]
    fn codex_native_metadata_is_listed_in_verbatim_workspace_scope() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rollout.jsonl");
        fs::write(
            &path,
            r#"{"type":"session_meta","payload":{"id":"native-session","cwd":"F:\\Mydev2023\\serverSpace"}}"#,
        )
        .expect("write native metadata");

        let mut drafts = HashMap::new();
        read_codex_session_files(dir.path(), &mut drafts);
        let sessions = finalize_sessions(
            drafts,
            DateTime::<Utc>::from(SystemTime::UNIX_EPOCH),
            10,
            &[PathBuf::from(r"\\?\F:\Mydev2023\serverSpace")],
            false,
        );

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].agent_session_id, "native-session");
    }

    #[test]
    fn prefers_explicit_titles_over_prompt_and_message_text() {
        let mut drafts = HashMap::new();

        update_draft(
            &mut drafts,
            "session-1",
            None,
            Some("Initial prompt".to_string()),
            NativeTitleSource::Prompt,
            None,
        );
        update_draft(
            &mut drafts,
            "session-1",
            None,
            Some("Actual thread title".to_string()),
            NativeTitleSource::ExplicitTitle,
            None,
        );
        update_draft(
            &mut drafts,
            "session-1",
            None,
            Some("Later content line".to_string()),
            NativeTitleSource::SessionContent,
            None,
        );

        assert_eq!(
            drafts
                .get("session-1")
                .and_then(|draft| draft.title.as_deref()),
            Some("Actual thread title")
        );
    }

    #[test]
    fn keeps_first_title_when_sources_have_same_priority() {
        let mut drafts = HashMap::new();

        update_draft(
            &mut drafts,
            "session-1",
            None,
            Some("First prompt".to_string()),
            NativeTitleSource::Prompt,
            None,
        );
        update_draft(
            &mut drafts,
            "session-1",
            None,
            Some("Second prompt".to_string()),
            NativeTitleSource::Prompt,
            None,
        );

        assert_eq!(
            drafts
                .get("session-1")
                .and_then(|draft| draft.title.as_deref()),
            Some("First prompt")
        );
    }

    #[test]
    fn codex_preview_reads_response_item_messages() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rollout.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"session_id\":\"native-session\",\"cwd\":\"C:/repo\"}}\n",
                "{\"timestamp\":\"2026-07-06T00:00:00Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"first prompt\"}]}}\n",
                "{\"timestamp\":\"2026-07-06T00:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"first answer\"}]}}\n",
                "{\"timestamp\":\"2026-07-06T00:00:02Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"second prompt\"}]}}\n",
                "{\"timestamp\":\"2026-07-06T00:00:03Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"second answer\"}]}}\n",
            ),
        )
        .expect("write jsonl");

        let entries = read_codex_preview_entries_from_file(&path);
        let (limited, truncated) = limit_preview_entries(entries, 1);

        assert!(truncated);
        assert_eq!(limited.len(), 2);
        assert_eq!(limited[0].role, "user");
        assert_eq!(limited[0].content, "second prompt");
        assert_eq!(limited[1].role, "assistant");
        assert_eq!(limited[1].content, "second answer");
    }

    #[test]
    fn claude_preview_reads_message_content() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("native-session.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"type\":\"user\",\"sessionId\":\"native-session\",\"timestamp\":\"2026-07-06T00:00:00Z\",\"message\":{\"role\":\"user\",\"content\":\"hello\"}}\n",
                "{\"type\":\"assistant\",\"sessionId\":\"native-session\",\"timestamp\":\"2026-07-06T00:00:01Z\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"hi there\"}]}}\n",
            ),
        )
        .expect("write jsonl");

        let entries = read_claude_preview_entries_from_file(&path);

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].role, "user");
        assert_eq!(entries[0].content, "hello");
        assert_eq!(entries[1].role, "assistant");
        assert_eq!(entries[1].content, "hi there");
    }

    #[test]
    fn default_preview_limit_keeps_latest_twenty_user_turns() {
        let entries = (1..=21)
            .flat_map(|index| {
                [
                    NativeSessionPreviewEntry {
                        role: "user".to_string(),
                        content: format!("question {index}"),
                        timestamp: None,
                    },
                    NativeSessionPreviewEntry {
                        role: "assistant".to_string(),
                        content: format!("answer {index}"),
                        timestamp: None,
                    },
                ]
            })
            .collect::<Vec<_>>();

        let (limited, truncated) =
            limit_preview_entries(entries, DEFAULT_NATIVE_SESSION_PREVIEW_TURNS);

        assert!(truncated);
        assert_eq!(limited.len(), 40);
        assert_eq!(limited[0].content, "question 2");
        assert_eq!(limited[39].content, "answer 21");
    }
}
