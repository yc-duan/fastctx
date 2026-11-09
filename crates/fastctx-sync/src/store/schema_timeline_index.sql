-- Ordinary index patches use zero; a rebuild stamps its target projection.
ALTER TABLE sync_timeline_items ADD COLUMN index_generation BLOB NOT NULL
    DEFAULT X'0000000000000000' CHECK(length(index_generation)=8);
CREATE INDEX sync_timeline_generation ON sync_timeline_items(
    hub,scope,conversation,active,index_generation,item,revision
);
CREATE TABLE sync_timeline_rebuilds (
    hub TEXT NOT NULL, scope TEXT NOT NULL, conversation TEXT NOT NULL,
    projection BLOB NOT NULL CHECK(length(projection)=8), root TEXT NOT NULL,
    phase INTEGER NOT NULL CHECK(phase IN (0,1)),
    after_position BLOB CHECK(after_position IS NULL OR length(after_position)<=262144),
    PRIMARY KEY(hub,scope,conversation)
) WITHOUT ROWID;
-- Existing incomplete heads must not depend on another installation to wake.
INSERT INTO sync_tasks(
    kind,hub,scope,task_key,wanted,completed,state,priority,next_due,lease_generation,
    attempts,dependency_revision,blocked_attempts,dependency_sequence
)
SELECT 'sync.projection',hub,scope,'derived.timeline-index/'||conversation,
    local,X'0000000000000000','ready',2,0,X'0000000000000000',0,local,0,X'0000000000000000'
FROM sync_timeline_heads WHERE root IS NOT NULL AND (indexed IS NULL OR indexed<>projection);
