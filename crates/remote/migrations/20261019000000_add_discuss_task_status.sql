-- Discuss is a regular task status, not a synthetic list of sessions.
-- This one-time upgrade preserves existing task/status identities and user edits.
INSERT INTO project_statuses (id, project_id, name, color, sort_order, hidden, created_at)
SELECT gen_random_uuid(), p.id, 'Discuss', '215 12% 55%',
       GREATEST(COALESCE(MIN(s.sort_order)::bigint, 0) - 1, -2147483648)::integer,
       false, NOW()
FROM projects p
LEFT JOIN project_statuses s ON s.project_id = p.id
WHERE NOT EXISTS (
    SELECT 1 FROM project_statuses existing
    WHERE existing.project_id = p.id AND LOWER(TRIM(existing.name)) = 'discuss'
)
GROUP BY p.id;
