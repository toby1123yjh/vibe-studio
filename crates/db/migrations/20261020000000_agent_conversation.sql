-- A rebuildable view of canonical agent_events, never a second event source.
CREATE TABLE agent_conversation_sessions (
    session_id BLOB PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
    epoch BLOB NOT NULL,
    revision INTEGER NOT NULL DEFAULT 0 CHECK (revision >= 0)
);

CREATE TABLE agent_conversation_checkpoints (
    agent_run_id BLOB PRIMARY KEY,
    session_id BLOB NOT NULL,
    run_attempt_number INTEGER NOT NULL CHECK (run_attempt_number > 0),
    sequence INTEGER NOT NULL CHECK (sequence > 0),
    FOREIGN KEY (agent_run_id, session_id)
        REFERENCES agent_runs(id, session_id) ON DELETE CASCADE
);

CREATE TABLE agent_conversation_blocks (
    session_id BLOB NOT NULL,
    block_id TEXT NOT NULL,
    agent_run_id BLOB NOT NULL,
    run_attempt_id BLOB NOT NULL,
    kind TEXT NOT NULL,
    role TEXT,
    native_id TEXT,
    parent_block_id TEXT,
    position TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    fields_json TEXT NOT NULL,
    body_lengths_json TEXT NOT NULL DEFAULT '{}',
    finished INTEGER NOT NULL DEFAULT 0 CHECK (finished IN (0, 1)),
    finish_reason TEXT,
    tombstone INTEGER NOT NULL DEFAULT 0 CHECK (tombstone IN (0, 1)),
    source_event_id BLOB NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (session_id, block_id),
    FOREIGN KEY (session_id) REFERENCES agent_conversation_sessions(session_id) ON DELETE CASCADE,
    FOREIGN KEY (agent_run_id, session_id) REFERENCES agent_runs(id, session_id) ON DELETE CASCADE,
    FOREIGN KEY (run_attempt_id, agent_run_id) REFERENCES agent_run_attempts(id, agent_run_id) ON DELETE CASCADE,
    FOREIGN KEY (source_event_id, agent_run_id) REFERENCES agent_events(event_id, agent_run_id) ON DELETE CASCADE,
    UNIQUE (session_id, position)
);

CREATE INDEX idx_agent_conversation_blocks_revision
    ON agent_conversation_blocks(session_id, revision);

CREATE TABLE agent_conversation_body_fragments (
    session_id BLOB NOT NULL,
    block_id TEXT NOT NULL,
    field TEXT NOT NULL,
    byte_offset INTEGER NOT NULL CHECK (byte_offset >= 0),
    content TEXT NOT NULL,
    byte_length INTEGER NOT NULL CHECK (byte_length >= 0),
    PRIMARY KEY (session_id, block_id, field, byte_offset),
    FOREIGN KEY (session_id, block_id)
        REFERENCES agent_conversation_blocks(session_id, block_id) ON DELETE CASCADE
);

-- Native answers are not safely repeatable after an uncertain pipe write.
CREATE TABLE agent_conversation_interaction_replies (
    reply_id BLOB PRIMARY KEY,
    session_id BLOB NOT NULL,
    agent_run_id BLOB NOT NULL,
    run_attempt_id BLOB NOT NULL,
    request_id TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    delivery_status TEXT NOT NULL CHECK (delivery_status IN (
        'pending', 'delivering', 'delivered', 'resolved', 'delivery_unknown', 'failed', 'cancelled'
    )),
    last_error TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (run_attempt_id, request_id),
    FOREIGN KEY (agent_run_id, session_id) REFERENCES agent_runs(id, session_id) ON DELETE CASCADE,
    FOREIGN KEY (run_attempt_id, agent_run_id) REFERENCES agent_run_attempts(id, agent_run_id) ON DELETE CASCADE
);
