-- Counters are unsigned big-endian eight-byte values. The owning database is
-- the data root; these records have no account scope.
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
