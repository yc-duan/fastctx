-- Sequence and revision columns are unsigned big-endian eight-byte values.
-- Their lexicographic order agrees with the protocol's unsigned order.
CREATE TABLE sync_accounts (
    hub TEXT NOT NULL, account TEXT NOT NULL, legacy_name TEXT NOT NULL,
    opened TEXT NOT NULL, PRIMARY KEY(hub,account), UNIQUE(hub,legacy_name,opened)
) WITHOUT ROWID;
CREATE TABLE sync_scopes (
    hub TEXT NOT NULL, scope TEXT NOT NULL, policy BLOB NOT NULL,
    acl_revision BLOB NOT NULL CHECK(length(acl_revision)=8),
    enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),
    publication_epoch BLOB NOT NULL CHECK(length(publication_epoch)=8),
    PRIMARY KEY(hub,scope)
) WITHOUT ROWID;
CREATE TABLE sync_objects (
    hub TEXT NOT NULL, scope TEXT NOT NULL, object TEXT NOT NULL,
    kind TEXT NOT NULL, schema INTEGER NOT NULL, authority BLOB NOT NULL,
    revision BLOB NOT NULL CHECK(length(revision)=8),
    content TEXT, history TEXT, state TEXT NOT NULL,
    completeness BLOB NOT NULL, PRIMARY KEY(hub,scope,object),
    FOREIGN KEY(hub,scope) REFERENCES sync_scopes(hub,scope)
) WITHOUT ROWID;
CREATE TABLE sync_object_versions (
    hub TEXT NOT NULL, scope TEXT NOT NULL, object TEXT NOT NULL,
    revision BLOB NOT NULL CHECK(length(revision)=8),
    operation TEXT NOT NULL, content TEXT, parents BLOB NOT NULL,
    state TEXT NOT NULL, PRIMARY KEY(hub,scope,object,revision)
) WITHOUT ROWID;
CREATE TABLE sync_scope_heads (
    hub TEXT NOT NULL, scope TEXT NOT NULL, epoch TEXT NOT NULL,
    sequence BLOB NOT NULL CHECK(length(sequence)=8), snapshot TEXT,
    PRIMARY KEY(hub,scope)
) WITHOUT ROWID;
CREATE TABLE sync_changes (
    hub TEXT NOT NULL, scope TEXT NOT NULL, epoch TEXT NOT NULL,
    sequence BLOB NOT NULL CHECK(length(sequence)=8),
    object TEXT NOT NULL, revision BLOB NOT NULL CHECK(length(revision)=8),
    operation TEXT NOT NULL, PRIMARY KEY(hub,scope,epoch,sequence)
) WITHOUT ROWID;
CREATE TABLE sync_local_head (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    sequence BLOB NOT NULL CHECK(length(sequence)=8)
);
CREATE TABLE sync_local_changes (
    sequence BLOB PRIMARY KEY CHECK(length(sequence)=8),
    hub TEXT NOT NULL, scope TEXT NOT NULL, object TEXT NOT NULL,
    revision BLOB NOT NULL CHECK(length(revision)=8),
    origin BLOB NOT NULL, operation TEXT NOT NULL
) WITHOUT ROWID;
CREATE INDEX sync_local_changes_scope ON sync_local_changes(hub,scope,sequence);
CREATE TABLE sync_stream_heads (
    hub TEXT NOT NULL, scope TEXT NOT NULL, stream TEXT NOT NULL,
    producer TEXT NOT NULL, epoch TEXT NOT NULL,
    sequence BLOB NOT NULL CHECK(length(sequence)=8), predecessor BLOB,
    PRIMARY KEY(hub,scope,stream,epoch)
) WITHOUT ROWID;
CREATE TABLE sync_stream_records (
    hub TEXT NOT NULL, scope TEXT NOT NULL, stream TEXT NOT NULL, epoch TEXT NOT NULL,
    sequence BLOB NOT NULL CHECK(length(sequence)=8), operation TEXT NOT NULL,
    kind TEXT NOT NULL, schema INTEGER NOT NULL, payload TEXT NOT NULL,
    causes BLOB NOT NULL, PRIMARY KEY(hub,scope,stream,epoch,sequence),
    UNIQUE(hub,scope,stream,epoch,operation)
) WITHOUT ROWID;
CREATE TABLE sync_operations (
    hub TEXT NOT NULL, scope TEXT NOT NULL, actor TEXT NOT NULL,
    operation TEXT NOT NULL, request_hash TEXT NOT NULL, result BLOB NOT NULL,
    PRIMARY KEY(hub,scope,actor,operation)
) WITHOUT ROWID;
CREATE TABLE sync_outbox (
    destination TEXT NOT NULL, hub TEXT NOT NULL, scope TEXT NOT NULL,
    operation TEXT NOT NULL, body BLOB NOT NULL,
    priority INTEGER NOT NULL CHECK(priority BETWEEN 0 AND 3), next_due INTEGER NOT NULL,
    PRIMARY KEY(destination,hub,scope,operation)
) WITHOUT ROWID;
CREATE INDEX sync_outbox_due ON sync_outbox(next_due,priority);
CREATE TABLE sync_peer_cursors (
    peer TEXT NOT NULL, hub TEXT NOT NULL, scope TEXT NOT NULL, cursor BLOB NOT NULL,
    PRIMARY KEY(peer,hub,scope)
) WITHOUT ROWID;
CREATE TABLE sync_consumer_cursors (
    consumer TEXT PRIMARY KEY, sequence BLOB NOT NULL CHECK(length(sequence)=8)
) WITHOUT ROWID;
CREATE TABLE sync_tasks (
    kind TEXT NOT NULL, hub TEXT NOT NULL, scope TEXT NOT NULL, task_key TEXT NOT NULL,
    wanted BLOB NOT NULL CHECK(length(wanted)=8), completed BLOB NOT NULL CHECK(length(completed)=8),
    state TEXT NOT NULL, priority INTEGER NOT NULL CHECK(priority BETWEEN 0 AND 3),
    next_due INTEGER NOT NULL, lease_generation BLOB NOT NULL CHECK(length(lease_generation)=8),
    lease_owner TEXT, attempts INTEGER NOT NULL, dependency TEXT, reason TEXT,
    PRIMARY KEY(kind,hub,scope,task_key)
) WITHOUT ROWID;
CREATE INDEX sync_tasks_due ON sync_tasks(state,next_due,priority);
CREATE INDEX sync_tasks_dependency ON sync_tasks(dependency,state,next_due);
CREATE TABLE sync_sources (
    hub TEXT NOT NULL, scope TEXT NOT NULL, source TEXT NOT NULL,
    generation TEXT NOT NULL, revision BLOB NOT NULL CHECK(length(revision)=8),
    committed_offset BLOB NOT NULL CHECK(length(committed_offset)=8),
    parser_version INTEGER NOT NULL, state TEXT, PRIMARY KEY(hub,scope,source)
) WITHOUT ROWID;
CREATE TABLE sync_runtime (
    hub TEXT NOT NULL, scope TEXT NOT NULL, kind TEXT NOT NULL, record_key TEXT NOT NULL,
    revision BLOB NOT NULL CHECK(length(revision)=8), body BLOB NOT NULL,
    PRIMARY KEY(hub,scope,kind,record_key)
) WITHOUT ROWID;
CREATE TABLE sync_projections (
    hub TEXT NOT NULL, scope TEXT NOT NULL, kind TEXT NOT NULL, object TEXT NOT NULL,
    source BLOB NOT NULL CHECK(length(source)=8), body BLOB NOT NULL,
    PRIMARY KEY(hub,scope,kind,object)
) WITHOUT ROWID;
CREATE TABLE sync_tombstones (
    hub TEXT NOT NULL, scope TEXT NOT NULL, object TEXT NOT NULL,
    revision BLOB NOT NULL CHECK(length(revision)=8), operation TEXT NOT NULL,
    PRIMARY KEY(hub,scope,object)
) WITHOUT ROWID;
CREATE TABLE sync_asset_blocks (
    hub TEXT NOT NULL, scope TEXT NOT NULL, hash TEXT NOT NULL,
    envelope BLOB NOT NULL, encoded_hash TEXT NOT NULL,
    decoded_bytes INTEGER NOT NULL CHECK(decoded_bytes BETWEEN 0 AND 1048576),
    encoded_bytes INTEGER NOT NULL CHECK(encoded_bytes BETWEEN 0 AND 1114112),
    storage_key TEXT NOT NULL, verified INTEGER NOT NULL CHECK(verified IN (0,1)),
    PRIMARY KEY(hub,scope,hash)
) WITHOUT ROWID;
CREATE TABLE sync_asset_manifests (
    hub TEXT NOT NULL, scope TEXT NOT NULL, hash TEXT NOT NULL,
    page BLOB NOT NULL, complete INTEGER NOT NULL CHECK(complete IN (0,1)),
    PRIMARY KEY(hub,scope,hash)
) WITHOUT ROWID;
CREATE TABLE sync_asset_edges (
    hub TEXT NOT NULL, scope TEXT NOT NULL, parent TEXT NOT NULL, child TEXT NOT NULL,
    PRIMARY KEY(hub,scope,parent,child)
) WITHOUT ROWID;
CREATE TABLE sync_asset_roots (
    hub TEXT NOT NULL, scope TEXT NOT NULL, object TEXT NOT NULL,
    revision BLOB NOT NULL CHECK(length(revision)=8), hash TEXT NOT NULL,
    PRIMARY KEY(hub,scope,object,revision,hash)
) WITHOUT ROWID;
CREATE INDEX sync_asset_roots_hash ON sync_asset_roots(hub,scope,hash);
CREATE TABLE sync_asset_pins (
    hub TEXT NOT NULL, scope TEXT NOT NULL, operation TEXT NOT NULL, hash TEXT NOT NULL,
    generation BLOB NOT NULL CHECK(length(generation)=8), phase TEXT NOT NULL,
    PRIMARY KEY(hub,scope,operation,hash)
) WITHOUT ROWID;
CREATE INDEX sync_asset_pins_hash ON sync_asset_pins(hub,scope,hash);
CREATE TABLE sync_asset_transfers (
    hub TEXT NOT NULL, scope TEXT NOT NULL, hash TEXT NOT NULL,
    progress BLOB NOT NULL, source_hint TEXT, phase TEXT NOT NULL,
    PRIMARY KEY(hub,scope,hash)
) WITHOUT ROWID;
CREATE TABLE sync_asset_gc (
    hub TEXT NOT NULL, scope TEXT NOT NULL, hash TEXT NOT NULL,
    generation BLOB NOT NULL CHECK(length(generation)=8), next_due INTEGER NOT NULL,
    PRIMARY KEY(hub,scope,hash)
) WITHOUT ROWID;
CREATE INDEX sync_asset_gc_due ON sync_asset_gc(next_due);
