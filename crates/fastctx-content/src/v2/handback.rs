//! Frames a hub put back from an older copy is handed back by the devices
//! that hold them (design §28.38): while its assets are sealed for the
//! recovery, a device gives it, frame by frame, what of a stream the hub
//! orders reaches past where the hub holds it now. The hub keeps each as
//! evidence of the scope's holding period, its objects with it, and takes
//! the longest chain of them back as it reopens.

use super::Library;
use super::exchange::RawFrame;
use super::frame::{Header, open};
use super::scopes::held;
use crate::error::{Error, invalid};
use crate::{ObjectRef, Result};
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{OptionalExtension, params};
use sha2::Digest as _;
use std::collections::BTreeSet;

/// Where a hub's library holds a stream as it is: the epoch it is in, the
/// device and key that epoch was signed for if one was, and the end of its
/// applied prefix — the next sequence and the chain value it ends with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tail {
    pub epoch: u64,
    pub orderer: Option<([u8; 32], [u8; 32])>,
    pub next: u64,
    pub head: [u8; 32],
}

/// A frame handed back, read: its header, and the objects its records
/// refer to, each once.
#[derive(Clone, Debug)]
pub struct HandedBack {
    pub header: Header,
    pub objects: Vec<ObjectRef>,
}

/// Reads `frame`, handed back for `scope`: its header decodes, its records
/// match the header's digest and are well formed, and it refers to each
/// object at one length. Whose signature it bears is the caller's to check.
pub fn read_handed_back(scope: &str, frame: &RawFrame) -> Result<HandedBack> {
    let header = Header::decode(&frame.header).map_err(Error::Invalid)?;
    if header.scope != scope {
        return Err(Error::Invalid("the frame is of another scope".into()));
    }
    let records = open(&header, &frame.body).map_err(Error::Invalid)?;
    for record in &records {
        record.record.check().map_err(Error::Invalid)?;
    }
    super::hub::lengths_agree(&records)?;
    Ok(HandedBack { header, objects: super::hub::objects_of(&records) })
}

impl Library {
    /// Where this library holds `stream` of `scope` in the holding period
    /// the scope is in; a stream it has nothing of yet stands at its start,
    /// in its first epoch. `None` when the scope is not held here.
    pub fn tail(&self, scope: &str, stream: &str) -> Result<Option<Tail>> {
        let (scope, stream) = (scope.to_string(), stream.to_string());
        Ok(self.database().read_now(move |db| {
            let Some((scope_id, _, Some(holding))) = held(db, &scope)? else { return Ok(None) };
            let Some(at) = super::hub::standing(db, scope_id, holding, &stream)? else {
                return Ok(Some(Tail { epoch: 0, orderer: None, next: 0, head: [0; 32] }));
            };
            let orderer = super::handover::epoch_of(db, at.stream, at.epoch)?.map(|epoch| (epoch.device, epoch.key));
            Ok(Some(Tail { epoch: at.epoch, orderer, next: at.next, head: at.head }))
        })?)
    }

    /// Keeps `frame`, handed back to this hub for `scope`, as evidence of
    /// the holding period the scope is in, saying `why`. `objects` are the
    /// bytes of objects its records refer to, kept with it; every object it
    /// refers to is here after, at its length, or nothing is kept and the
    /// answer says what is missing. An object given that the frame does not
    /// refer to is not kept.
    pub fn keep_handed_back(
        &self,
        scope: &str,
        frame: &RawFrame,
        objects: &[Vec<u8>],
        why: &str,
    ) -> Result<std::result::Result<(), String>> {
        let read = read_handed_back(scope, frame)?;
        let referred: BTreeSet<([u8; 32], u64)> = read.objects.iter().map(|object| (object.hash, object.size)).collect();
        // Nothing is written before all is checked: each object given is one
        // the frame refers to, at its length, and each it refers to not given
        // is here already (codex-f92d833e #4).
        let mut given = BTreeSet::new();
        for bytes in objects {
            let hash: [u8; 32] = sha2::Sha256::digest(bytes).into();
            if !referred.contains(&(hash, bytes.len() as u64)) {
                return Ok(Err("an object given is not one the frame refers to".to_string()));
            }
            given.insert(hash);
        }
        for object in &read.objects {
            if given.contains(&object.hash) {
                continue;
            }
            match self.size(&object.hash)? {
                Some(size) if size == object.size => {}
                Some(_) => return Ok(Err("an object it refers to is here with another length".into())),
                None => return Ok(Err(format!("an object it refers to is missing ({} bytes)", object.size))),
            }
        }
        let mut leases = Vec::with_capacity(objects.len());
        let mut wrong = None;
        for bytes in objects {
            match self.put(&mut bytes.as_slice(), "handed back", super::LEASE) {
                Ok(lease) => leases.push(lease),
                Err(error) => {
                    wrong = Some(error.to_string());
                    break;
                }
            }
        }
        let kept = match wrong {
            Some(why) => Err(why),
            None => {
                let (key, header, why) = (scope.to_string(), read.header, why.to_string());
                let stripped = RawFrame { header: frame.header.clone(), signature: frame.signature.clone(), body: frame.body.clone(), objects: Vec::new() };
                let refs = read.objects;
                let bytes = stripped.header.len() + stripped.signature.len() + stripped.body.len() + refs.len() * 96 + 1024;
                self.database().write_now_at(Priority::Normal, bytes, move |db| {
                    let Some((scope_id, _, Some(holding))) = held(db, &key)? else { return Err(invalid("the scope is not held here")) };
                    let at = super::hub::prefix(db, scope_id, holding, &header.stream)?;
                    // Shown here: its bytes are here in this transaction.
                    let mut shown = BTreeSet::new();
                    for object in &refs {
                        if super::objects::present(db, &object.hash)?.is_some() {
                            shown.insert(object.hash);
                        }
                    }
                    if shown.len() != refs.len() {
                        return Ok(Err("an object it refers to went while it was kept".to_string()));
                    }
                    super::handover::keep_evidence(db, (at.stream, holding), &header, &stripped, &why, &refs, &shown)?;
                    Ok(Ok(()))
                })?
            }
        };
        for lease in leases {
            self.release(lease)?;
        }
        if kept.is_ok() {
            self.wrote();
        }
        Ok(kept)
    }

    /// A frame of `stream` of `scope` kept as evidence in the holding period
    /// the scope is in, by its epoch, where it starts and its chain value.
    pub fn evidence_frame(&self, scope: &str, stream: &str, at: (u64, u64), chain: &[u8; 32]) -> Result<Option<RawFrame>> {
        let (scope, stream, chain) = (scope.to_string(), stream.to_string(), *chain);
        let (epoch, first) = at;
        Ok(self.database().read_now(move |db| {
            let Some((scope_id, _, Some(holding))) = held(db, &scope)? else { return Ok(None) };
            let Some(stream_id) = db
                .prepare_cached("SELECT id FROM streams WHERE scope = ?1 AND name = ?2")?
                .query_row(params![scope_id, stream], |row| row.get::<_, i64>(0))
                .optional()?
            else {
                return Ok(None);
            };
            Ok(db
                .prepare_cached(
                    "SELECT header, signature, body FROM evidence
                     WHERE stream = ?1 AND epoch = ?2 AND first = ?3 AND chain = ?4 AND holding = ?5 LIMIT 1",
                )?
                .query_row(params![stream_id, epoch as i64, first as i64, &chain[..], holding], |row| {
                    Ok(RawFrame { header: row.get(0)?, signature: row.get(1)?, body: row.get(2)?, objects: Vec::new() })
                })
                .optional()?)
        })?)
    }
}
