-- Sequence and revision columns are unsigned big-endian eight-byte values.
-- Their lexicographic order agrees with the protocol's unsigned order.
-- Authority epochs are random 16-byte identities and have no temporal order.
-- The owning database is the data root; these records have no account scope.
CREATE TABLE sync_format_upgrades (
    service TEXT NOT NULL CHECK(service IN ('world','hub')),
    format INTEGER NOT NULL CHECK(format>0),
    attempt INTEGER NOT NULL CHECK(attempt IN (0,1)),
    written_by TEXT NOT NULL CHECK(length(written_by) BETWEEN 1 AND 128),
    previous_format INTEGER CHECK(previous_format IS NULL OR (previous_format>=1 AND previous_format<format)),
    backup_basis TEXT NOT NULL CHECK(backup_basis IN ('new_database','existing_database')),
    database_bytes BLOB NOT NULL CHECK(length(database_bytes)=8),
    wal_bytes BLOB NOT NULL CHECK(length(wal_bytes)=8),
    upgraded_at INTEGER NOT NULL CHECK(upgraded_at>=0),
    revision BLOB NOT NULL CHECK(length(revision)=8),
    phase TEXT NOT NULL CHECK(phase IN ('prepared','copying','copied','complete','abandoned','not_required')),
    snapshot_at INTEGER CHECK(snapshot_at IS NULL OR snapshot_at>=0),
    file_name TEXT CHECK(file_name IS NULL OR length(file_name) BETWEEN 1 AND 192),
    copied_pages INTEGER CHECK(copied_pages IS NULL OR copied_pages>0),
    reason TEXT CHECK(reason IS NULL OR length(reason) BETWEEN 1 AND 4096),
    end_reason TEXT CHECK(end_reason IS NULL OR end_reason IN ('interrupted','space','deadline','wal_limit','snapshot','copy','integrity','publication')),
    CHECK((reason IS NULL) = (end_reason IS NULL)),
    CHECK(backup_basis!='new_database' OR (previous_format IS NULL AND database_bytes=X'0000000000000000' AND wal_bytes=X'0000000000000000')),
    PRIMARY KEY(service,format,attempt)
) WITHOUT ROWID;
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
    kind TEXT NOT NULL, schema INTEGER NOT NULL, authority BLOB NOT NULL, evidence BLOB NOT NULL,
    authority_epoch BLOB NOT NULL CHECK(length(authority_epoch)=16),
    revision BLOB NOT NULL CHECK(length(revision)=8),
    current_authority_epoch BLOB NOT NULL CHECK(length(current_authority_epoch)=16),
    current_version BLOB NOT NULL CHECK(length(current_version)=8),
    content BLOB, history TEXT, state TEXT NOT NULL,
    completeness BLOB NOT NULL, PRIMARY KEY(hub,scope,object),
    FOREIGN KEY(hub,scope) REFERENCES sync_scopes(hub,scope)
) WITHOUT ROWID;
CREATE INDEX sync_objects_kind ON sync_objects(hub,scope,kind,object);
CREATE TABLE sync_object_versions (
    hub TEXT NOT NULL, scope TEXT NOT NULL, object TEXT NOT NULL,
    authority_epoch BLOB NOT NULL CHECK(length(authority_epoch)=16),
    revision BLOB NOT NULL CHECK(length(revision)=8),
    actor TEXT NOT NULL, operation TEXT NOT NULL, content BLOB, parents BLOB NOT NULL,
    state TEXT NOT NULL, PRIMARY KEY(hub,scope,object,authority_epoch,revision)
) WITHOUT ROWID;
CREATE TABLE sync_scope_heads (
    hub TEXT NOT NULL, scope TEXT NOT NULL, epoch TEXT NOT NULL,
    sequence BLOB NOT NULL CHECK(length(sequence)=8), snapshot TEXT,
    prefix_digest TEXT NOT NULL,
    PRIMARY KEY(hub,scope)
) WITHOUT ROWID;
CREATE TABLE sync_changes (
    hub TEXT NOT NULL, scope TEXT NOT NULL, epoch TEXT NOT NULL,
    sequence BLOB NOT NULL CHECK(length(sequence)=8),
    object TEXT NOT NULL, authority_epoch BLOB NOT NULL CHECK(length(authority_epoch)=16),
    revision BLOB NOT NULL CHECK(length(revision)=8),
    operation TEXT NOT NULL, canonical_change BLOB NOT NULL, prefix_digest TEXT NOT NULL,
    PRIMARY KEY(hub,scope,epoch,sequence)
) WITHOUT ROWID;
CREATE TABLE sync_local_head (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    sequence BLOB NOT NULL CHECK(length(sequence)=8),
    scheduler_slot INTEGER NOT NULL DEFAULT 0 CHECK(scheduler_slot BETWEEN 0 AND 7),
    dependency_sequence BLOB NOT NULL DEFAULT X'0000000000000000' CHECK(length(dependency_sequence)=8)
);
CREATE TABLE sync_local_changes (
    sequence BLOB PRIMARY KEY CHECK(length(sequence)=8),
    hub TEXT NOT NULL, scope TEXT NOT NULL, object TEXT NOT NULL, kind TEXT NOT NULL,
    authority_epoch BLOB CHECK(authority_epoch IS NULL OR length(authority_epoch)=16),
    revision BLOB CHECK(revision IS NULL OR length(revision)=8),
    origin BLOB NOT NULL, operation TEXT NOT NULL,
    CHECK((authority_epoch IS NULL) = (revision IS NULL))
) WITHOUT ROWID;
CREATE INDEX sync_local_changes_scope ON sync_local_changes(hub,scope,sequence);
-- A producer's first epoch on this machine commits before the hub confirms it:
-- its epoch proof and prefix digests stay empty and its facts stay local until
-- confirmation builds the prefix over the stored records.
-- `object` is the destination object the stream belongs to; `local` is the local
-- sequence of the commit that last advanced it, which installers catch up with.
CREATE TABLE sync_stream_heads (
    hub TEXT NOT NULL, scope TEXT NOT NULL, stream TEXT NOT NULL, object TEXT NOT NULL,
    producer BLOB NOT NULL, epoch TEXT NOT NULL, kind TEXT NOT NULL, schema INTEGER NOT NULL,
    sequence BLOB NOT NULL CHECK(length(sequence)=8), local BLOB NOT NULL CHECK(length(local)=8),
    epoch_proof TEXT, prefix_digest TEXT,
    CHECK(prefix_digest IS NULL OR epoch_proof IS NOT NULL),
    PRIMARY KEY(hub,scope,stream,epoch)
) WITHOUT ROWID;
CREATE INDEX sync_stream_heads_object ON sync_stream_heads(hub,scope,object,kind);
CREATE TABLE sync_stream_records (
    hub TEXT NOT NULL, scope TEXT NOT NULL, stream TEXT NOT NULL, epoch TEXT NOT NULL,
    sequence BLOB NOT NULL CHECK(length(sequence)=8), operation TEXT NOT NULL,
    kind TEXT NOT NULL, schema INTEGER NOT NULL, payload BLOB NOT NULL,
    causes BLOB NOT NULL, batch_proof TEXT NOT NULL, prefix_digest TEXT,
    PRIMARY KEY(hub,scope,stream,epoch,sequence),
    UNIQUE(hub,scope,stream,epoch,operation)
) WITHOUT ROWID;
-- A record retains its payload block and the roots it declares, forever: facts
-- are assets. Revocation and deletion remove these rows through their own cleanup.
CREATE TABLE sync_stream_roots (
    hub TEXT NOT NULL, scope TEXT NOT NULL, stream TEXT NOT NULL, epoch TEXT NOT NULL,
    sequence BLOB NOT NULL CHECK(length(sequence)=8), hash TEXT NOT NULL,
    root_kind TEXT NOT NULL CHECK(root_kind IN ('block','manifest')), decoded_bytes INTEGER,
    CHECK((root_kind='block' AND decoded_bytes IS NOT NULL AND decoded_bytes BETWEEN 0 AND 1048576)
       OR (root_kind='manifest' AND decoded_bytes IS NULL)),
    PRIMARY KEY(hub,scope,stream,epoch,sequence,hash,root_kind),
    FOREIGN KEY(hub,scope,stream,epoch,sequence) REFERENCES sync_stream_records(hub,scope,stream,epoch,sequence)
) WITHOUT ROWID;
CREATE INDEX sync_stream_roots_hash ON sync_stream_roots(hub,scope,hash,root_kind);
CREATE TABLE sync_operations (
    hub TEXT NOT NULL, actor TEXT NOT NULL, operation TEXT NOT NULL,
    request_hash TEXT NOT NULL, result BLOB NOT NULL, accepted_proofs BLOB,
    expected_fragments INTEGER CHECK(expected_fragments BETWEEN 1 AND 256),
    received_fragments BLOB, result_digest TEXT,
    PRIMARY KEY(hub,actor,operation)
) WITHOUT ROWID;
CREATE TABLE sync_pending_mutations (
    hub TEXT NOT NULL, origin TEXT NOT NULL, operation TEXT NOT NULL, actor TEXT NOT NULL,
    local_sequence BLOB NOT NULL CHECK(length(local_sequence)=8),
    request_hash TEXT NOT NULL, original_request BLOB NOT NULL,
    mode TEXT NOT NULL, state TEXT NOT NULL, preserve_offline INTEGER NOT NULL,
    PRIMARY KEY(hub,origin,operation)
) WITHOUT ROWID;
CREATE INDEX sync_pending_order ON sync_pending_mutations(hub,origin,local_sequence);
CREATE TABLE sync_pending_objects (
    hub TEXT NOT NULL, origin TEXT NOT NULL, operation TEXT NOT NULL,
    scope TEXT NOT NULL, object TEXT NOT NULL, kind TEXT NOT NULL,
    base BLOB NOT NULL, before_root BLOB, candidate_root BLOB,
    predecessor TEXT, original_intent BLOB NOT NULL, holding BLOB NOT NULL,
    PRIMARY KEY(hub,origin,operation,scope,object),
    FOREIGN KEY(hub,origin,operation) REFERENCES sync_pending_mutations(hub,origin,operation)
) WITHOUT ROWID;
CREATE INDEX sync_pending_object ON sync_pending_objects(hub,scope,object,origin,operation);
CREATE TABLE sync_pending_roots (
    hub TEXT NOT NULL, origin TEXT NOT NULL, operation TEXT NOT NULL,
    scope TEXT NOT NULL, object TEXT NOT NULL, kind TEXT NOT NULL, hash TEXT NOT NULL,
    root_kind TEXT NOT NULL CHECK(root_kind IN ('block','manifest')), decoded_bytes INTEGER,
    role TEXT NOT NULL,
    CHECK((root_kind='block' AND decoded_bytes IS NOT NULL AND decoded_bytes BETWEEN 0 AND 1048576)
       OR (root_kind='manifest' AND decoded_bytes IS NULL)),
    PRIMARY KEY(hub,origin,operation,scope,object,hash,root_kind,role)
) WITHOUT ROWID;
CREATE INDEX sync_pending_roots_hash ON sync_pending_roots(hub,scope,hash,root_kind);
CREATE TABLE sync_conflicts (
    hub TEXT NOT NULL, scope TEXT NOT NULL, object TEXT NOT NULL, operation TEXT NOT NULL,
    base_authority_epoch BLOB CHECK(base_authority_epoch IS NULL OR length(base_authority_epoch)=16),
    base_version BLOB CHECK(base_version IS NULL OR length(base_version)=8),
    current_authority_epoch BLOB NOT NULL CHECK(length(current_authority_epoch)=16),
    current_version BLOB NOT NULL CHECK(length(current_version)=8),
    alternative_authority_epoch BLOB NOT NULL CHECK(length(alternative_authority_epoch)=16),
    alternative_version BLOB NOT NULL CHECK(length(alternative_version)=8),
    resolved_by TEXT, PRIMARY KEY(hub,scope,object,operation),
    CHECK((base_authority_epoch IS NULL) = (base_version IS NULL))
) WITHOUT ROWID;
CREATE TABLE sync_outbox (
    destination TEXT NOT NULL, hub TEXT NOT NULL, scope TEXT NOT NULL,
    operation TEXT NOT NULL, body BLOB NOT NULL, holding BLOB,
    priority INTEGER NOT NULL CHECK(priority BETWEEN 0 AND 3), next_due INTEGER NOT NULL,
    PRIMARY KEY(destination,hub,scope,operation)
) WITHOUT ROWID;
CREATE INDEX sync_outbox_due ON sync_outbox(next_due,priority);
CREATE TABLE sync_peer_cursors (
    peer TEXT NOT NULL, hub TEXT NOT NULL, scope TEXT NOT NULL,
    acl_revision BLOB NOT NULL CHECK(length(acl_revision)=8),
    publication_epoch BLOB NOT NULL CHECK(length(publication_epoch)=8),
    view_id TEXT NOT NULL, cursor BLOB NOT NULL,
    PRIMARY KEY(peer,hub,scope,acl_revision,publication_epoch)
) WITHOUT ROWID;
CREATE TABLE sync_peer_views (
    peer TEXT NOT NULL, hub TEXT NOT NULL, scope TEXT NOT NULL, view_id TEXT NOT NULL,
    definition BLOB NOT NULL, transition BLOB, state TEXT NOT NULL,
    PRIMARY KEY(peer,hub,scope,view_id)
) WITHOUT ROWID;
CREATE TABLE sync_consumer_cursors (
    consumer TEXT PRIMARY KEY, sequence BLOB NOT NULL CHECK(length(sequence)=8)
) WITHOUT ROWID;
CREATE TABLE sync_dependencies (
    dependency TEXT PRIMARY KEY, sequence BLOB NOT NULL CHECK(length(sequence)=8)
) WITHOUT ROWID;
CREATE TABLE sync_tasks (
    kind TEXT NOT NULL, hub TEXT NOT NULL, scope TEXT NOT NULL, task_key TEXT NOT NULL,
    wanted BLOB NOT NULL CHECK(length(wanted)=8), completed BLOB NOT NULL CHECK(length(completed)=8),
    state TEXT NOT NULL, priority INTEGER NOT NULL CHECK(priority BETWEEN 0 AND 3),
    next_due INTEGER NOT NULL, lease_generation BLOB NOT NULL CHECK(length(lease_generation)=8),
    lease_owner TEXT, attempts INTEGER NOT NULL, dependency TEXT, reason TEXT, holding BLOB,
    lease_wanted BLOB CHECK(lease_wanted IS NULL OR length(lease_wanted)=8),
    dependency_revision BLOB NOT NULL CHECK(length(dependency_revision)=8), input_dependency TEXT,
    lease_dependency_revision BLOB CHECK(lease_dependency_revision IS NULL OR length(lease_dependency_revision)=8),
    lease_dependency_sequence BLOB CHECK(lease_dependency_sequence IS NULL OR length(lease_dependency_sequence)=8),
    blocked_attempts INTEGER NOT NULL CHECK(blocked_attempts>=0),
    dependency_sequence BLOB NOT NULL CHECK(length(dependency_sequence)=8),
    CHECK(completed<=wanted),
    CHECK((state='leased' AND lease_owner IS NOT NULL AND lease_wanted IS NOT NULL
            AND lease_dependency_revision IS NOT NULL AND lease_dependency_sequence IS NOT NULL)
       OR (state!='leased' AND lease_owner IS NULL AND lease_wanted IS NULL
            AND lease_dependency_revision IS NULL AND lease_dependency_sequence IS NULL)),
    PRIMARY KEY(kind,hub,scope,task_key)
) WITHOUT ROWID;
CREATE INDEX sync_tasks_due ON sync_tasks(state,priority,next_due,hub,scope,kind,task_key);
CREATE INDEX sync_tasks_dependency ON sync_tasks(dependency,state,next_due);
-- The committed parser checkpoint of each source. Its private state root is held
-- by the root use saved beside it, swapped in the commit that advances it.
CREATE TABLE sync_sources (
    hub TEXT NOT NULL, scope TEXT NOT NULL, source TEXT NOT NULL,
    generation TEXT NOT NULL, revision BLOB NOT NULL CHECK(length(revision)=8),
    committed_offset BLOB NOT NULL CHECK(length(committed_offset)=8),
    parser_version INTEGER NOT NULL, state BLOB, state_lease BLOB,
    CHECK((state IS NULL) = (state_lease IS NULL)),
    PRIMARY KEY(hub,scope,source)
) WITHOUT ROWID;
-- Registered source index families, namespaced by destination scope and source.
CREATE TABLE sync_source_index (
    hub TEXT NOT NULL, scope TEXT NOT NULL, source TEXT NOT NULL, family TEXT NOT NULL,
    key BLOB NOT NULL CHECK(length(key) BETWEEN 1 AND 1024),
    revision BLOB NOT NULL CHECK(length(revision)=8), value BLOB NOT NULL,
    PRIMARY KEY(hub,scope,source,family,key)
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
    authority_epoch BLOB NOT NULL CHECK(length(authority_epoch)=16),
    revision BLOB NOT NULL CHECK(length(revision)=8), operation TEXT NOT NULL,
    PRIMARY KEY(hub,scope,object)
) WITHOUT ROWID;
CREATE TABLE sync_asset_blocks (
    hub TEXT NOT NULL, scope TEXT NOT NULL, hash TEXT NOT NULL,
    decoded_bytes INTEGER NOT NULL CHECK(decoded_bytes BETWEEN 0 AND 1048576),
    verified INTEGER NOT NULL CHECK(verified IN (0,1)), PRIMARY KEY(hub,scope,hash)
) WITHOUT ROWID;
CREATE TABLE sync_asset_representations (
    hub TEXT NOT NULL, scope TEXT NOT NULL, hash TEXT NOT NULL, representation TEXT NOT NULL,
    envelope BLOB NOT NULL, encoded_digest TEXT NOT NULL,
    encoded_bytes INTEGER NOT NULL CHECK(encoded_bytes BETWEEN 0 AND 1114112),
    storage_key TEXT NOT NULL, verified INTEGER NOT NULL CHECK(verified IN (0,1)),
    PRIMARY KEY(hub,scope,hash,representation),
    FOREIGN KEY(hub,scope,hash) REFERENCES sync_asset_blocks(hub,scope,hash)
) WITHOUT ROWID;
CREATE UNIQUE INDEX sync_asset_encoding ON sync_asset_representations(hub,scope,hash,envelope);
CREATE TABLE sync_asset_access (
    hub TEXT NOT NULL, scope TEXT NOT NULL, object TEXT NOT NULL, kind TEXT NOT NULL,
    hash TEXT NOT NULL, root_kind TEXT NOT NULL CHECK(root_kind IN ('block','manifest')),
    decoded_bytes INTEGER, expanded INTEGER NOT NULL DEFAULT 0 CHECK(expanded IN (0,1)),
    CHECK((root_kind='block' AND decoded_bytes IS NOT NULL AND decoded_bytes BETWEEN 0 AND 1048576)
       OR (root_kind='manifest' AND decoded_bytes IS NULL)),
    PRIMARY KEY(hub,scope,object,kind,hash,root_kind)
) WITHOUT ROWID;
CREATE INDEX sync_asset_access_pending ON sync_asset_access(hub,scope,hash,expanded,object,kind);
CREATE TABLE sync_asset_manifests (
    hub TEXT NOT NULL, scope TEXT NOT NULL, hash TEXT NOT NULL,
    page BLOB NOT NULL, complete INTEGER NOT NULL CHECK(complete IN (0,1)),
    validated INTEGER NOT NULL CHECK(validated IN (0,1)),
    PRIMARY KEY(hub,scope,hash)
) WITHOUT ROWID;
CREATE TABLE sync_asset_edges (
    hub TEXT NOT NULL, scope TEXT NOT NULL, parent TEXT NOT NULL, child TEXT NOT NULL,
    child_kind TEXT NOT NULL CHECK(child_kind IN ('block','manifest')),
    decoded_bytes INTEGER,
    CHECK((child_kind='block' AND decoded_bytes IS NOT NULL AND decoded_bytes BETWEEN 0 AND 1048576)
       OR (child_kind='manifest' AND decoded_bytes IS NULL)),
    PRIMARY KEY(hub,scope,parent,child,child_kind)
) WITHOUT ROWID;
CREATE INDEX sync_asset_edges_child ON sync_asset_edges(hub,scope,child,parent);
CREATE TABLE sync_asset_roots (
    hub TEXT NOT NULL, scope TEXT NOT NULL, object TEXT NOT NULL,
    authority_epoch BLOB NOT NULL CHECK(length(authority_epoch)=16),
    revision BLOB NOT NULL CHECK(length(revision)=8), hash TEXT NOT NULL,
    root_kind TEXT NOT NULL CHECK(root_kind IN ('block','manifest')), decoded_bytes INTEGER,
    CHECK((root_kind='block' AND decoded_bytes IS NOT NULL AND decoded_bytes BETWEEN 0 AND 1048576)
       OR (root_kind='manifest' AND decoded_bytes IS NULL)),
    PRIMARY KEY(hub,scope,object,authority_epoch,revision,hash,root_kind)
) WITHOUT ROWID;
CREATE INDEX sync_asset_roots_hash ON sync_asset_roots(hub,scope,hash);
-- Source uploads name the engine-issued step that owns them; only that step's
-- commit consumes them and only its cancellation releases them.
CREATE TABLE sync_asset_uploads (
    hub TEXT NOT NULL, scope TEXT NOT NULL, operation TEXT NOT NULL, object TEXT NOT NULL,
    kind TEXT NOT NULL, holding BLOB NOT NULL, purpose BLOB NOT NULL,
    cancelled INTEGER NOT NULL CHECK(cancelled IN (0,1)),
    source_scope TEXT, source TEXT, source_step BLOB CHECK(source_step IS NULL OR length(source_step)=8),
    consumed INTEGER NOT NULL DEFAULT 0 CHECK(consumed IN (0,1)),
    CHECK((source_scope IS NULL) = (source IS NULL) AND (source IS NULL) = (source_step IS NULL)),
    CHECK(source IS NOT NULL OR consumed=0),
    PRIMARY KEY(hub,scope,operation,object)
) WITHOUT ROWID;
CREATE INDEX sync_asset_uploads_step ON sync_asset_uploads(hub,source_scope,source,source_step) WHERE source IS NOT NULL;
-- Step revisions are issued once per source and never reused. A cancelled step
-- consumes its number; the parser checkpoint and host generation stay as they were.
CREATE TABLE sync_source_step_heads (
    hub TEXT NOT NULL, scope TEXT NOT NULL, source TEXT NOT NULL,
    issued BLOB NOT NULL CHECK(length(issued)=8),
    PRIMARY KEY(hub,scope,source)
) WITHOUT ROWID;
CREATE TABLE sync_source_steps (
    hub TEXT NOT NULL, scope TEXT NOT NULL, source TEXT NOT NULL,
    step BLOB NOT NULL CHECK(length(step)=8), generation TEXT NOT NULL,
    expected BLOB CHECK(expected IS NULL OR length(expected)=8),
    operation TEXT NOT NULL, holding BLOB NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('active','consumed','cancelled')),
    receipt BLOB, drained INTEGER NOT NULL DEFAULT 0 CHECK(drained IN (0,1)),
    CHECK((state='consumed') = (receipt IS NOT NULL)),
    CHECK(state!='active' OR drained=0),
    PRIMARY KEY(hub,scope,source,step),
    UNIQUE(hub,operation)
) WITHOUT ROWID;
CREATE UNIQUE INDEX sync_source_steps_active ON sync_source_steps(hub,scope,source) WHERE state='active';
CREATE INDEX sync_source_steps_draining ON sync_source_steps(hub,scope,source,step) WHERE state!='active' AND drained=0;
-- Every destination of one step; a destination serves one active step at a time.
CREATE TABLE sync_source_step_targets (
    hub TEXT NOT NULL, scope TEXT NOT NULL, source TEXT NOT NULL,
    step BLOB NOT NULL CHECK(length(step)=8),
    target_scope TEXT NOT NULL, target_source TEXT NOT NULL, object TEXT NOT NULL, kind TEXT NOT NULL,
    stream TEXT NOT NULL, holding BLOB NOT NULL, purpose BLOB NOT NULL,
    active INTEGER NOT NULL CHECK(active IN (0,1)),
    PRIMARY KEY(hub,scope,source,step,target_scope,target_source),
    FOREIGN KEY(hub,scope,source,step) REFERENCES sync_source_steps(hub,scope,source,step)
) WITHOUT ROWID;
CREATE UNIQUE INDEX sync_source_step_targets_active ON sync_source_step_targets(hub,target_scope,target_source) WHERE active=1;
CREATE TABLE sync_asset_pins (
    hub TEXT NOT NULL, scope TEXT NOT NULL, operation TEXT NOT NULL, object TEXT NOT NULL,
    kind TEXT NOT NULL, hash TEXT NOT NULL,
    root_kind TEXT NOT NULL CHECK(root_kind IN ('block','manifest')), decoded_bytes INTEGER,
    generation BLOB NOT NULL CHECK(length(generation)=8),
    phase TEXT NOT NULL CHECK(phase IN ('staging','ready','released')), holding BLOB NOT NULL,
    CHECK((root_kind='block' AND decoded_bytes IS NOT NULL AND decoded_bytes BETWEEN 0 AND 1048576)
       OR (root_kind='manifest' AND decoded_bytes IS NULL)),
    PRIMARY KEY(hub,scope,operation,object,hash,root_kind)
) WITHOUT ROWID;
CREATE INDEX sync_asset_pins_hash ON sync_asset_pins(hub,scope,hash) WHERE phase!='released';
CREATE INDEX sync_asset_pins_cleanup ON sync_asset_pins(hub,scope,operation,object,kind,hash,root_kind) WHERE phase!='released';
CREATE INDEX sync_asset_pin_knowledge ON sync_asset_pins(hub,scope,hash,root_kind,object,kind);
CREATE INDEX sync_asset_pin_validation ON sync_asset_pins(hub,scope,hash,root_kind,operation,object);
-- Local use lifetimes retain a complete closure without granting body access.
CREATE TABLE sync_root_leases (
    hub TEXT NOT NULL, scope TEXT NOT NULL, lease TEXT NOT NULL,
    object TEXT NOT NULL, kind TEXT NOT NULL, hash TEXT NOT NULL,
    root_kind TEXT NOT NULL CHECK(root_kind IN ('block','manifest')), decoded_bytes INTEGER,
    operation TEXT NOT NULL, generation BLOB NOT NULL CHECK(length(generation)=8),
    usage BLOB NOT NULL, holding BLOB NOT NULL, purpose BLOB NOT NULL, expires INTEGER,
    released INTEGER NOT NULL CHECK(released IN (0,1)),
    CHECK((root_kind='block' AND decoded_bytes IS NOT NULL AND decoded_bytes BETWEEN 0 AND 1048576)
       OR (root_kind='manifest' AND decoded_bytes IS NULL)),
    PRIMARY KEY(hub,scope,lease)
) WITHOUT ROWID;
CREATE INDEX sync_root_leases_hash ON sync_root_leases(hub,scope,hash,root_kind) WHERE released=0;
CREATE INDEX sync_root_leases_scope ON sync_root_leases(hub,scope,lease) WHERE released=0;
CREATE INDEX sync_root_leases_object ON sync_root_leases(hub,scope,object,kind,hash,root_kind,decoded_bytes) WHERE released=0;
CREATE INDEX sync_root_leases_due ON sync_root_leases(expires,hub,scope,lease) WHERE released=0 AND expires IS NOT NULL;
CREATE TABLE sync_asset_transfers (
    hub TEXT NOT NULL, scope TEXT NOT NULL, hash TEXT NOT NULL, peer TEXT NOT NULL,
    publication_epoch BLOB NOT NULL CHECK(length(publication_epoch)=8),
    acl_revision BLOB NOT NULL CHECK(length(acl_revision)=8), representation TEXT NOT NULL,
    progress BLOB NOT NULL, source_hint TEXT, phase TEXT NOT NULL,
    PRIMARY KEY(hub,scope,hash,peer,acl_revision,publication_epoch,representation)
) WITHOUT ROWID;
CREATE TABLE sync_asset_gc (
    hub TEXT NOT NULL, scope TEXT NOT NULL, hash TEXT NOT NULL,
    root_kind TEXT NOT NULL CHECK(root_kind IN ('block','manifest')),
    generation BLOB NOT NULL CHECK(length(generation)=8), next_due INTEGER NOT NULL,
    claimed INTEGER NOT NULL DEFAULT 0 CHECK(claimed IN (0,1)),
    failures INTEGER NOT NULL DEFAULT 0 CHECK(failures BETWEEN 0 AND 16),
    failure TEXT NOT NULL DEFAULT '',
    PRIMARY KEY(hub,scope,hash,root_kind)
) WITHOUT ROWID;
CREATE INDEX sync_asset_gc_due ON sync_asset_gc(next_due);
-- A batch operation names one signed batch per stream epoch; retries return it.
CREATE TABLE sync_source_proofs (
    hub TEXT NOT NULL, scope TEXT NOT NULL, proof TEXT NOT NULL,
    stream TEXT NOT NULL, epoch TEXT NOT NULL, producer BLOB NOT NULL, operation TEXT NOT NULL,
    first_sequence BLOB NOT NULL CHECK(length(first_sequence)=8),
    last_sequence BLOB NOT NULL CHECK(length(last_sequence)=8),
    purpose TEXT NOT NULL, signer BLOB NOT NULL CHECK(length(signer)=32),
    signed_bytes BLOB NOT NULL, signature BLOB NOT NULL CHECK(length(signature)=64),
    state TEXT NOT NULL CHECK(state IN ('local','accepted','awaiting_epoch')), PRIMARY KEY(hub,scope,proof),
    UNIQUE(hub,scope,stream,epoch,operation)
) WITHOUT ROWID;
CREATE INDEX sync_source_waiting_epoch ON sync_source_proofs(hub,scope,stream,epoch,state);
CREATE TABLE sync_epoch_descriptors (
    hub TEXT NOT NULL, scope TEXT NOT NULL, stream TEXT NOT NULL, epoch TEXT NOT NULL,
    producer BLOB NOT NULL, kind TEXT NOT NULL, schema INTEGER NOT NULL,
    predecessor BLOB, closed_through BLOB CHECK(closed_through IS NULL OR length(closed_through)=8),
    proof TEXT NOT NULL, signed_bytes BLOB NOT NULL,
    signer BLOB NOT NULL CHECK(length(signer)=32), signature BLOB NOT NULL CHECK(length(signature)=64),
    operation TEXT NOT NULL, PRIMARY KEY(hub,scope,stream,epoch)
) WITHOUT ROWID;
CREATE TABLE sync_hub_signing_keys (
    hub TEXT NOT NULL, public_key BLOB NOT NULL CHECK(length(public_key)=32),
    revision BLOB NOT NULL CHECK(length(revision)=8), rotation_proof BLOB,
    current INTEGER NOT NULL CHECK(current IN (0,1)),
    PRIMARY KEY(hub,public_key)
) WITHOUT ROWID;
CREATE TABLE sync_hub_control_heads (
    hub TEXT PRIMARY KEY, sequence BLOB NOT NULL CHECK(length(sequence)=8)
) WITHOUT ROWID;
CREATE TABLE sync_exposure_intents (
    hub TEXT NOT NULL, machine TEXT NOT NULL, generation BLOB NOT NULL CHECK(length(generation)=8),
    operation TEXT NOT NULL, request BLOB NOT NULL, holders BLOB NOT NULL,
    policy_revision BLOB NOT NULL CHECK(length(policy_revision)=8), plan_digest TEXT NOT NULL,
    state TEXT NOT NULL, PRIMARY KEY(hub,machine,generation), UNIQUE(hub,machine,operation)
) WITHOUT ROWID;
CREATE TABLE sync_exposure_receipts (
    hub TEXT NOT NULL, machine TEXT NOT NULL, generation BLOB NOT NULL CHECK(length(generation)=8),
    operation TEXT NOT NULL, plan_digest TEXT NOT NULL, signed_bytes BLOB NOT NULL,
    signer BLOB NOT NULL CHECK(length(signer)=32), signature BLOB NOT NULL CHECK(length(signature)=64),
    PRIMARY KEY(hub,machine,generation)
) WITHOUT ROWID;
CREATE TABLE sync_exposure_cleanup (
    hub TEXT NOT NULL, machine TEXT NOT NULL, generation BLOB NOT NULL CHECK(length(generation)=8),
    phase TEXT NOT NULL, checkpoint BLOB NOT NULL, blocked_reason TEXT,
    PRIMARY KEY(hub,machine,generation)
) WITHOUT ROWID;
CREATE TABLE sync_peer_capabilities (
    peer TEXT NOT NULL, hub TEXT NOT NULL, advertised BLOB NOT NULL,
    exposure_contract TEXT, PRIMARY KEY(peer,hub)
) WITHOUT ROWID;
CREATE TABLE sync_timeline_items (
    hub TEXT NOT NULL, scope TEXT NOT NULL, conversation TEXT NOT NULL, item TEXT NOT NULL,
    revision BLOB NOT NULL CHECK(length(revision)=8), accepted BLOB NOT NULL CHECK(length(accepted)=8),
    position BLOB NOT NULL, body BLOB NOT NULL, turn TEXT NOT NULL, parent TEXT,
    active INTEGER NOT NULL CHECK(active IN (0,1)),
    PRIMARY KEY(hub,scope,conversation,item,revision)
) WITHOUT ROWID;
CREATE INDEX sync_timeline_order ON sync_timeline_items(hub,scope,conversation,active,position,item);
-- What each conversation's timeline has installed. `root` is absent until the
-- first item revision; `applied` is the source commit installation caught up
-- with; `local` is this commit's local sequence, which readers resume after;
-- `indexed` lags `projection` while a branch switch rebuilds the position index.
CREATE TABLE sync_timeline_heads (
    hub TEXT NOT NULL, scope TEXT NOT NULL, conversation TEXT NOT NULL,
    projection BLOB NOT NULL CHECK(length(projection)=8), through_cursor BLOB NOT NULL, root TEXT,
    indexed BLOB CHECK(indexed IS NULL OR length(indexed)=8),
    applied BLOB NOT NULL CHECK(length(applied)=8), local BLOB NOT NULL CHECK(length(local)=8),
    CHECK((root IS NULL) = (projection=X'0000000000000000')),
    PRIMARY KEY(hub,scope,conversation)
) WITHOUT ROWID;
CREATE INDEX sync_timeline_heads_root ON sync_timeline_heads(hub,scope,root) WHERE root IS NOT NULL;
CREATE INDEX sync_timeline_heads_local ON sync_timeline_heads(hub,scope,local);
