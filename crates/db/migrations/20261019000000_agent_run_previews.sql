-- A rebuildable, bounded projection of canonical events. Agent execution never
-- depends on this table or on the availability of its background projector.
CREATE TABLE agent_run_previews (
    agent_run_id             BLOB PRIMARY KEY NOT NULL,
    algorithm_version        INTEGER NOT NULL CHECK (algorithm_version > 0),
    last_run_attempt_id      BLOB,
    last_run_attempt_number  INTEGER NOT NULL DEFAULT 0,
    last_sequence            INTEGER NOT NULL DEFAULT 0,
    last_event_id            BLOB,
    preview_json             TEXT NOT NULL CHECK (json_valid(preview_json)),
    updated_at               TEXT NOT NULL,
    FOREIGN KEY (agent_run_id) REFERENCES agent_runs(id) ON DELETE CASCADE,
    CHECK (
        (last_run_attempt_number = 0 AND last_sequence = 0
            AND last_run_attempt_id IS NULL AND last_event_id IS NULL)
        OR
        (last_run_attempt_number > 0 AND last_sequence > 0
            AND last_run_attempt_id IS NOT NULL AND last_event_id IS NOT NULL)
    )
);
