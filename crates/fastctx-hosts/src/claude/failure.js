// Catch's next replays an already settled native result; it never runs the
// user's operation twice. Re-entry permits next, but rejects every $ call.
function worldFailure($, name, error) {
  worldNotice($, 'fastctx could not complete its ' + name + ' hook: ' + String(error?.message ?? error))
}

// A World hook that failed is told to Claude Code's debug log alone: the
// World adds nothing to Claude Code's own interface (`world.md`), and a
// World tool that cannot run is refused with the reason, which the model
// reads.
function worldNotice($, text) {
  try {
    $.ui.log(text, { to: 'debug' })
  } catch (_) {
    // Reporting cannot become another dependency of the native operation.
  }
}

async function continueWithoutWorld($, name, e, next) {
  if (next.error.kind === 're-entry') return next(e)
  if (next.called) {
    // A rejected native operation remains its own failure, without a World
    // diagnosis. A successful replay identifies a failed observation after it.
    const result = await next(e)
    worldFailure($, name, next.error)
    return result
  }
  worldFailure($, name, next.error)
  return next(e)
}

async function* streamWithoutWorld($, name, e, next) {
  if (!next.called && next.error.kind !== 're-entry') worldFailure($, name, next.error)
  const stream = next(e)
  for await (const piece of stream) yield piece
  const result = await stream.result
  if (next.called && next.error.kind !== 're-entry') worldFailure($, name, next.error)
  return result
}

function unidentifiedWorldCall(error) {
  return { deny: 'The fastctx plugin could not tell fastctx which session made this call, so it did not run: ' + error }
}

async function continueToolWithoutWorld($, registry, e, next) {
  if (next.error.kind === 're-entry') return next(e)
  // An unexecuted World call still needs its identity proof. This restriction
  // belongs to World tools only; native and other plugins' tools pass through.
  try {
    if (next.called) {
      const result = await next(e)
      worldFailure($, 'tool.call', next.error)
      return result
    }
    worldFailure($, 'tool.call', next.error)
    if (!next.called && registry.worldTools.has(e.tool) && !registry.provedCalls.has(e.tool_use_id)) return unidentifiedWorldCall(next.error.message)
    return await next(e)
  } finally { registry.provedCalls.delete(e.tool_use_id) }
}
