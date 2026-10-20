// Process-local origins survive failed World registration and context replacement.
const usageOrigins = new Map()

async function usageOrigin($, id) {
  if (!usageOrigins.has(id)) {
    usageOrigins.set(id, sessionUsage($).then((usage) => usage?.cost?.usd))
  }
  const since = await usageOrigins.get(id)
  if (typeof since !== 'number' || !Number.isFinite(since) || since < 0) usageOrigins.delete(id)
  return since
}

// All reporting paths share one queue: take finished requests before sampling
// the cost, then persist that sample before another path can sample it again.
async function stepMeasured($, context, step = null, force = false) {
  context.usageSteps ??= []
  if (step !== null) context.usageSteps.push(step)
  const pending = (context.usageQueue ?? Promise.resolve()).then(async () => {
    const steps = context.usageSteps.splice(0)
    const usage = await sessionUsage($)
    const total = usage?.cost?.usd
    const since = await usageOrigin($, context.sessionId)
    if (typeof total !== 'number' || typeof since !== 'number' || context.hostProcess === null) {
      context.usageSteps.unshift(...steps)
      return usage
    }
    if (!force && total === context.measuredUsd && steps.length === 0) return usage
    const process = JSON.stringify([context.hostProcess.pid, context.hostProcess.token])
    const counted = { ...usage, counted: { process, since, steps } }
    // event() owns retry after accepting a report, including its request list.
    await reports($, context).event('session', { happened: { event: 'changed', turn: null, change: { kind: 'usage', usage: counted } } })
    context.measuredUsd = total
    if (force && usage.context?.tokens !== undefined) {
      await reports($, context).event('session', { happened: { event: 'context', used: usage.context.tokens, window: usage.context.window } })
    }
    return usage
  })
  context.usageQueue = pending.catch((error) => {
    $.ui.log('World step usage: ' + String(error), { to: 'debug' })
  })
  // 2026-10-20: reporting must not fail or replay a model request that already ended.
  return pending.catch(() => null)
}

async function heldStep($, context, input) {
  try {
    const hold = JSON.parse(await $.fs.read(context.dir + '/hold.json'))
    if (typeof hold.said !== 'string' || !Number.isFinite(Date.parse(hold.until)) || Date.parse(hold.until) <= Date.now()) return null
    if (hold.models !== null && (!Array.isArray(hold.models) || !hold.models.includes('fable') || !/fable|mythos/i.test(input.model))) return null
    return () => {
      const result = { turnId: input.turnId, index: input.index, answer: hold.said, toolUses: [], stopReason: 'end_turn', usage: null }
      const stream = (async function* () {
        yield { kind: 'text', index: 0, text: hold.said }
        yield { kind: 'stop', stopReason: 'end_turn', usage: null }
        return result
      })()
      stream.result = Promise.resolve(result)
      return stream
    }
  } catch {
    return null
  }
}

// Only timing and accounting facts are retained; chunks pass through unchanged.
async function* measuredStep($, context, shown, reported, input, next, held) {
  const timing = { started_at: new Date().toISOString(), first_at: null, ended_at: null, approximate: false }
  let result = null
  let outcome = 'interrupted'
  let phase = 'before_response'
  const stream = shown.step(reported, input, next)
  try {
    for (;;) {
      const part = await stream.next()
      if (part.done) { result = part.value; break }
      const response = part.value.kind !== 'engine' && (part.value.kind !== 'stop' || part.value.stopReason != null)
      if (!held && response && timing.first_at === null) timing.first_at = new Date().toISOString()
      if (!held && response) phase = 'streaming'
      yield part.value
    }
    outcome = held ? 'held' : next.signal?.aborted ? 'interrupted' : result?.stopReason == null ? 'failed' : 'completed'
    if (held) phase = 'before_response'
    else if (outcome === 'completed') phase = 'finished'
    return result
  } catch (error) {
    outcome = error?.name === 'AbortError' ? 'interrupted' : 'failed'
    throw error
  } finally {
    try { await stream.return() } catch { /* The original stream error remains authoritative. */ }
    timing.ended_at = new Date().toISOString()
    await stepMeasured($, context, { usage: result?.usage ?? null, model: input.model,
      turn: input.turnId, agent: input.agentId ?? null, timing, outcome, phase })
  }
}
