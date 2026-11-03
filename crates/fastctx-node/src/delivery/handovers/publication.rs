//! Handover inputs and their head publish together for a small submission.
//! Larger snapshots retain bounded staging before the head makes them visible.
use super::*;

pub(crate) fn put(
    directory: &ConversationDir,
    handover: Handover,
    imported: Option<Timestamp>,
) -> io::Result<Handover> {
    // Import already owns the family's migration gate; normal publication
    // owns only this handover's gate, never two gates at the same rank.
    let gate = imported.is_none().then(|| GATES.get(&directory.path().join(format!("handover-{}", handover.id))));
    let _gate = gate.as_ref().map(|gate| gate.lock());
    collect_inputs(directory, handover.id.as_str())?;
    let prior = records::read(directory, |rows| header(rows, handover.id.as_str()))?;
    if prior.as_ref().is_some_and(|prior| prior.turn.is_some() || prior.unresolved.is_some()) {
        return records::read(directory, |rows| {
            get(rows, &handover.id)?.ok_or_else(|| Error::new("handover disappeared"))
        });
    }
    if handover.inputs.len() > INPUTS || serde_json::to_vec(&handover)?.len() > BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "handover exceeds its 16,384 input / 64 MiB admission budget",
        ));
    }
    // A snapshot becomes visible only after all of its ordered rows commit.
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(io::Error::other)?;
    let revision = format!("{:x}", u128::from_be_bytes(random));
    let mut small = Vec::new();
    for (batch, inputs) in handover.inputs.chunks(16).enumerate() {
        let mut prepared = Vec::new();
        for (offset, input) in inputs.iter().enumerate() {
            prepared.push((batch * 16 + offset, input.delivery.id.to_string(), Prepared::new(directory, input)?));
        }
        if handover.inputs.len() <= 4 {
            small = prepared;
        } else {
            let (id, revision) = (handover.id.clone(), revision.clone());
            records::write_at(directory, Priority::Interactive, 16 * 8192, move |rows| {
                install_inputs(rows, id.as_str(), &revision, prepared)
            })?;
        }
    }
    let context = Prepared::new(
        directory,
        &Context { receipt: handover.context.clone(), stamp: handover.context_input.clone() },
    )?;
    let unresolved = handover.unresolved.as_ref().map(|end| Prepared::new(directory, end)).transpose()?;
    let (id, turn, attempting, count) =
        (handover.id.clone(), handover.turn.clone(), handover.attempting, handover.inputs.len());
    let published = revision.clone();
    let modified = imported.unwrap_or_else(Timestamp::now);
    let installed = records::write_at(directory, Priority::Interactive, 16 * 1024 + small.len() * 8192, move |rows| {
        if header(rows, id.as_str())?.is_some_and(|head| head.turn.is_some() || head.unresolved.is_some()) {
            return Ok(false);
        }
        install_inputs(rows, id.as_str(), &published, small)?;
        let actual: i64 = rows.connection().query_row(
            "SELECT count(*) FROM handover_inputs WHERE conversation=?1 AND handover=?2 AND revision=?3",
            (rows.conversation(), id.as_str(), &published),
            |row| row.get(0),
        )?;
        if actual != count as i64 {
            return Err(Error::new("handover snapshot is incomplete"));
        }
        context.install(rows, "handover-context", id.as_str())?;
        if let Some(end) = &unresolved {
            end.install(rows, "handover-unresolved", id.as_str())?;
        }
        rows.connection().execute("INSERT INTO handover_heads(conversation,identity,revision,context,unresolved,turn,attempting,active,modified,modified_millis) VALUES(?1,?2,?3,?4,?5,?6,?7,1,?8,?9) ON CONFLICT(conversation,identity) DO UPDATE SET revision=excluded.revision,context=excluded.context,unresolved=excluded.unresolved,turn=excluded.turn,attempting=excluded.attempting,active=1,bound=0,modified=excluded.modified,modified_millis=excluded.modified_millis", params![rows.conversation(), id.as_str(), published, records::encode(&context.value)?, unresolved.as_ref().map(|value| records::encode(&value.value)).transpose()?, turn, attempting, modified.to_string(), modified.as_millisecond()])?;
        Ok(true)
    })?;
    if installed {
        Ok(Handover { revision, ..handover })
    } else {
        records::read(directory, |rows| get(rows, &handover.id)?.ok_or_else(|| Error::new("handover disappeared")))
    }
}

fn install_inputs(rows: &Rows<'_>, id: &str, revision: &str, inputs: Vec<(usize, String, Prepared)>) -> Result<()> {
    for (ordinal, input, value) in inputs {
        value.install(rows, "handover-inputs", &root(id, revision, ordinal))?;
        rows.connection().execute("INSERT INTO handover_inputs(conversation,handover,revision,ordinal,identity,body) VALUES(?1,?2,?3,?4,?5,?6)", params![rows.conversation(), id, revision, ordinal as i64, input, records::encode(&value.value)?])?;
    }
    Ok(())
}
