//! A partial archival still exposes every standalone entry until the identity changes.

use super::*;

pub(super) fn entries(world: &WorldDir, town: &TownName) -> io::Result<Option<Vec<(u64, Entry)>>> {
    let Some(super::carry::Originals { place, archived: originals, retained }) = super::carry::originals(world, town)?
    else {
        return Ok(None);
    };
    let signed_dir = world.town(town).signed();
    let number = |path: &Path| {
        (path.parent() == Some(signed_dir.as_path()))
            .then(|| path.file_stem().and_then(|name| name.to_str()).and_then(|name| name.parse::<u64>().ok()))
            .flatten()
    };
    let mut paths = Vec::new();
    for original in originals {
        if let Some(seq) = number(original.source()) {
            original.verify()?;
            paths.push((seq, original.readable().to_path_buf()));
        }
    }
    for original in retained {
        if let Some(seq) = number(original.source()) {
            paths.push((seq, original.readable()?.to_path_buf()));
        }
    }
    let mut entries = Vec::new();
    for (seq, path) in paths {
        let signed: Signed = fastctx_net::link::state::read(&path)?;
        let claimed: Entry = signed.claim().map_err(io::Error::other)?;
        if claimed.hub != place.hub || claimed.town != place.town {
            return Err(io::Error::other("a retained source belongs to another ledger"));
        }
        let entry = if claimed.key == fastctx_net::sign::PublicKey::from_bytes([0; 32])
            && !fastctx_net::memory::has_memory(&claimed.files)
        {
            signed.open(&own_key(world)?.public(), fastctx_net::sign::Domain::Ledger).map_err(io::Error::other)?
        } else {
            ledger::open(&signed, &place.hub, &place.town).map_err(io::Error::other)?
        };
        entries.push((seq, entry));
    }
    entries.sort_by_key(|(seq, _)| *seq);
    Ok(Some(entries))
}
