// The first sample is taken before this process can make a model request.
const usageOrigins = new Map()

async function usageOrigin($, id) {
  if (!usageOrigins.has(id)) {
    usageOrigins.set(id, { at: new Date().toISOString(), value: sessionUsage($).then((usage) => usage?.cost?.usd) })
  }
  const since = await usageOrigins.get(id).value
  if (typeof since !== 'number' || !Number.isFinite(since) || since < 0) usageOrigins.delete(id)
  return since
}

async function initializeAccounting($, context) {
  const process = JSON.stringify([context.hostProcess.pid, context.hostProcess.token])
  const names = (await $.fs.list(context.dir)).map((entry) => entry.name)
  let origin = null
  if (names.includes('accounting-origin.json')) {
    const kept = JSON.parse(await $.fs.read(context.dir + '/accounting-origin.json'))
    if (kept.process === process) origin = kept
  }
  if (origin === null) {
    const since = await usageOrigin($, context.sessionId)
    if (typeof since !== 'number') {
      context.accountingOrigin = null
      worldFailure($, 'accounting', new Error('Claude did not provide its accounting baseline'))
      return
    }
    origin = { process, since, account_at: usageOrigins.get(context.sessionId).at }
    await $.fs.write(context.dir + '/accounting-origin.json', JSON.stringify(origin))
  }
  context.accountingOrigin = origin
  const receipts = await $.fs.list(context.dir + '/accounting')
  const ready = new Set(receipts.map((entry) => entry.name))
  for (const entry of receipts) {
    if (!entry.name.endsWith('.json') || ready.has(entry.name.slice(0, -5) + '.ready')) continue
    const path = context.dir + '/accounting/' + entry.name.slice(0, -5)
    await readReport($, path)
    await $.fs.write(path + '.ready', '')
  }
  // A complete usage body written before a crash still needs its commit marker.
  const events = await $.fs.list(context.dir + '/events')
  const committed = new Set(events.map((entry) => entry.name))
  for (const entry of events) {
    if (!entry.name.endsWith('.json') || committed.has(entry.name.slice(0, -5) + '.done')) continue
    const path = context.dir + '/events/' + entry.name.slice(0, -5)
    const report = await readReport($, path)
    if (report.kind === 'session' && report.happened?.change?.kind === 'usage' && report.happened.change.usage?.counted) {
      await $.fs.write(path + '.done', '')
    }
  }
}

async function flushAccounting($, context) {
  context.accountingReports ??= new Map()
  for (const [name, body] of context.accountingReports) {
    const path = context.dir + '/accounting/' + name
    await writeReport($, path, body)
    await $.fs.write(path + '.ready', '')
    context.accountingReports.delete(name)
  }
}

function recoverAccounting($, context) {
  const pending = (context.accountingQueue ?? Promise.resolve()).then(() => flushAccounting($, context))
  context.accountingQueue = pending.catch((error) => {
    worldFailure($, 'accounting', error)
    retry($, context)
  })
  return pending
}

function accountingReport($, context, usage, fields = {}) {
  if (context.accountingOrigin === null) return Promise.resolve()
  const counted = { ...usage, counted: { ...context.accountingOrigin, meter: true, ...fields } }
  const stamp = reserved(context)
  const body = JSON.stringify({ kind: 'session', at: stamp.at,
    happened: { event: 'changed', turn: null, change: { kind: 'usage', usage: counted } } })
  context.accountingReports ??= new Map()
  context.accountingReports.set(stamp.id, body)
  // Each session owns these immutable files. The node publishes and acknowledges
  // them, so a plugin reload or a failed event write cannot discard a receipt.
  const path = context.dir + '/accounting/' + stamp.id
  const written = writeReport($, path, body).then(() => $.fs.write(path + '.ready', '')).then(() => {
    context.accountingReports.delete(stamp.id)
  })
  return written.catch((error) => {
    retry($, context)
    throw error
  })
}

// Official collector records carry the host's own request price. Never infer a
// request's dollars from a concurrent session-wide sample or from copied prices.
async function pricedRequest($, context, e) {
  const a = e.attributes
  if (typeof a.model !== 'string' || !Number.isFinite(a.cost_usd) || a.cost_usd < 0) {
    throw new Error('Claude reported an invalid request price')
  }
  const ended = Date.parse(e.loggedAt)
  const duration = Number(a.duration_ms)
  const first = Number(a.ttft_ms)
  const timing = { ended_at: e.loggedAt, approximate: false,
    started_at: Number.isFinite(duration) ? new Date(ended - duration).toISOString() : null,
    first_at: Number.isFinite(duration) && Number.isFinite(first) ? new Date(ended - duration + first).toISOString() : null }
  const source = typeof a.query_source === 'string' && a.query_source.startsWith('agent:') ? 'agent'
    : ['sdk', 'repl_main'].includes(a.query_source) ? 'main' : 'background'
  // The effort is the one the request body carried, absent when none was sent.
  const priced = { key: String(a['event.sequence']), usd: a.cost_usd, model: a.model, timing, source,
    effort: typeof a.effort === 'string' ? a.effort : null, turn: null, agent: null, tokens: { input: Number(a.input_tokens ?? 0), output: Number(a.output_tokens ?? 0),
      cache_read: Number(a.cache_read_tokens ?? 0), cache_write: Number(a.cache_creation_tokens ?? 0), reasoning: 0 } }
  if (a['event.sequence'] === undefined) throw new Error('Claude request has no accounting sequence')
  const key = usageKey(a.input_tokens, a.output_tokens, a.cache_read_tokens, a.cache_creation_tokens)
  const book = pairing(context)
  const at = book.steps.findIndex((one) => one.key === key)
  if (at >= 0) {
    Object.assign(priced, book.steps.splice(at, 1)[0].owner)
    return accountingReport($, context, {}, { priced })
  }
  // Its step has not ended yet: the price waits a moment for it, then is
  // written as it is. Nothing here holds up the request.
  let settled = false
  const take = (owner) => {
    if (settled) return
    settled = true
    Object.assign(priced, owner)
    accountingReport($, context, {}, { priced }).catch((error) => worldFailure($, 'request accounting', error))
  }
  book.prices.push({ key, take })
  $.clock.after(PAIRING_MS, () => {
    const index = book.prices.findIndex((one) => one.take === take)
    if (index >= 0) book.prices.splice(index, 1)
    take({ turn: null, agent: null })
  })
}

// A priced request names no turn or agent; the step that made it does, and
// both carry the usage of the one response. Whichever comes first waits a
// moment for the other.
const PAIRING_MS = 3000

function usageKey(input, output, cacheRead, cacheWrite) {
  return [input, output, cacheRead, cacheWrite].map((n) => Number(n ?? 0)).join(':')
}

function pairing(context) {
  context.pairing ??= { steps: [], prices: [] }
  const now = Date.now()
  context.pairing.steps = context.pairing.steps.filter((one) => now - one.at < PAIRING_MS)
  return context.pairing
}

// A step that ended with usage gives its turn and agent to its price.
function stepPriced(context, step) {
  const u = step.usage
  const key = usageKey(u.input_tokens, u.output_tokens, u.cache_read_input_tokens, u.cache_creation_input_tokens)
  // The step says whose request it was more surely than the request's own source.
  const owner = { turn: step.turn ?? null, agent: step.agent ?? null, source: step.agent ? 'agent' : 'main' }
  const book = pairing(context)
  const at = book.prices.findIndex((one) => one.key === key)
  if (at >= 0) return book.prices.splice(at, 1)[0].take(owner)
  book.steps.push({ key, at: Date.now(), owner })
}

async function stepMeasured($, context, step = null, force = false) {
  try {
    // Failed and held steps have no priced success event, but still have a call row.
    if (step !== null && step.usage === null) {
      const key = JSON.stringify([step.turn, step.agent, step.index])
      await accountingReport($, context, {}, { steps: [step], detail_key: key })
    } else if (step !== null) {
      stepPriced(context, step)
    }
    const usage = await sessionUsage($)
    if (usage === null) throw new Error('Claude did not provide its usage sample')
    await accountingReport($, context, usage)
    if (force && usage.context?.tokens !== undefined) {
      await reports($, context).event('session', { happened: { event: 'context', used: usage.context.tokens, window: usage.context.window } })
    }
    return usage
  } catch (error) {
    worldFailure($, 'accounting', error)
    return null
  }
}

async function heldStep($, context, input) {
  try {
    const hold = JSON.parse(await $.fs.read(context.dir + '/hold.json'))
    if (typeof hold.said !== 'string') return null
    if (hold.until !== null && (!Number.isFinite(Date.parse(hold.until)) || Date.parse(hold.until) <= Date.now())) return null
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
    await noteStepStop($, context, input, result)
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
    // A held step sent nothing; any other names the effort it asked for.
    const effort = !held && typeof input.effort === 'string' ? input.effort : null
    await stepMeasured($, context, { usage: result?.usage ?? null, model: input.model,
      turn: input.turnId, index: input.index, agent: input.agentId ?? null, effort, timing, outcome, phase })
  }
}
