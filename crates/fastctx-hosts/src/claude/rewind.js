// Checkpoints contain only hashes of public message projections. Handles are
// opaque; they are passed back unchanged, never compared with transcript UUIDs.
async function messageFingerprints(messages) {
  const hashes = []
  for (const message of messages) {
    const projection = messageProjection(message)
    const bytes = new TextEncoder().encode(JSON.stringify(canonicalMessage(projection)))
    const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', bytes))
    hashes.push([...digest].map((byte) => byte.toString(16).padStart(2, '0')).join(''))
  }
  return hashes
}

function messageProjection(message) {
  return { role: message.role, text: message.text,
    toolUses: message.toolUses.map(({ tool_use_id, tool, input }) => ({ tool_use_id, tool, input })),
    toolResults: (message.toolResults ?? []).map(({ tool_use_id, text, isError }) => ({ tool_use_id, text, isError: isError === true })) }
}

function canonicalMessage(value) {
  if (Array.isArray(value)) return value.map(canonicalMessage)
  if (value !== null && typeof value === 'object') return Object.fromEntries(Object.keys(value).sort().map((key) => [key, canonicalMessage(value[key])]))
  return value
}

async function controlCheckpoint($, state, turn, before) {
  if (before || state.checkpoints.full !== true) {
    const messages = await $.session.messages()
    const hashes = await messageFingerprints(messages)
    const previous = state.checkpoints.messages
    const tail = previous.slice(-hashes.length)
    const matches = state.checkpoints.full === true && (messages.length < 4096 ? previous.length === hashes.length : previous.length >= hashes.length)
      && tail.every((hash, index) => hashes[index] === hash)
    if (!matches) {
      state.checkpoints.turns = {}
      state.checkpoints.messages = hashes
      state.checkpoints.uuids = hashes.map(() => null)
      state.checkpoints.full = messages.length < 4096
      state.lastUuid = undefined
      state.lastRawUuid = undefined
    }
  }
  if (state.checkpoints.full !== true) return null
  const hashes = state.checkpoints.messages
  if (before && hashes.length > 0 && state.lastUuid === undefined) {
    // Where Claude Code's file cannot say which of its rows the session's
    // messages are, as when a failed request left a message only in memory,
    // the turn goes on without a boundary to rewind to rather than waiting.
    try {
      const argv = [FASTCTX, 'claude-session-policy', WORLD, state.host.session, '--messages', String(hashes.length)]
      if (state.checkpoints.compacted_parent != null) argv.push('--after-compact', state.checkpoints.compacted_parent)
      const child = $.process.spawn({ argv, input: JSON.stringify({ tail: (await $.session.messages()).map(messageProjection) }) })
      let stdout = '', stderr = ''
      for await (const piece of child) { if (piece.stream === 'stdout') stdout += piece.text; else stderr += piece.text }
      if ((await child.result).code !== 0) throw new Error('reading the retained context identity: ' + stderr)
      const policy = JSON.parse(stdout)
      state.lastUuid = policy.last_uuid ?? undefined
      state.lastRawUuid = policy.context_tail ?? undefined
      state.checkpoints.uuids = policy.context_uuids
      delete state.checkpoints.compacted_parent
    } catch (error) {
      $.ui.log('World checkpoint: ' + String(error), { to: 'debug' })
    }
  }
  state.checkpoints.last_uuid = state.lastUuid ?? null
  state.checkpoints.last_raw_uuid = state.lastRawUuid ?? null
  if (turn !== null) {
    state.checkpoints.turns[turn] ??= {}
    state.checkpoints.turns[turn][before ? 'before' : 'after'] = hashes.length
    state.checkpoints.turns[turn][before ? 'before_uuid' : 'after_uuid'] = state.lastUuid ?? null
  }
  await saveCheckpoints($, state)
  return hashes.length
}

// The public history includes messages discarded by compaction. Bind its
// retained suffix to the native context before exporting or importing a fork.
async function controlRetained($, state) {
  const publicMessages = await $.session.messages()
  const child = $.process.spawn({ argv: [FASTCTX, 'claude-session-policy', WORLD, state.host.session, '--messages', String(publicMessages.length)],
    input: JSON.stringify({ retained: true, tail: publicMessages.map(messageProjection) }) })
  let stdout = '', stderr = ''
  for await (const piece of child) { if (piece.stream === 'stdout') stdout += piece.text; else stderr += piece.text }
  if ((await child.result).code !== 0) throw new Error('reading the retained fork context: ' + stderr)
  const policy = JSON.parse(stdout)
  if (policy.context_contract !== 1) throw new Error('the World node is too old to identify the retained fork context')
  const messages = publicMessages.slice(Math.max(0, publicMessages.length - policy.retained_messages))
  return { messages, hashes: await messageFingerprints(messages), policy }
}

async function controlExportCheckpoint($, state) {
  const retained = await controlRetained($, state)
  let hashes = retained.hashes
  const policy = retained.policy
  const previous = state.checkpoints
  if (policy.retained_messages > hashes.length) {
    const tail = previous.messages.slice(-hashes.length)
    if (previous.full !== true || previous.messages.length !== policy.retained_messages
      || !hashes.every((hash, index) => tail[index] === hash)) return null
    hashes = previous.messages
  }
  const offset = previous.messages.length - hashes.length
  if (previous.full === true && offset >= 0 && hashes.every((hash, index) => previous.messages[offset + index] === hash)) {
    state.checkpoints = checkpointsFrom(previous, offset)
  } else state.checkpoints = { messages: hashes, turns: {}, full: true, uuids: [] }
  state.checkpoints.uuids = policy.context_uuids
  state.lastUuid = policy.last_uuid ?? undefined
  state.lastRawUuid = policy.context_tail ?? undefined
  state.checkpoints.last_uuid = state.lastUuid ?? null
  state.checkpoints.last_raw_uuid = state.lastRawUuid ?? null
  delete state.checkpoints.compacted_parent
  await saveCheckpoints($, state)
  return state.checkpoints
}

async function saveCheckpoints($, state) {
  await writeReport($, state.directory + '/checkpoints', JSON.stringify(state.checkpoints))
}

async function trackCheckpoint($, state, result) {
  const message = result.message
  if (message.type !== 'assistant' && (message.type !== 'user' || message.isMeta === true || message.role === undefined)) return false
  if (state.checkpoints.full !== true) return true
  const blocks = message.content
  const text = blocks.filter((block) => block.type === 'text').map((block) => block.text).join('')
  const toolUses = blocks.filter((block) => block.type === 'tool_use').map((block) => ({ tool_use_id: block.id, tool: block.name, input: block.input }))
  const toolResults = blocks.filter((block) => block.type === 'tool_result').map((block) => ({ tool_use_id: block.tool_use_id,
    text: typeof block.content === 'string' ? block.content : (block.content ?? []).filter((part) => part !== null && typeof part === 'object' && part.type === 'text').map((part) => String(part.text ?? '')).join('\n'), isError: block.is_error === true }))
  const [hash] = await messageFingerprints([{ role: message.type, text, toolUses, toolResults }])
  state.checkpoints.uuids ??= state.checkpoints.messages.map(() => null)
  const index = state.checkpoints.uuids.indexOf(result.uuid)
  if (index < 0) {
    state.checkpoints.uuids.push(result.uuid)
    state.checkpoints.messages.push(hash)
  } else state.checkpoints.messages[index] = hash
  return true
}

// The checkpoints without their first `offset` messages: a turn that began
// before them keeps where it ended, one that ended before them goes.
function checkpointsFrom(checkpoints, offset) {
  const turns = {}
  for (const [turn, point] of Object.entries(checkpoints.turns)) {
    if (point.after !== undefined && point.after - offset <= 0) continue
    const kept = { ...point }
    if (point.after !== undefined) kept.after = point.after - offset
    if (point.before !== undefined && point.before - offset > 0) kept.before = point.before - offset
    else {
      delete kept.before
      kept.before_uuid = null
    }
    turns[turn] = kept
  }
  return { ...checkpoints, messages: checkpoints.messages.slice(offset), uuids: checkpoints.uuids.slice(offset), turns }
}

function checkpointCut(current, checkpoint) {
  if (!checkpoint.length) throw new Error('Claude Code cannot retain an empty conversation through session.compact')
  if (!checkpoint.every((hash, index) => current[index] === hash)) throw new Error('the selected turn was compacted away or has no retained message boundary')
  return checkpoint.length
}

async function controlRewind($, state, control) {
  if (state.busy !== false) throw new Error('Claude Code can rewind only between turns')
  if (state.compacting) throw new Error('another context operation is still running')
  const count = state.checkpoints.turns[control.from]?.before
  const supplied = control.checkpoint
  const checkpoint = supplied?.hashes ?? (count === undefined ? undefined : state.checkpoints.messages.slice(0, count))
  if (checkpoint === undefined) throw new Error('this turn has no retained Claude message checkpoint')
  if (checkpoint.length === 0) throw new Error('Claude Code cannot retain an empty conversation through session.compact')
  state.compacting = true
  state.rewind = { checkpoint, from: control.from ?? null }
  const retainedUntil = supplied?.retained_until ?? state.checkpoints.turns[control.from]?.before_uuid
  if (retainedUntil == null || state.lastRawUuid == null) { state.rewind = null; state.compacting = false; throw new Error('this turn has no observed transcript boundary') }
  const previousTail = state.lastRawUuid
  try {
    const result = await $.session.compact()
    if (result.skip !== undefined) return { kind: 'refused', said: result.skip }
    if (!state.rewind.applied) throw new Error('Claude did not apply the requested message checkpoint')
    const retained = await messageFingerprints(result.messages)
    if (retained.length !== checkpoint.length || !retained.every((hash, index) => checkpoint[index] === hash)) throw new Error('another Claude hook changed the requested replacement prefix')
    state.checkpoints.messages = checkpoint
    state.checkpoints.full = true
    state.checkpoints.uuids = state.checkpoints.uuids?.slice(0, checkpoint.length) ?? checkpoint.map(() => null)
    for (const [turn, point] of Object.entries(state.checkpoints.turns)) {
      if (point.before >= checkpoint.length) delete state.checkpoints.turns[turn]
      else if (point.after > checkpoint.length) delete point.after
    }
    await saveCheckpoints($, state)
    return { kind: 'rewound', from: control.from ?? null, retained_messages: result.messages.length,
      files_restored: false, compacted_turns_reachable: false, retained_until: retainedUntil, previous_tail: previousTail,
      retained: result.messages.map(messageProjection) }
  } finally { state.rewind = null; state.compacting = false }
}

async function compactedCheckpoint($, state, messages, parent) {
  state.checkpoints = { messages: await messageFingerprints(messages), turns: {}, full: true,
    uuids: messages.map(() => null), compacted_parent: parent }
  state.lastUuid = undefined
  state.lastRawUuid = undefined
  await saveCheckpoints($, state)
}

async function controlCompaction($, e, next) {
  if (e.agentId !== undefined) return next(e)
  const state = await controlBegin($, sessions)
  if (state === null) return next(e)
  if (state.rewind === null || e.trigger !== 'plugin') {
    const parent = state.lastRawUuid
    const result = await next(e)
    if (e.trigger !== 'precompute' && result.skip === undefined) {
      await compactedCheckpoint($, state, result.messages, parent)
    }
    return result
  }
  const cut = checkpointCut(await messageFingerprints(e.messages), state.rewind.checkpoint)
  state.rewind.applied = true
  return { messages: e.messages.slice(0, cut) }
}
