-- P0 file-driven workflow protocol. Control files carry task bodies; these
-- tables carry identity, acceptance, and idempotent main-Agent handover state.
CREATE TABLE workflow_split_batches (
    id BLOB PRIMARY KEY,
    run_id BLOB NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
    node_execution_id BLOB NOT NULL REFERENCES node_executions(id) ON DELETE CASCADE,
    manifest_path TEXT NOT NULL,
    manifest_sha256 TEXT NOT NULL,
    accepted_at TEXT,
    dispatch_path TEXT,
    dispatch_sha256 TEXT,
    source_batch_id BLOB REFERENCES workflow_split_batches(id) ON DELETE RESTRICT,
    summary_node_execution_id BLOB REFERENCES node_executions(id) ON DELETE SET NULL,
    error_text TEXT,
    state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','accepted','dispatching','running','partial','succeeded','failed','cancelled')),
    created_at TEXT NOT NULL DEFAULT (datetime('now','subsec')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now','subsec')),
    UNIQUE(run_id,node_execution_id)
);

CREATE TABLE workflow_split_subtasks (
    id BLOB PRIMARY KEY,
    batch_id BLOB NOT NULL REFERENCES workflow_split_batches(id) ON DELETE CASCADE,
    subtask_id TEXT NOT NULL,
    title TEXT NOT NULL,
    instruction_path TEXT NOT NULL,
    instruction_sha256 TEXT NOT NULL,
    depends_on_json TEXT NOT NULL DEFAULT '[]',
    stage_index INTEGER NOT NULL DEFAULT 0,
    node_id TEXT,
    node_execution_id BLOB REFERENCES node_executions(id) ON DELETE SET NULL,
    session_id BLOB REFERENCES sessions(id) ON DELETE SET NULL,
    source_subtask_id BLOB REFERENCES workflow_split_subtasks(id) ON DELETE RESTRICT,
    error_text TEXT,
    state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','queued','running','succeeded','failed','cancelled','blocked')),
    created_at TEXT NOT NULL DEFAULT (datetime('now','subsec')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now','subsec')),
    UNIQUE(batch_id,subtask_id)
);
CREATE INDEX idx_workflow_split_subtasks_batch ON workflow_split_subtasks(batch_id,stage_index,state);

CREATE TABLE workflow_handover_events (
    id BLOB PRIMARY KEY,
    batch_id BLOB NOT NULL UNIQUE REFERENCES workflow_split_batches(id) ON DELETE CASCADE,
    run_id BLOB NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
    -- Preserve the original identity after Session deletion; recovery reports
    -- the missing destination instead of silently dropping the pending work.
    session_id BLOB NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','claimed','completed','failed','cancelled')),
    agent_run_id BLOB,
    run_attempt_id BLOB,
    turn_id BLOB,
    error_text TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now','subsec')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now','subsec'))
);
CREATE INDEX idx_workflow_handover_pending ON workflow_handover_events(state,created_at);

CREATE TABLE workflow_node_handoffs (
    node_execution_id BLOB PRIMARY KEY REFERENCES node_executions(id) ON DELETE CASCADE,
    run_id BLOB NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
    handoff_path TEXT NOT NULL,
    handoff_sha256 TEXT NOT NULL,
    files_json TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now','subsec'))
);
CREATE INDEX idx_workflow_node_handoffs_run ON workflow_node_handoffs(run_id);

CREATE TABLE workflow_run_deliveries (
    run_id BLOB PRIMARY KEY REFERENCES workflow_runs(id) ON DELETE CASCADE,
    completeness TEXT NOT NULL CHECK(completeness IN ('full','partial')),
    source_run_id BLOB REFERENCES workflow_runs(id) ON DELETE RESTRICT,
    source_batch_id BLOB REFERENCES workflow_split_batches(id) ON DELETE RESTRICT,
    missing_subtasks_json TEXT NOT NULL DEFAULT '[]',
    summary_node_id TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now','subsec'))
);

ALTER TABLE workflow_run_submissions ADD COLUMN source_batch_id BLOB
    REFERENCES workflow_split_batches(id) ON DELETE RESTRICT;
