CREATE TABLE sync_legacy_pins (
    conversation TEXT NOT NULL, source TEXT NOT NULL, identity TEXT NOT NULL,
    fingerprint TEXT NOT NULL, kind TEXT NOT NULL, body BLOB, selector TEXT,
    records INTEGER NOT NULL DEFAULT 0, bytes INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(conversation,source,identity,fingerprint),
    CHECK(records >= 0 AND bytes >= 0),
    CHECK(body IS NULL OR length(body) <= 8388608)
) WITHOUT ROWID;
CREATE INDEX sync_legacy_pin_totals ON sync_legacy_pins(source,conversation) WHERE source='@totals';
CREATE TRIGGER sync_legacy_pin_added AFTER INSERT ON sync_legacy_pins WHEN NEW.source!='@totals'
BEGIN
    INSERT INTO sync_legacy_pins(conversation,source,identity,fingerprint,kind,records,bytes)
    VALUES(NEW.conversation,'@totals','','','runtime.migration',1,coalesce(length(NEW.body),0))
    ON CONFLICT(conversation,source,identity,fingerprint)
    DO UPDATE SET records=records+1,bytes=bytes+coalesce(length(NEW.body),0);
END;
CREATE TRIGGER sync_legacy_pin_deleted AFTER DELETE ON sync_legacy_pins WHEN OLD.source!='@totals'
BEGIN
    UPDATE sync_legacy_pins SET records=records-1,bytes=bytes-coalesce(length(OLD.body),0)
    WHERE conversation=OLD.conversation AND source='@totals';
END;
