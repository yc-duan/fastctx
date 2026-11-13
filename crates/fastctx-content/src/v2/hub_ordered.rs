//! Streams the hub orders itself (design §2.2, §28.17): it writes on from
//! the prefix it applied, in an epoch whose ordering device is the hub, each
//! frame signed by its service key in the domain devices sign theirs in, and
//! takes the frames as it takes a device's. Devices hold them as replicas.

use super::Library;
use super::exchange::RawFrame;
use super::frame::{Header, KindRecord, Signer, seal};
use super::scopes::held;
use crate::Result;
use crate::error::{Error, invalid};
use fastctx_storage::rusqlite::{OptionalExtension, params};

/// The streams the hub orders, in a conversation's scope, an account's or a
/// project's: no device writes them.
pub const HUB_ORDERED: [&str; 9] =
    ["plan", "history", "goals", "sentinels", "settings", "skills", "ledger", "audit", "notices"];

impl Library {
    /// Frames of `records` for `stream` of `scope`, ordered by the hub, whose
    /// service key `signer` is: chained on from what the hub applied of it,
    /// in its epoch. Nothing is written here; each is taken in order, and the
    /// writes of one stream are made one at a time.
    pub fn hub_frames(
        &self,
        scope: &str,
        stream: &str,
        records: Vec<KindRecord>,
        signer: &dyn Signer,
    ) -> Result<Vec<RawFrame>> {
        let (key, name) = (scope.to_string(), stream.to_string());
        let (epoch, mut first, mut previous) = self.database().read_now(move |db| {
            let scope = key;
            let Some((scope_id, _, Some(holding))) = held(db, &scope)? else {
                return Err(invalid("the scope is not held here"));
            };
            let found: Option<(i64, i64)> = db
                .prepare_cached("SELECT id, epoch FROM streams WHERE scope = ?1 AND name = ?2")?
                .query_row(params![scope_id, name], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()?;
            let Some((stream, epoch)) = found else { return Ok((0, 0, [0; 32])) };
            let cursor: Option<(i64, Option<Vec<u8>>)> = db
                .prepare_cached("SELECT sequence, head FROM cursors WHERE stream = ?1 AND holding = ?2")?
                .query_row(params![stream, holding], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()?;
            let (next, head) = match cursor {
                Some((sequence, head)) => (
                    sequence as u64 + 1,
                    head.unwrap_or_default().try_into().map_err(|_| invalid("a malformed cursor"))?,
                ),
                None => (0, [0; 32]),
            };
            Ok((epoch as u64, next, head))
        })?;
        // Ordered by a device, the stream is not the hub's to write.
        if let Some((last, key)) = self.last_epoch(scope, stream)?
            && (last != epoch || key != signer.key())
        {
            return Err(Error::Invalid("the stream is ordered by a device, not the hub".into()));
        }
        let sealed = seal(&records).map_err(Error::Invalid)?;
        let mut frames = Vec::with_capacity(sealed.len());
        for piece in sealed {
            let header = Header {
                scope: scope.to_string(),
                stream: stream.to_string(),
                epoch,
                first,
                count: piece.count,
                previous,
                digest: piece.digest,
                device: signer.device(),
                key: signer.key(),
            };
            let encoded = header.encode();
            let signature = signer.sign(&encoded);
            previous = header.chain();
            first += u64::from(piece.count);
            frames.push(RawFrame { header: encoded, signature, body: piece.body, objects: Vec::new() });
        }
        Ok(frames)
    }
}
