use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    AGENT_EVENT_PAYLOAD_VERSION, AGENT_EVENT_SCHEMA_VERSION, AgentEventEnvelope, AgentEventPayload,
    AgentRunStatus, CanonicalMessage, NativeAuditReference, ProjectionStatus,
    RUN_STATE_REDUCER_VERSION, RUN_STATE_SCHEMA_VERSION, RunState,
};

pub const NATIVE_AUDIT_SCHEMA_VERSION: u16 = 1;
pub const NATIVE_AUDIT_ROOT_RELATIVE: &str = "runtime/native-audit/v1";
pub const NATIVE_AUDIT_MANIFEST_FILE: &str = "manifest.json";
pub const NATIVE_AUDIT_FRAMES_FILE: &str = "frames.jsonl";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeAuditDirection {
    Input,
    Output,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeAuditChannel {
    CanonicalInput,
    NativeInput,
    NativeOutput,
    Stdin,
    Stdout,
    Stderr,
    Rpc,
    RpcRequest,
    RpcResponse,
    RpcNotification,
    Sidecar,
    #[serde(untagged)]
    Other(String),
}

impl NativeAuditChannel {
    pub fn as_str(&self) -> &str {
        match self {
            Self::CanonicalInput => "canonical_input",
            Self::NativeInput => "native_input",
            Self::NativeOutput => "native_output",
            Self::Stdin => "stdin",
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
            Self::Rpc => "rpc",
            Self::RpcRequest => "rpc_request",
            Self::RpcResponse => "rpc_response",
            Self::RpcNotification => "rpc_notification",
            Self::Sidecar => "sidecar",
            Self::Other(value) => value,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativePayloadEncoding {
    Base64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeAuditFrame {
    pub audit_format_version: u16,
    pub sequence: u64,
    pub timestamp: DateTime<Utc>,
    pub direction: NativeAuditDirection,
    pub channel: NativeAuditChannel,
    pub content_type: String,
    pub correlation_id: Uuid,
    pub payload_encoding: NativePayloadEncoding,
    pub payload_base64: String,
    pub payload_checksum: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

impl NativeAuditFrame {
    pub fn from_bytes(
        sequence: u64,
        timestamp: DateTime<Utc>,
        direction: NativeAuditDirection,
        channel: NativeAuditChannel,
        content_type: impl Into<String>,
        correlation_id: Uuid,
        payload: &[u8],
        metadata: Option<Value>,
    ) -> Self {
        Self {
            audit_format_version: NATIVE_AUDIT_SCHEMA_VERSION,
            sequence,
            timestamp,
            direction,
            channel,
            content_type: content_type.into(),
            correlation_id,
            payload_encoding: NativePayloadEncoding::Base64,
            payload_base64: base64::engine::general_purpose::STANDARD.encode(payload),
            payload_checksum: sha256_hex(payload),
            metadata,
        }
    }

    pub fn payload_bytes(&self) -> Result<Vec<u8>, NativeAuditError> {
        let payload = base64::engine::general_purpose::STANDARD
            .decode(&self.payload_base64)
            .map_err(NativeAuditError::InvalidBase64)?;
        let checksum = sha256_hex(&payload);
        if checksum != self.payload_checksum {
            return Err(NativeAuditError::PayloadChecksumMismatch {
                sequence: self.sequence,
                expected: self.payload_checksum.clone(),
                actual: checksum,
            });
        }
        Ok(payload)
    }

    fn validate(&self, expected_sequence: u64) -> Result<(), NativeAuditError> {
        if self.audit_format_version != NATIVE_AUDIT_SCHEMA_VERSION {
            return Err(NativeAuditError::UnsupportedVersion {
                provided: self.audit_format_version,
                current: NATIVE_AUDIT_SCHEMA_VERSION,
            });
        }
        if self.sequence != expected_sequence {
            return Err(NativeAuditError::SequenceMismatch {
                expected: expected_sequence,
                actual: self.sequence,
            });
        }
        self.payload_bytes().map(|_| ())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeAuditIntegrityStatus {
    Open,
    Complete,
    Partial,
    Corrupt,
    AuditFailed,
    Recovered,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeAuditMetadata {
    pub session_id: Uuid,
    pub agent_run_id: Uuid,
    pub turn_id: Uuid,
    pub run_attempt_id: Uuid,
    pub run_attempt_number: u32,
    pub provider_id: String,
    pub runtime_profile_id: String,
    pub workspace_path: String,
    pub runtime_version: Option<String>,
    pub protocol_version: Option<String>,
    pub adapter_version: String,
    pub mapper_version: String,
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeAuditManifest {
    pub audit_schema_version: u16,
    pub session_id: Uuid,
    pub agent_run_id: Uuid,
    pub turn_id: Uuid,
    pub run_attempt_id: Uuid,
    pub run_attempt_number: u32,
    pub provider_id: String,
    pub runtime_profile_id: String,
    pub workspace_path: String,
    pub runtime_version: Option<String>,
    pub protocol_version: Option<String>,
    pub adapter_version: String,
    pub mapper_version: String,
    pub frame_count: u64,
    pub first_sequence: Option<u64>,
    pub last_sequence: Option<u64>,
    pub final_checksum: Option<String>,
    pub integrity_status: NativeAuditIntegrityStatus,
    pub created_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    pub manifest_relative_path: String,
    pub frames_relative_path: String,
    pub raw_content_trusted: bool,
}

impl NativeAuditManifest {
    pub fn versions(&self) -> NativeAuditVersionSet {
        NativeAuditVersionSet {
            audit_schema_version: self.audit_schema_version,
            runtime_version: self.runtime_version.clone(),
            adapter_version: self.adapter_version.clone(),
            protocol_version: self.protocol_version.clone(),
            mapper_version: self.mapper_version.clone(),
        }
    }

    pub fn index(&self) -> NativeAuditStreamIndex {
        NativeAuditStreamIndex {
            id: None,
            session_id: self.session_id,
            agent_run_id: self.agent_run_id,
            run_attempt_id: self.run_attempt_id,
            audit_schema_version: self.audit_schema_version,
            runtime_version: self.runtime_version.clone(),
            protocol_version: self.protocol_version.clone(),
            adapter_version: self.adapter_version.clone(),
            mapper_version: self.mapper_version.clone(),
            manifest_relative_path: self.manifest_relative_path.clone(),
            frames_relative_path: self.frames_relative_path.clone(),
            first_sequence: self.first_sequence,
            last_sequence: self.last_sequence,
            final_checksum: self.final_checksum.clone(),
            integrity_status: serde_json::to_string(&self.integrity_status)
                .unwrap_or_else(|_| "\"open\"".to_string())
                .trim_matches('"')
                .to_string(),
            created_at: self.created_at,
            closed_at: self.closed_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeAuditStreamIndex {
    pub id: Option<Uuid>,
    pub session_id: Uuid,
    pub agent_run_id: Uuid,
    pub run_attempt_id: Uuid,
    pub audit_schema_version: u16,
    pub runtime_version: Option<String>,
    pub protocol_version: Option<String>,
    pub adapter_version: String,
    pub mapper_version: String,
    pub manifest_relative_path: String,
    pub frames_relative_path: String,
    pub first_sequence: Option<u64>,
    pub last_sequence: Option<u64>,
    pub final_checksum: Option<String>,
    pub integrity_status: String,
    pub created_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeAuditVersionSet {
    pub audit_schema_version: u16,
    pub runtime_version: Option<String>,
    pub adapter_version: String,
    pub protocol_version: Option<String>,
    pub mapper_version: String,
}

#[derive(Debug, thiserror::Error)]
pub enum NativeAuditError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("invalid base64 payload: {0}")]
    InvalidBase64(#[from] base64::DecodeError),
    #[error("audit stream is already closed")]
    Closed,
    #[error("audit stream already exists at {0}")]
    AlreadyExists(PathBuf),
    #[error("audit frame sequence expected {expected}, got {actual}")]
    SequenceMismatch { expected: u64, actual: u64 },
    #[error("audit frame {sequence} payload checksum mismatch: expected {expected}, got {actual}")]
    PayloadChecksumMismatch {
        sequence: u64,
        expected: String,
        actual: String,
    },
    #[error("audit frames checksum mismatch: expected {expected}, got {actual}")]
    StreamChecksumMismatch { expected: String, actual: String },
    #[error("audit frame stream is truncated")]
    TruncatedFrame,
    #[error("audit manifest is partial or still open")]
    PartialManifest,
    #[error("audit schema version {provided} is not supported (current {current})")]
    UnsupportedVersion { provided: u16, current: u16 },
    #[error("audit replay mapper mismatch for {field}: expected {expected:?}, got {actual:?}")]
    MapperMismatch {
        field: &'static str,
        expected: String,
        actual: String,
    },
    #[error("audit frame {0} is malformed")]
    MalformedFrame(u64),
    #[error("audit writer failed closed after a durable write error")]
    FailedClosed,
    #[error("audit fixture {kind} does not match replay output")]
    FixtureMismatch { kind: &'static str },
}

pub struct NativeAuditWriter {
    directory: PathBuf,
    manifest_path: PathBuf,
    frames_path: PathBuf,
    frames: File,
    manifest: NativeAuditManifest,
    next_sequence: u64,
    stream_hasher: Sha256,
    closed: bool,
    failed: bool,
}

pub type NativeAuditStream = NativeAuditWriter;
pub type NativeFrame = NativeAuditFrame;
pub type AuditManifest = NativeAuditManifest;
pub type NativeAuditReader = AuditBundle;

impl NativeAuditWriter {
    pub fn create(metadata: NativeAuditMetadata) -> Result<Self, NativeAuditError> {
        Self::create_in(workspace_utils::assets::asset_dir(), metadata)
    }

    pub fn new(
        root: impl AsRef<Path>,
        metadata: NativeAuditMetadata,
    ) -> Result<Self, NativeAuditError> {
        Self::create_in(root, metadata)
    }

    pub fn create_in(
        root: impl AsRef<Path>,
        metadata: NativeAuditMetadata,
    ) -> Result<Self, NativeAuditError> {
        if metadata.run_attempt_number == 0 {
            return Err(NativeAuditError::SequenceMismatch {
                expected: 1,
                actual: 0,
            });
        }
        let relative = stream_relative_paths(&metadata);
        let directory = root.as_ref().join(&relative.directory);
        fs::create_dir_all(&directory)?;
        let manifest_path = directory.join(NATIVE_AUDIT_MANIFEST_FILE);
        let frames_path = directory.join(NATIVE_AUDIT_FRAMES_FILE);
        if manifest_path.exists() || frames_path.exists() {
            return Err(NativeAuditError::AlreadyExists(directory));
        }
        let frames = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&frames_path)?;
        let manifest = NativeAuditManifest {
            audit_schema_version: NATIVE_AUDIT_SCHEMA_VERSION,
            session_id: metadata.session_id,
            agent_run_id: metadata.agent_run_id,
            turn_id: metadata.turn_id,
            run_attempt_id: metadata.run_attempt_id,
            run_attempt_number: metadata.run_attempt_number,
            provider_id: metadata.provider_id,
            runtime_profile_id: metadata.runtime_profile_id,
            workspace_path: metadata.workspace_path,
            runtime_version: metadata.runtime_version,
            protocol_version: metadata.protocol_version,
            adapter_version: metadata.adapter_version,
            mapper_version: metadata.mapper_version,
            frame_count: 0,
            first_sequence: None,
            last_sequence: None,
            final_checksum: None,
            integrity_status: NativeAuditIntegrityStatus::Open,
            created_at: metadata.created_at,
            closed_at: None,
            manifest_relative_path: relative.manifest,
            frames_relative_path: relative.frames,
            raw_content_trusted: true,
        };
        write_atomic_json(&manifest_path, &manifest)?;
        Ok(Self {
            directory,
            manifest_path,
            frames_path,
            frames,
            manifest,
            next_sequence: 1,
            stream_hasher: Sha256::new(),
            closed: false,
            failed: false,
        })
    }

    pub fn manifest(&self) -> &NativeAuditManifest {
        &self.manifest
    }

    pub fn paths(&self) -> (&Path, &Path) {
        (&self.manifest_path, &self.frames_path)
    }

    pub fn append(
        &mut self,
        frame: NativeAuditFrame,
    ) -> Result<NativeAuditReference, NativeAuditError> {
        if self.closed {
            return Err(NativeAuditError::Closed);
        }
        if self.failed {
            return Err(NativeAuditError::FailedClosed);
        }
        if let Err(error) = frame.validate(self.next_sequence) {
            self.failed = true;
            return Err(error);
        }
        let mut line = serde_json::to_vec(&frame)?;
        line.push(b'\n');
        if let Err(error) = self
            .frames
            .write_all(&line)
            .and_then(|_| self.frames.sync_data())
        {
            self.failed = true;
            return Err(NativeAuditError::Io(error));
        }
        self.stream_hasher.update(&line);
        self.manifest.frame_count += 1;
        self.manifest.first_sequence.get_or_insert(frame.sequence);
        self.manifest.last_sequence = Some(frame.sequence);
        self.next_sequence += 1;
        Ok(NativeAuditReference {
            stream_id: self.manifest.run_attempt_id,
            sequence: frame.sequence,
            checksum: Some(frame.payload_checksum),
        })
    }

    pub fn append_frame(
        &mut self,
        frame: NativeAuditFrame,
    ) -> Result<NativeAuditReference, NativeAuditError> {
        self.append(frame)
    }

    pub fn append_bytes(
        &mut self,
        direction: NativeAuditDirection,
        channel: NativeAuditChannel,
        content_type: impl Into<String>,
        correlation_id: Uuid,
        payload: &[u8],
        metadata: Option<Value>,
    ) -> Result<NativeAuditReference, NativeAuditError> {
        let frame = NativeAuditFrame::from_bytes(
            self.next_sequence,
            Utc::now(),
            direction,
            channel,
            content_type,
            correlation_id,
            payload,
            metadata,
        );
        self.append(frame)
    }

    pub fn append_canonical_input(
        &mut self,
        message: &CanonicalMessage,
        correlation_id: Uuid,
    ) -> Result<NativeAuditReference, NativeAuditError> {
        let payload = serde_json::to_vec(message)?;
        self.append_bytes(
            NativeAuditDirection::Input,
            NativeAuditChannel::CanonicalInput,
            "application/json",
            correlation_id,
            &payload,
            None,
        )
    }

    pub fn append_canonical_input_reference(
        &mut self,
        reference: &NativeAuditReference,
        correlation_id: Uuid,
    ) -> Result<NativeAuditReference, NativeAuditError> {
        let metadata = serde_json::json!({
            "stream_id": reference.stream_id,
            "sequence": reference.sequence,
            "checksum": reference.checksum,
        });
        self.append_bytes(
            NativeAuditDirection::Input,
            NativeAuditChannel::CanonicalInput,
            "application/vnd.vibe-kanban.native-audit-reference+json",
            correlation_id,
            &[],
            Some(metadata),
        )
    }

    pub fn append_native_input(
        &mut self,
        channel: NativeAuditChannel,
        content_type: impl Into<String>,
        correlation_id: Uuid,
        payload: &[u8],
    ) -> Result<NativeAuditReference, NativeAuditError> {
        self.append_bytes(
            NativeAuditDirection::Input,
            channel,
            content_type,
            correlation_id,
            payload,
            None,
        )
    }

    pub fn record_native_input(
        &mut self,
        channel: NativeAuditChannel,
        content_type: impl Into<String>,
        correlation_id: Uuid,
        payload: &[u8],
    ) -> Result<NativeAuditReference, NativeAuditError> {
        self.append_native_input(channel, content_type, correlation_id, payload)
    }

    pub fn append_native_output(
        &mut self,
        channel: NativeAuditChannel,
        content_type: impl Into<String>,
        correlation_id: Uuid,
        payload: &[u8],
    ) -> Result<NativeAuditReference, NativeAuditError> {
        self.append_bytes(
            NativeAuditDirection::Output,
            channel,
            content_type,
            correlation_id,
            payload,
            None,
        )
    }

    pub fn record_native_output(
        &mut self,
        channel: NativeAuditChannel,
        content_type: impl Into<String>,
        correlation_id: Uuid,
        payload: &[u8],
    ) -> Result<NativeAuditReference, NativeAuditError> {
        self.append_native_output(channel, content_type, correlation_id, payload)
    }

    pub fn close(mut self) -> Result<NativeAuditManifest, NativeAuditError> {
        self.close_with_status(if self.failed {
            NativeAuditIntegrityStatus::AuditFailed
        } else {
            NativeAuditIntegrityStatus::Complete
        })
    }

    pub fn close_with_status(
        &mut self,
        status: NativeAuditIntegrityStatus,
    ) -> Result<NativeAuditManifest, NativeAuditError> {
        if self.closed {
            return Ok(self.manifest.clone());
        }
        if let Err(error) = self.frames.sync_all() {
            self.failed = true;
            return Err(error.into());
        }
        // Once a durable append or close operation failed, the bundle must
        // remain visibly failed. Callers cannot turn a failed writer into a
        // replayable Complete bundle by supplying an optimistic status.
        self.manifest.integrity_status = if self.failed {
            NativeAuditIntegrityStatus::AuditFailed
        } else {
            status
        };
        self.manifest.final_checksum = Some(hex_digest(&self.stream_hasher));
        self.manifest.closed_at = Some(Utc::now());
        if let Err(error) = write_atomic_json(&self.manifest_path, &self.manifest) {
            self.failed = true;
            return Err(error);
        }
        self.closed = true;
        Ok(self.manifest.clone())
    }

    pub fn fail_closed(&mut self) -> Result<NativeAuditManifest, NativeAuditError> {
        self.failed = true;
        self.close_with_status(NativeAuditIntegrityStatus::AuditFailed)
    }

    pub fn finalize(self) -> Result<NativeAuditManifest, NativeAuditError> {
        self.close()
    }

    pub fn is_failed(&self) -> bool {
        self.failed
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderEvent {
    pub sequence: u64,
    pub timestamp: DateTime<Utc>,
    pub direction: NativeAuditDirection,
    pub channel: NativeAuditChannel,
    pub content_type: String,
    pub correlation_id: Uuid,
    pub payload: Vec<u8>,
    pub payload_json: Option<Value>,
    pub native_ref: NativeAuditReference,
}

pub type AgentEvent = AgentEventEnvelope;

pub trait NativeAuditReplayMapper {
    fn versions(&self) -> NativeAuditVersionSet;
    fn begin_replay(&self) {}
    fn validate_manifest(&self, manifest: &NativeAuditManifest) -> Result<(), NativeAuditError> {
        compare_versions(&self.versions(), &manifest.versions())
    }
    fn decode(&self, frame: &NativeAuditFrame) -> Result<ProviderEvent, NativeAuditError>;
    fn map(
        &self,
        event: &ProviderEvent,
        manifest: &NativeAuditManifest,
    ) -> Result<Vec<AgentEvent>, NativeAuditError>;
}

#[derive(Debug, Clone)]
pub struct DefaultNativeAuditMapper {
    versions: NativeAuditVersionSet,
}

impl DefaultNativeAuditMapper {
    pub fn new(versions: NativeAuditVersionSet) -> Self {
        Self { versions }
    }

    pub fn for_manifest(manifest: &NativeAuditManifest) -> Self {
        Self::new(manifest.versions())
    }
}

impl NativeAuditReplayMapper for DefaultNativeAuditMapper {
    fn versions(&self) -> NativeAuditVersionSet {
        self.versions.clone()
    }

    fn decode(&self, frame: &NativeAuditFrame) -> Result<ProviderEvent, NativeAuditError> {
        let payload = frame.payload_bytes()?;
        let payload_json = if frame.content_type.contains("json") {
            serde_json::from_slice(&payload).ok()
        } else {
            None
        };
        Ok(ProviderEvent {
            sequence: frame.sequence,
            timestamp: frame.timestamp,
            direction: frame.direction,
            channel: frame.channel.clone(),
            content_type: frame.content_type.clone(),
            correlation_id: frame.correlation_id,
            payload,
            payload_json,
            native_ref: NativeAuditReference {
                stream_id: Uuid::nil(),
                sequence: frame.sequence,
                checksum: Some(frame.payload_checksum.clone()),
            },
        })
    }

    fn map(
        &self,
        event: &ProviderEvent,
        manifest: &NativeAuditManifest,
    ) -> Result<Vec<AgentEvent>, NativeAuditError> {
        let payload = event.payload_json.clone().unwrap_or_else(|| {
            Value::String(base64::engine::general_purpose::STANDARD.encode(&event.payload))
        });
        let event_id = deterministic_uuid(manifest.run_attempt_id, event.sequence);
        Ok(vec![AgentEventEnvelope {
            schema_version: AGENT_EVENT_SCHEMA_VERSION,
            payload_version: AGENT_EVENT_PAYLOAD_VERSION,
            event_id,
            session_id: manifest.session_id,
            agent_run_id: manifest.agent_run_id,
            turn_id: manifest.turn_id,
            run_attempt_id: manifest.run_attempt_id,
            run_attempt_number: manifest.run_attempt_number,
            sequence: event.sequence,
            correlation_id: event.correlation_id,
            orchestration_run_id: None,
            orchestration_node_execution_id: None,
            timestamp: event.timestamp,
            native_refs: vec![NativeAuditReference {
                stream_id: manifest.run_attempt_id,
                sequence: event.sequence,
                checksum: Some(event.native_ref.checksum.clone().unwrap_or_default()),
            }],
            payload: AgentEventPayload::ProviderExtension {
                provider_namespace: manifest.provider_id.clone(),
                provider_event: event.channel.as_str().to_string(),
                payload,
            },
        }])
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuditReplayResult {
    pub provider_events: Vec<ProviderEvent>,
    pub agent_events: Vec<AgentEvent>,
    pub state: RunState,
}

pub struct AuditBundle {
    directory: PathBuf,
    manifest: NativeAuditManifest,
    frames: Vec<NativeAuditFrame>,
}

impl AuditBundle {
    pub fn read_from(directory: impl AsRef<Path>) -> Result<Self, NativeAuditError> {
        Self::read(directory)
    }

    pub fn export(
        source: impl AsRef<Path>,
        destination: impl AsRef<Path>,
    ) -> Result<(), NativeAuditError> {
        Self::read(source)?.export_to(destination)
    }

    pub fn read(directory: impl AsRef<Path>) -> Result<Self, NativeAuditError> {
        let directory = directory.as_ref().to_path_buf();
        let manifest_path = directory.join(NATIVE_AUDIT_MANIFEST_FILE);
        let frames_path = directory.join(NATIVE_AUDIT_FRAMES_FILE);
        let manifest: NativeAuditManifest = serde_json::from_slice(&fs::read(&manifest_path)?)
            .map_err(|_| NativeAuditError::PartialManifest)?;
        if !matches!(
            manifest.integrity_status,
            NativeAuditIntegrityStatus::Complete | NativeAuditIntegrityStatus::Recovered
        ) {
            return Err(NativeAuditError::PartialManifest);
        }
        if manifest.audit_schema_version != NATIVE_AUDIT_SCHEMA_VERSION {
            return Err(NativeAuditError::UnsupportedVersion {
                provided: manifest.audit_schema_version,
                current: NATIVE_AUDIT_SCHEMA_VERSION,
            });
        }
        let bytes = fs::read(&frames_path)?;
        if !bytes.is_empty() && !bytes.ends_with(b"\n") {
            return Err(NativeAuditError::TruncatedFrame);
        }
        let mut stream_hasher = Sha256::new();
        let mut frames = Vec::new();
        let mut sequence = 1;
        for raw_line in bytes.split_inclusive(|byte| *byte == b'\n') {
            if raw_line == b"\n" {
                continue;
            }
            stream_hasher.update(raw_line);
            let line = raw_line.strip_suffix(b"\n").unwrap_or(raw_line);
            let frame: NativeAuditFrame = serde_json::from_slice(line)
                .map_err(|_| NativeAuditError::MalformedFrame(sequence))?;
            frame.validate(sequence)?;
            sequence += 1;
            frames.push(frame);
        }
        if manifest.frame_count != frames.len() as u64
            || manifest.first_sequence != frames.first().map(|frame| frame.sequence)
            || manifest.last_sequence != frames.last().map(|frame| frame.sequence)
        {
            return Err(NativeAuditError::MalformedFrame(sequence));
        }
        let actual_checksum = hex_digest(&stream_hasher);
        if manifest.final_checksum.as_deref() != Some(actual_checksum.as_str()) {
            return Err(NativeAuditError::StreamChecksumMismatch {
                expected: manifest.final_checksum.unwrap_or_default(),
                actual: actual_checksum,
            });
        }
        Ok(Self {
            directory,
            manifest,
            frames,
        })
    }

    pub fn manifest(&self) -> &NativeAuditManifest {
        &self.manifest
    }

    pub fn frames(&self) -> &[NativeAuditFrame] {
        &self.frames
    }

    pub fn export_to(&self, destination: impl AsRef<Path>) -> Result<(), NativeAuditError> {
        let destination = destination.as_ref();
        fs::create_dir_all(destination)?;
        fs::copy(
            self.directory.join(NATIVE_AUDIT_MANIFEST_FILE),
            destination.join(NATIVE_AUDIT_MANIFEST_FILE),
        )?;
        fs::copy(
            self.directory.join(NATIVE_AUDIT_FRAMES_FILE),
            destination.join(NATIVE_AUDIT_FRAMES_FILE),
        )?;
        for name in [
            "expected-provider-events.jsonl",
            "expected-agent-events.jsonl",
        ] {
            let source = self.directory.join(name);
            if source.exists() {
                fs::copy(&source, destination.join(name))?;
            }
        }
        Ok(())
    }

    pub fn validate_versions(
        &self,
        expected: &NativeAuditVersionSet,
    ) -> Result<(), NativeAuditError> {
        let actual = self.manifest.versions();
        compare_versions(expected, &actual)
    }

    pub fn replay<M: NativeAuditReplayMapper>(
        &self,
        mapper: &M,
    ) -> Result<AuditReplayResult, NativeAuditError> {
        mapper.validate_manifest(&self.manifest)?;
        mapper.begin_replay();
        let mut provider_events = Vec::with_capacity(self.frames.len());
        let mut agent_events = Vec::new();
        for frame in &self.frames {
            let mut provider_event = mapper.decode(frame)?;
            provider_event.native_ref.stream_id = self.manifest.run_attempt_id;
            let mapped = mapper.map(&provider_event, &self.manifest)?;
            provider_events.push(provider_event);
            agent_events.extend(mapped);
        }
        let mut state = RunState {
            state_schema_version: RUN_STATE_SCHEMA_VERSION,
            reducer_version: RUN_STATE_REDUCER_VERSION,
            session_id: self.manifest.session_id,
            agent_run_id: self.manifest.agent_run_id,
            turn_id: self.manifest.turn_id,
            status: AgentRunStatus::Pending,
            projection_status: ProjectionStatus::Current,
            last_run_attempt_id: None,
            last_run_attempt_number: 0,
            last_event_sequence: 0,
            last_event_id: None,
            provider_session: None,
            terminal_output: None,
            last_error: None,
            observed_configuration: None,
            pending_interactions: Vec::new(),
            closed_interaction_ids: Vec::new(),
            unknown_event_count: 0,
            updated_at: self.manifest.created_at,
        };
        for event in &agent_events {
            super::reducer::reduce_agent_event(&mut state, event).map_err(|error| {
                NativeAuditError::MalformedFrame(error.to_string().len() as u64)
            })?;
        }
        Ok(AuditReplayResult {
            provider_events,
            agent_events,
            state,
        })
    }

    pub fn replay_default(&self) -> Result<AuditReplayResult, NativeAuditError> {
        self.replay(&DefaultNativeAuditMapper::for_manifest(&self.manifest))
    }

    pub fn replay_fixture<M: NativeAuditReplayMapper>(
        &self,
        mapper: &M,
    ) -> Result<AuditReplayResult, NativeAuditError> {
        let result = self.replay(mapper)?;
        if let Some(expected) =
            self.read_expected::<ProviderEvent>("expected-provider-events.jsonl")?
        {
            if expected != result.provider_events {
                return Err(NativeAuditError::FixtureMismatch {
                    kind: "provider_events",
                });
            }
        }
        if let Some(expected) = self.read_expected::<AgentEvent>("expected-agent-events.jsonl")? {
            if expected != result.agent_events {
                return Err(NativeAuditError::FixtureMismatch {
                    kind: "agent_events",
                });
            }
        }
        Ok(result)
    }

    fn read_expected<T: DeserializeOwned>(
        &self,
        name: &str,
    ) -> Result<Option<Vec<T>>, NativeAuditError> {
        let path = self.directory.join(name);
        if !path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(path)?;
        let mut values = Vec::new();
        for line in content.lines().filter(|line| !line.trim().is_empty()) {
            values.push(serde_json::from_str(line)?);
        }
        Ok(Some(values))
    }
}

struct RelativePaths {
    directory: PathBuf,
    manifest: String,
    frames: String,
}

fn stream_relative_paths(metadata: &NativeAuditMetadata) -> RelativePaths {
    let session = metadata.session_id.to_string();
    let directory = PathBuf::from(NATIVE_AUDIT_ROOT_RELATIVE)
        .join("sessions")
        .join(session.chars().take(2).collect::<String>())
        .join(session)
        .join("agent-runs")
        .join(metadata.agent_run_id.to_string())
        .join("attempts")
        .join(metadata.run_attempt_id.to_string());
    RelativePaths {
        manifest: directory
            .join(NATIVE_AUDIT_MANIFEST_FILE)
            .to_string_lossy()
            .replace('\\', "/"),
        frames: directory
            .join(NATIVE_AUDIT_FRAMES_FILE)
            .to_string_lossy()
            .replace('\\', "/"),
        directory,
    }
}

fn write_atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<(), NativeAuditError> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("manifest has no parent"))?;
    let temporary = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        Uuid::new_v4()
    ));
    let bytes = serde_json::to_vec_pretty(value)?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    drop(file);
    if let Err(error) = fs::rename(&temporary, path) {
        #[cfg(windows)]
        {
            if error.kind() == io::ErrorKind::AlreadyExists {
                fs::remove_file(path)?;
                fs::rename(&temporary, path)?;
            } else {
                return Err(error.into());
            }
        }
        #[cfg(not(windows))]
        {
            return Err(error.into());
        }
    }
    Ok(())
}

fn compare_versions(
    expected: &NativeAuditVersionSet,
    actual: &NativeAuditVersionSet,
) -> Result<(), NativeAuditError> {
    if expected.audit_schema_version != actual.audit_schema_version {
        return Err(NativeAuditError::MapperMismatch {
            field: "audit_schema_version",
            expected: expected.audit_schema_version.to_string(),
            actual: actual.audit_schema_version.to_string(),
        });
    }
    for (field, expected_value, actual_value) in [
        (
            "runtime_version",
            expected.runtime_version.clone().unwrap_or_default(),
            actual.runtime_version.clone().unwrap_or_default(),
        ),
        (
            "adapter_version",
            expected.adapter_version.clone(),
            actual.adapter_version.clone(),
        ),
        (
            "protocol_version",
            expected.protocol_version.clone().unwrap_or_default(),
            actual.protocol_version.clone().unwrap_or_default(),
        ),
        (
            "mapper_version",
            expected.mapper_version.clone(),
            actual.mapper_version.clone(),
        ),
    ] {
        if expected_value != actual_value {
            return Err(NativeAuditError::MapperMismatch {
                field,
                expected: expected_value,
                actual: actual_value,
            });
        }
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_digest(&hasher)
}

fn hex_digest(hasher: &Sha256) -> String {
    let digest = hasher.clone().finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn deterministic_uuid(run_attempt_id: Uuid, sequence: u64) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(run_attempt_id.as_bytes());
    hasher.update(sequence.to_be_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hasher.finalize()[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    fn metadata() -> NativeAuditMetadata {
        NativeAuditMetadata {
            session_id: Uuid::from_u128(1),
            agent_run_id: Uuid::from_u128(2),
            turn_id: Uuid::from_u128(3),
            run_attempt_id: Uuid::from_u128(4),
            run_attempt_number: 1,
            provider_id: "fixture".to_string(),
            runtime_profile_id: "fixture:default".to_string(),
            workspace_path: "C:/workspace".to_string(),
            runtime_version: Some("runtime-1".to_string()),
            protocol_version: Some("protocol-1".to_string()),
            adapter_version: "adapter-1".to_string(),
            mapper_version: "mapper-1".to_string(),
            created_at: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        }
    }

    #[test]
    fn writes_expected_layout_and_replays_deterministically() {
        let root = tempdir().unwrap();
        let metadata = metadata();
        let mut writer = NativeAuditWriter::create_in(root.path(), metadata.clone()).unwrap();
        writer
            .append_native_input(
                NativeAuditChannel::Stdin,
                "text/plain",
                Uuid::from_u128(5),
                b"hello",
            )
            .unwrap();
        writer
            .append_native_output(
                NativeAuditChannel::Stdout,
                "text/plain",
                Uuid::from_u128(5),
                b"world",
            )
            .unwrap();
        let manifest = writer.close().unwrap();
        let directory = root
            .path()
            .join(&manifest.manifest_relative_path)
            .parent()
            .unwrap()
            .to_path_buf();
        assert!(directory.join("manifest.json").exists());
        assert!(directory.join("frames.jsonl").exists());
        let bundle = AuditBundle::read(&directory).unwrap();
        let first = bundle.replay_default().unwrap();
        let second = bundle.replay_default().unwrap();
        assert_eq!(first.provider_events, second.provider_events);
        assert_eq!(first.agent_events, second.agent_events);
        assert_eq!(first.state, second.state);
    }

    #[test]
    fn rejects_truncated_frame_and_partial_manifest() {
        let root = tempdir().unwrap();
        let mut writer = NativeAuditWriter::create_in(root.path(), metadata()).unwrap();
        writer
            .append_native_output(
                NativeAuditChannel::Stdout,
                "text/plain",
                Uuid::from_u128(5),
                b"x",
            )
            .unwrap();
        let manifest = writer.close().unwrap();
        let directory = root
            .path()
            .join(&manifest.manifest_relative_path)
            .parent()
            .unwrap()
            .to_path_buf();
        let frames_path = directory.join("frames.jsonl");
        let mut bytes = fs::read(&frames_path).unwrap();
        bytes.pop();
        fs::write(&frames_path, bytes).unwrap();
        assert!(matches!(
            AuditBundle::read(&directory),
            Err(NativeAuditError::TruncatedFrame)
        ));
    }

    #[test]
    fn mapper_version_mismatch_is_rejected() {
        let root = tempdir().unwrap();
        let mut writer = NativeAuditWriter::create_in(root.path(), metadata()).unwrap();
        writer
            .append_native_output(
                NativeAuditChannel::Stdout,
                "text/plain",
                Uuid::from_u128(5),
                b"x",
            )
            .unwrap();
        let manifest = writer.close().unwrap();
        let directory = root
            .path()
            .join(&manifest.manifest_relative_path)
            .parent()
            .unwrap()
            .to_path_buf();
        let bundle = AuditBundle::read(&directory).unwrap();
        let mut versions = manifest.versions();
        versions.mapper_version = "mapper-2".to_string();
        assert!(matches!(
            bundle.replay(&DefaultNativeAuditMapper::new(versions)),
            Err(NativeAuditError::MapperMismatch {
                field: "mapper_version",
                ..
            })
        ));
    }

    #[test]
    fn failed_writer_cannot_be_closed_as_complete() {
        let root = tempdir().unwrap();
        let mut writer = NativeAuditWriter::create_in(root.path(), metadata()).unwrap();
        let invalid_sequence = NativeAuditFrame::from_bytes(
            2,
            Utc::now(),
            NativeAuditDirection::Output,
            NativeAuditChannel::Stdout,
            "text/plain",
            Uuid::from_u128(5),
            b"out-of-order",
            None,
        );
        assert!(matches!(
            writer.append(invalid_sequence),
            Err(NativeAuditError::SequenceMismatch { .. })
        ));

        let manifest = writer
            .close_with_status(NativeAuditIntegrityStatus::Complete)
            .unwrap();
        assert_eq!(
            manifest.integrity_status,
            NativeAuditIntegrityStatus::AuditFailed
        );
    }
}
