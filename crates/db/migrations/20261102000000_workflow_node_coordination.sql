-- Reuse the existing durable main-session handover queue for node exceptions.
-- A failed node has one event; repeated observations cannot create new turns.
CREATE TABLE workflow_handover_events_next (
    id BLOB PRIMARY KEY,
    batch_id BLOB UNIQUE REFERENCES workflow_split_batches(id) ON DELETE CASCADE,
    source_node_execution_id BLOB UNIQUE REFERENCES node_executions(id) ON DELETE CASCADE,
    failure_kind TEXT,
    evidence_path TEXT,
    evidence_sha256 TEXT,
    run_id BLOB NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
    session_id BLOB NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','claimed','completed','failed','cancelled')),
    agent_run_id BLOB,
    run_attempt_id BLOB,
    turn_id BLOB,
    error_text TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now','subsec')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now','subsec')),
    CHECK ((batch_id IS NOT NULL AND source_node_execution_id IS NULL AND failure_kind IS NULL)
        OR (batch_id IS NULL AND source_node_execution_id IS NOT NULL
            AND failure_kind IS NOT NULL AND failure_kind IN ('execution_failed','delivery_invalid'))),
    CHECK ((evidence_path IS NULL) = (evidence_sha256 IS NULL))
);

INSERT INTO workflow_handover_events_next
    (id,batch_id,run_id,session_id,state,agent_run_id,run_attempt_id,turn_id,error_text,created_at,updated_at)
SELECT id,batch_id,run_id,session_id,state,agent_run_id,run_attempt_id,turn_id,error_text,created_at,updated_at
FROM workflow_handover_events;

DROP TABLE workflow_handover_events;
ALTER TABLE workflow_handover_events_next RENAME TO workflow_handover_events;
CREATE INDEX idx_workflow_handover_pending ON workflow_handover_events(state,created_at);
