import { presentation } from './presentation.js'

// The World's mod for Claude Code.
//
// It tells the World the session exists and what it does, turn by turn and
// tool by tool, and it hands the session what the World has for it: in a
// new turn at once when the session is idle, beside the next tool result
// when it is busy, and at the end of the turn for whatever is still held
// then.
//
// It talks to the fastctx node only through files in the session's own
// directory, `<world>/claude/<session id>/`:
//   session.json       written when the session starts
//   transcript.json    where Claude Code keeps the session's own record
//   events/<n>.json    a report or an ordered batch of stream events, and
//   events/<n>.done    written after it, once it is whole
//   inbox/<id>.json    what the node has for the session; read, never changed
//   receipts/<id>      written once something was handed to the model
//   stop.json          the turn the node asks to be stopped; read, never changed
// Before a call to the World's own tools goes through, it writes which
// session makes it to `<world>/claude-calls/<call id>.json`; the node takes
// that file when the call arrives, and sweeps those of ended sessions.
// Which turn took what it handed over is an event of its own, before the
// end of that turn: the node owes that turn's end to whoever it was from.
// Controls add $.command.list/run and $.config.list. A public request waits
// in $.process.spawn, released when its node, host, turn or viewers leave.
// MCP ownership is resolved from $.mcp.connect and $.session.usage at startup.
// The inbox address identifies native-shell commands, never MCP calls.

const WORLD = __FASTCTX_WORLD__
const FASTCTX = __FASTCTX_COMMAND__
// Inbox polling cadence; a tool boundary can deliver words sooner.
const LOOK_MS = 2000
// Claude Code limits each fs read/write to 4 MiB of UTF-8.
const FILE_BYTES = 4 * 1024 * 1024

__FASTCTX_CONTROLS__

// /clear may replace even the same session ID. Keep the old object alive for
// its timers, queued writes and late turn/tool results; never reset it in place.
function conversation(sessionId, previous) {
  const context = {
    sessionId, previous, ready: null,
    dir: null,
    closed: false,
    timer: null,
    busy: false,
    turn: null,
    sequence: 0,
    logicalTime: 0,
    hostProcess: null,
    registration: null,
    requestInFlight: false,
    submissionTurns: [],
    // A submission that entered before the turn taking it started.
    awaiting: null,
    handled: new Set(),
    failing: new Set(),
    held: [],
    submitting: [],
    owed: [],
    looking: false,
    stopped: null,
    writing: Promise.resolve(),
    streamBatch: null,
    unwritten: new Map(),
    failedReports: new Set()
  }
  context.shown = presentation(() => reserved(context), () => context.turn)
  return context
}

function chunks(text) {
  const parts = []
  let start = 0, position = 0, bytes = 0
  for (const character of text) {
    const point = character.codePointAt(0)
    const count = point <= 0x7f ? 1 : point <= 0x7ff ? 2 : point <= 0xffff ? 3 : 4
    if (bytes + count > FILE_BYTES) {
      parts.push(text.slice(start, position))
      start = position
      bytes = 0
    }
    bytes += count
    position += character.length
  }
  parts.push(text.slice(start))
  return parts
}

async function writeReport($, path, body) {
  const parts = chunks(body)
  if (parts.length === 1) return $.fs.write(path + '.json', body)
  for (let index = 0; index < parts.length; index++) await $.fs.write(path + '.parts/' + index, parts[index])
  await $.fs.write(path + '.json', JSON.stringify({ parts: parts.length }))
}

async function readReport($, path) {
  let value = JSON.parse(await $.fs.read(path + '.json'))
  if (value.parts !== undefined) {
    let body = ''
    for (let index = 0; index < value.parts; index++) body += await $.fs.read(path + '.parts/' + index)
    value = JSON.parse(body)
  }
  return value
}

function reserved(context) {
  const id = nextName(context)
  // Six sequence digits fill the six sub-millisecond digits of a Rust
  // timestamp. The counter's carry in nextName keeps this monotonic.
  const at = new Date(context.logicalTime).toISOString().replace('Z', String(context.sequence).padStart(6, '0') + 'Z')
  return { id, at }
}

function nextName(context) {
  const now = Date.now()
  if (now > context.logicalTime) { context.logicalTime = now; context.sequence = 0 }
  context.sequence += 1
  // Carry at the six-digit field boundary so lexical order remains causal.
  if (context.sequence === 1000000) { context.logicalTime += 1; context.sequence = 0 }
  return String(context.logicalTime).padStart(15, '0') + '-' + String(context.sequence).padStart(6, '0')
}

function event($, context, kind, fields) {
  if (!context.dir) return context.writing
  context.streamBatch = null
  const { id: name, at } = reserved(context)
  const pending = [...context.unwritten.keys()].map((name) => name + '.json')
  const body = JSON.stringify({ kind, at, ...fields, pending })
  context.unwritten.set(name, { body })
  const written = context.writing.then(() => writePending($, context, name))
  // A failed report fails its caller; it never counts as an acknowledgment.
  context.writing = written.catch(() => { context.failedReports.add(name) })
  return written
}

async function writePending($, context, name) {
  const report = context.unwritten.get(name)
  // A batch's filename predates its last fragment. Keep the allocator's
  // watermark even after acknowledgment deletes the batch, so reload in the
  // same millisecond (or a clock rollback) cannot reuse fragment timestamps.
  if (report.sequence !== undefined) await $.fs.write(context.dir + '/applied/' + report.sequence + '.sequence', '')
  const path = context.dir + '/events/' + name
  await writeReport($, path, report.body)
  await $.fs.write(path + '.done', '')
  context.unwritten.delete(name)
}

// A stream only appends observations in memory. The zero-delay timer starts
// serialization outside the generator; while earlier IO runs, arriving pieces
// share this batch. Other reports seal it so they cannot overtake any piece.
function streamEvent($, context, entry) {
  // A stream can first expose prose after session.end already cancelled the
  // old poller. Its reports still need the same retry and receipt lifecycle.
  if (context.timer === null) context.timer = $.clock.every(LOOK_MS, () => look($, context))
  let batch = context.streamBatch
  if (batch === null) {
    const { id: name } = reserved(context)
    batch = { kind: 'batch', events: [], pending: [...context.unwritten.keys()].map((name) => name + '.json') }
    context.streamBatch = batch
    const report = { body: null, sequence: null }
    context.unwritten.set(name, report)
    const ready = new Promise((resolve) => { $.clock.after(0, resolve) })
    const written = context.writing.then(() => ready).then(async () => {
      if (context.streamBatch === batch) context.streamBatch = null
      report.body = JSON.stringify(batch)
      report.sequence = nextName(context)
      await writePending($, context, name)
    })
    // Keep the batch for the session's retry loop; failure must not reject or
    // cancel the host stream, which has already received the same pieces.
    context.writing = written.catch(() => { context.failedReports.add(name) })
  }
  batch.events.push({ ...entry, at: reserved(context).at })
}

// The scanner follows $ only through this file's top-level functions. Give
// presentation callbacks bound to this dispatch and conversation, never $.
function reports($, context) {
  return {
    event: (kind, fields) => event($, context, kind, fields),
    stream: context.observe,
    stored: context.stored,
    save: (state) => {
      context.streamBatch = null
      const body = JSON.stringify({ process: context.hostProcess, ...state })
      const saved = context.writing.then(() => writeReport($, context.dir + '/presentation', body))
      context.writing = saved.catch(() => {})
      return saved
    },
  }
}

function stamp() {
  return new Date().toISOString()
}

function activity($, context) {
  context.streamBatch = null
  const body = JSON.stringify({ process: context.hostProcess, turn: context.busy ? context.turn : null })
  const saved = context.writing.then(() => $.fs.write(context.dir + '/activity.json', body))
  context.writing = saved.catch(() => {})
  return saved
}

// A receipt tells the node the model has it. One that cannot be written is
// written at the next look; meanwhile it is not handed over again.
async function receipt($, context, delivery, how, acceptedTurn = context.turn) {
  try {
    await $.fs.write(context.dir + '/receipts/' + delivery.id, JSON.stringify({ at: stamp(), how, turn: acceptedTurn }))
  } catch (error) {
    context.owed.push([delivery, how, acceptedTurn])
  }
}

async function settle($, context) {
  for (const [delivery, how, acceptedTurn] of context.owed.splice(0, context.owed.length)) await receipt($, context, delivery, how, acceptedTurn)
}

// What the model reads when the World speaks on its own account. The user's
// own words go in as they are, as the user's.
function framed(delivery) {
  return 'A message from fastctx, not typed by the user:\n\n' + delivery.text
}

// Says which turn took what was handed over.
function handed($, context, deliveries, turn, how) {
  return event($, context, 'handed', { deliveries: deliveries.map((delivery) => delivery.id), turn, how })
}

// Submits words as a new turn; the turn that takes them is told when it
// starts. Returns whether they went.
async function submit($, context, deliveries, prompt) {
  if (context.closed || context.submitting.length > 0) return false
  context.submitting.push(...deliveries)
  if ((await $.session.id()) !== context.sessionId) {
    context.submitting.splice(0, context.submitting.length)
    return false
  }
  for (const delivery of deliveries) {
    await $.fs.write(context.dir + '/attempts/' + delivery.id, JSON.stringify({ at: stamp(), turn: context.turn, state: 'submitting' }))
  }
  try {
    context.requestInFlight = true
    context.submissionTurns.splice(0, context.submissionTurns.length)
    const result = await $.prompt.submit(prompt)
    context.requestInFlight = false
    if (result.drop !== undefined) throw new Error(result.drop)
    const candidates = context.submissionTurns.filter((started) => took(started.text, result.text))
    if (candidates.length === 0) {
      // The prompt entered; the turn it starts begins once the session is
      // idle, which may be only after this very hook returns (a submission at
      // the end of a turn). The next turn with its text is the one that took
      // it (`startTurn`); meanwhile nothing else is submitted.
      context.awaiting = { text: result.text, deliveries }
      return true
    }
    context.submitting.splice(0, context.submitting.length)
    if (candidates.length !== 1) {
      await uncertain($, context, deliveries.map((delivery) => delivery.id), 'the host did not identify one consuming turn for this submission')
      return true
    }
    await submitted($, context, deliveries, candidates[0].turnId)
    return true
  } catch (error) {
    context.requestInFlight = false
    if (!deliveries.some((delivery) => context.submitting.includes(delivery))) throw error
    for (const delivery of deliveries) {
      const at = context.submitting.indexOf(delivery)
      if (at >= 0) context.submitting.splice(at, 1)
    }
    for (const delivery of deliveries) await $.fs.write(context.dir + '/attempts/' + delivery.id, JSON.stringify({ state: 'refused' }))
    await trouble($, context, deliveries[0].id, 'it could not be handed over: ' + error)
    return false
  }
}

// Says that `acceptedTurn` took what was submitted.
async function submitted($, context, deliveries, acceptedTurn) {
  const report = handed($, context, deliveries, acceptedTurn, 'submitted')
  await Promise.all([report, ...deliveries.map((delivery) => receipt($, context, delivery, 'submitted', acceptedTurn))])
}

// Hands everything held to the model in one new turn; on failure it is
// held again, before anything held since.
async function flush($, context, how) {
  if (context.closed || context.busy || context.submitting.length > 0 || context.held.length === 0) return
  const deliveries = [context.held.shift()]
  if (deliveries[0].as_user !== true) {
    while (context.held.length > 0 && context.held[0].as_user !== true) deliveries.push(context.held.shift())
  }
  const prompt = deliveries[0].as_user === true
    ? { text: deliveries[0].text, asUser: true }
    : { text: deliveries.map(framed).join('\n\n') }
  if (!(await submit($, context, deliveries, prompt))) context.held.unshift(...deliveries)
}

// The names in one of the session's directories, or null when they cannot
// be listed.
async function listing($, context, sub) {
  try {
    return (await $.fs.list(context.dir + '/' + sub)).map((entry) => entry.name)
  } catch (error) {
    return null
  }
}

// Tells the World, once, that an inbox file could not be handed over yet;
// it is tried again at every look until it goes through.
async function uncertain($, context, ids, text) {
  const key = 'uncertain:' + ids.join(',')
  if (context.failing.has(key)) return
  context.failing.add(key)
  await event($, context, 'uncertain', { deliveries: ids, text })
}

async function trouble($, context, name, text) {
  if (context.failing.has(name)) return
  context.failing.add(name)
  await event($, context, 'trouble', { file: name, text })
}

async function look($, context) {
  const current = !context.closed && (await $.session.id()) === context.sessionId
  if (current) await stopping($, context)
  if (context.looking) return
  context.looking = true
  try {
    await settle($, context)
    const retry = context.writing.then(async () => {
      for (const name of context.failedReports) {
        await writePending($, context, name)
        context.failedReports.delete(name)
      }
    })
    context.writing = retry.catch(() => {})
    await retry
    if (context.closed && context.timer && context.owed.length === 0 && context.unwritten.size === 0) {
      context.timer.cancel()
      context.timer = null
    }
    if (context.closed || !current) return
    await looked($, context)
  } finally {
    context.looking = false
  }
}

// Stops the running turn when the node asks for it. Only the turn that is
// running can be stopped, and only by its id; a request for a turn that has
// ended is left as it is.
async function stopping($, context) {
  let asked = null
  try {
    asked = JSON.parse(await $.fs.read(context.dir + '/stop.json')).turn
  } catch (error) {
    return
  }
  if (!context.busy || asked !== context.turn) return false
  if (asked === context.stopped) return true
  context.stopped = asked
  try {
    await $.turn.abort({ turnId: asked })
  } catch (error) {
    await event($, context, 'stop_failed', { turn: asked, text: String(error) })
  }
  return true
}

async function looked($, context) {
  await settle($, context)
  // What has a receipt was handed over already, perhaps before this mod
  // loaded again. Without knowing which, anything handed over could repeat.
  const receipts = await listing($, context, 'receipts')
  if (receipts === null) return
  if (!context.busy) await flush($, context, 'submitted')
  const receipted = new Set(receipts)
  // Node acknowledgment writes receipts before retiring handed reports.
  // Scan both, then receipts again, so that transition cannot fall between
  // two snapshots taken while this mod is reloading.
  for (const name of (await listing($, context, 'events')) || []) {
    if (!name.endsWith('.json')) continue
    try {
      const report = await readReport($, context.dir + '/events/' + name.slice(0, -5))
      if (report.kind === 'handed') for (const id of report.deliveries) receipted.add(id)
    } catch (error) {
      const latest = await listing($, context, 'receipts')
      if (latest === null) throw error
      for (const id of latest) receipted.add(id)
    }
  }
  for (const id of (await listing($, context, 'receipts')) || []) receipted.add(id)
  const attempts = new Set((await listing($, context, 'attempts')) || [])
  const fresh = ((await listing($, context, 'inbox')) || [])
    .filter((name) => name.endsWith('.json') && !context.handled.has(name) && !receipted.has(name.slice(0, -5)))
    .sort()
  for (const name of fresh) {
    if (attempts.has(name.slice(0, -5)) && JSON.parse(await $.fs.read(context.dir + '/attempts/' + name.slice(0, -5))).state !== 'refused') {
      await uncertain($, context, [name.slice(0, -5)], 'a prior mod stopped during submission; acceptance is unknown')
      continue
    }
    let delivery = null
    try {
      delivery = JSON.parse(await $.fs.read(context.dir + '/inbox/' + name))
      if (delivery.text_parts !== undefined) {
        delivery.text = ''
        for (let index = 0; index < delivery.text_parts; index++) {
          delivery.text += await $.fs.read(context.dir + '/inbox-parts/' + delivery.id + '/' + index)
        }
      }
    } catch (error) {
      await trouble($, context, name, 'it could not be read: ' + error)
      continue
    }
    if (context.busy || context.submitting.length > 0) {
      context.held.push(delivery)
      context.handled.add(name)
      continue
    }
    // Taken before the session is asked, so no other look takes it too.
    context.handled.add(name)
    // The user's own words, typed on one of the user's machines: only these
    // go in as the user.
    const prompt = delivery.as_user === true ? { text: delivery.text, asUser: true } : { text: framed(delivery) }
    if (!(await submit($, context, [delivery], prompt))) {
      context.handled.delete(name)
      continue
    }
    context.failing.delete(name)
    // The turn.start callback writes the receipt together with handed.
    // Successful submission alone does not identify a consuming turn.
  }
}

async function startSession($, context) {
  const id = context.sessionId
  context.dir = WORLD + '/claude/' + id
  // A reload can happen in the same clock tick, with old reports still
  // pending. Their reserved names survive in events or applied.
  for (const sub of ['events', 'applied']) {
    await $.fs.write(context.dir + '/' + sub + '/.created', '')
    const names = await listing($, context, sub)
    if (names === null) throw new Error('cannot recover the report sequence')
    for (const name of names) {
      const match = /^(\d+)-(?:end-)?(\d+)\./.exec(name)
      if (!match) continue
      const time = Number(match[1]), count = Number(match[2])
      if (time > context.logicalTime) { context.logicalTime = time; context.sequence = count }
      else if (time === context.logicalTime) context.sequence = Math.max(context.sequence, count)
    }
  }
  const instance = reserved(context)
  // A session can be resumed in the same process, even in the same clock
  // tick. Preserve the registration's generation independently of reports.
  await $.fs.write(context.dir + '/applied/' + instance.id + '.sequence', '')
  const registration = { session: id, previous: context.previous, at: instance.at, instance: instance.id }
  const parent = await $.process.run([FASTCTX, 'parent-process'])
  if (parent.exitCode !== 0) throw new Error('identifying the Claude host: ' + parent.stderr)
  registration.process = JSON.parse(parent.stdout)
  context.hostProcess = registration.process
  const files = (await $.fs.list(context.dir)).map((entry) => entry.name)
  if (files.includes('activity.json')) {
    const previous = JSON.parse(await $.fs.read(context.dir + '/activity.json'))
    if (previous.process && previous.process.pid === context.hostProcess.pid && previous.process.token === context.hostProcess.token) {
      context.turn = previous.turn
      context.busy = context.turn !== null
    }
  }
  if (files.includes('presentation.json')) {
    const state = await readReport($, context.dir + '/presentation')
    if (state.process.pid !== context.hostProcess.pid || state.process.token !== context.hostProcess.token) {
      state.tools = []
      state.requests = []
    }
    context.shown.restore(state)
  }
  const ask = async (name, get) => {
    try {
      registration[name] = await get()
    } catch (error) {
      registration[name] = null
    }
  }
  // Native-shell commands can carry this address. MCP connections need the
  // per-call proof below because their environment is a startup snapshot.
  await ask('inbox_address', () => $.env.get('CLAUDE_CODE_MESSAGING_SOCKET'))
  await ask('entrypoint', () => $.env.get('CLAUDE_CODE_ENTRYPOINT'))
  await ask('cwd', () => $.session.cwd())
  await ask('model', () => $.session.model())
  await ask('surfaces', () => $.session.surfaces())
  await ask('version', () => $.session.version())
  // The receipts directory exists from the start, so failing to list it
  // means a failure, never "nothing handed over yet".
  await $.fs.write(context.dir + '/receipts/.created', '')
  await $.fs.write(context.dir + '/session.json', JSON.stringify(registration))
  context.registration = registration
  // What already waits when the session starts again is handed over, not
  // skipped: a closed session's answers are what it should see on return.
  context.timer = $.clock.every(LOOK_MS, () => look($, context))
  // Background observations retain this conversation even after /clear.
  context.observe = (happened) => streamEvent($, context, { kind: 'session', happened })
  // Claude Code keeps a record before its file has it; the node waits for
  // the last one kept when it reads the file.
  context.stored = (uuid) => streamEvent($, context, { kind: 'stored', uuid })
}

// Whether a turn starting with `started` is the one that took the prompt
// whose text entered as `entered`. The person's own words start their turn
// bare; anything else a plugin submits starts it inside the host's frame,
// "The <plugin> plugin sent a message:" before and a note after (2.1.289).
function took(started, entered) {
  return started === entered || (entered !== '' && started.includes(entered))
}

async function startTurn($, context, e, next) {
  await startControlTurn($, e)
  context.busy = true
  context.turn = e.turnId
  if (context.requestInFlight) context.submissionTurns.push({ turnId: e.turnId, text: e.text })
  const awaited = context.awaiting !== null && took(e.text, context.awaiting.text) ? context.awaiting : null
  if (awaited !== null) {
    context.awaiting = null
    context.submitting.splice(0, context.submitting.length)
  }
  const started = event($, context, 'turn_start', { turn: context.turn })
  const saved = activity($, context)
  await Promise.all([started, saved])
  if (awaited !== null) await submitted($, context, awaited.deliveries, e.turnId)
  return next(e)
}

async function completeTurn($, context, e, next) {
  await completeControlTurn($, e)
  if (e.agentId !== undefined) return next(e)
  await context.shown.end(reports($, context), e.turnId)
  if (context.turn === e.turnId) { context.busy = false; context.turn = null }
  const completed = event($, context, 'turn_complete', {
    turn: e.turnId,
    aborted: e.isAborted,
    reason: e.reason,
    refusal: e.reason === 'refusal' ? e.refusal : null,
    ms: e.durationMs,
    answer: e.answer ?? null,
    streamed: true,
  })
  const saved = activity($, context)
  await Promise.all([completed, saved])
  await flush($, context, 'submitted at the end of a turn')
  return next(e)
}

// Both successful and failed main-tool boundaries can append World facts.
// Appending preserves the text without the classic context-size rewrite.
async function boundary($, context, e, next) {
  if (e.agent_id !== undefined) return next(e)
  const result = await next(e)
  if (context.closed || (await $.session.id()) !== context.sessionId) return result
  if (await stopping($, context)) return result
  const acceptedTurn = context.turn
  if (!context.busy || acceptedTurn === null) return result
  const deliveries = context.held.filter((delivery) => delivery.as_user !== true)
  if (deliveries.length === 0) return result
  const selected = new Set(deliveries)
  for (let i = context.held.length - 1; i >= 0; i--) if (selected.has(context.held[i])) context.held.splice(i, 1)
  for (const delivery of deliveries) await $.fs.write(context.dir + '/attempts/' + delivery.id, JSON.stringify({ state: 'appending', turn: acceptedTurn }))
  try {
    const appended = await $.session.append({ message: { type: 'user', content: [{ type: 'text', text: deliveries.map(framed).join('\n\n') }] } })
    if (appended.deny !== undefined) {
      for (const delivery of deliveries) await $.fs.write(context.dir + '/attempts/' + delivery.id, JSON.stringify({ state: 'refused' }))
      context.held.unshift(...deliveries)
      await trouble($, context, deliveries[0].id, appended.deny)
      return result
    }
    for (const delivery of deliveries) await receipt($, context, delivery, 'appended at a main tool boundary', acceptedTurn)
    await handed($, context, deliveries, acceptedTurn, 'appended at a main tool boundary')
  } catch (error) {
    // append may already have crossed the host boundary: retain its
    // evidence rather than replaying an unconfirmed external effect.
    await trouble($, context, deliveries[0].id, 'append acceptance needs reconciliation: ' + error)
    throw error
  }
  return result
}

async function endSession($, context, e, next) {
  await endControls($, e)
  context.closed = true
  // The node may restart while this process runs another conversation.
  // Process liveness alone would otherwise resurrect this closed one.
  await $.fs.write(context.dir + '/closed/' + context.registration.instance, '')
  context.streamBatch = null
  if (context.awaiting !== null) {
    const { deliveries } = context.awaiting
    context.awaiting = null
    context.submitting.splice(0, context.submitting.length)
    await uncertain($, context, deliveries.map((delivery) => delivery.id), 'the session ended before a turn took what was submitted')
  }
  const { id: name, at } = reserved(context)
  const body = JSON.stringify({ kind: 'session_end', at, instance: context.registration.instance, reason: e.reason, pending: [...context.unwritten.keys()].map((name) => name + '.json') })
  context.unwritten.set(name, { body })
  try {
    await writePending($, context, name)
    if (context.timer && context.owed.length === 0 && context.unwritten.size === 0) {
      context.timer.cancel()
      context.timer = null
    }
  } catch (error) {
    context.failedReports.add(name)
    throw error
  }
  return next(e)
}

// Where Claude Code keeps this session's own record, as its hooks are told
// (`transcript_path`): the World reads the session's past there. Without
// this file the node says it does not know where that record is.
async function transcript($, context, path) {
  if (typeof path !== 'string' || path === '' || !context.dir) return
  try {
    await $.fs.write(context.dir + '/transcript.json', JSON.stringify({ path }))
  } catch (error) {
    // A session start must not fail for the World's sake.
  }
}

async function session($, registry, id, renew = false) {
  let context = registry.sessions.get(id)
  if (!context || (renew && context.closed)) {
    context = conversation(id, registry.cleared)
    // Session switches keep their order even within one clock tick or when
    // the wall clock moves backwards. The node uses registration time to
    // keep late reports from moving the resident's delivery address back.
    for (const prior of registry.sessions.values()) {
      if (prior.logicalTime > context.logicalTime) {
        context.logicalTime = prior.logicalTime
        context.sequence = prior.sequence
      } else if (prior.logicalTime === context.logicalTime) {
        context.sequence = Math.max(context.sequence, prior.sequence)
      }
    }
    registry.cleared = null
    registry.sessions.set(id, context)
    context.ready = startSession($, context)
  }
  await context.ready
  return context
}

async function route($, registry, name, e) {
  const id = name === 'session.end' ? e.sessionId : await $.session.id()
  const resultId = name === 'session.append' && e.agentId === undefined
    ? e.message.content.find((block) => block.type === 'tool_result')?.tool_use_id : undefined
  const context = (name === 'turn.complete' && registry.turns.get(e.turnId))
    || (resultId !== undefined && registry.toolOwners.get(resultId)) || await session($, registry, id)
  if (name === 'turn.start') registry.turns.set(e.turnId, context)
  if (name === 'tool.call' && e.agentId === undefined) registry.toolOwners.set(e.tool_use_id, context)
  return context
}

export function register(on) {
  const registry = { sessions: new Map(), turns: new Map(), toolOwners: new Map(), cleared: null, worldTools: new Set() }
  on('session.start', async ($, e, next) => {
    await startControls($)
    await worldTools($, registry)
    await session($, registry, await $.session.id(), true)
    return next(e)
  })
  on('turn.start', async ($, e, next) => {
    return startTurn($, await route($, registry, 'turn.start', e), e, next)
  })
  on('turn.complete', async ($, e, next) => {
    return completeTurn($, await route($, registry, 'turn.complete', e), e, next)
  })
  on('turn.step', async function* ($, e, next) {
    const context = registry.turns.get(e.turnId) || await session($, registry, await $.session.id())
    return yield* context.shown.step(reports($, context), e, next)
  })
  on('tool.call', async ($, e, next) => {
    const context = await route($, registry, 'tool.call', e)
    if (registry.worldTools.has(e.tool)) {
      try {
        await proveCall($, context, e.tool_use_id)
      } catch (error) {
        return { deny: 'The fastctx plugin could not tell fastctx which session made this call, so it did not run: ' + error }
      }
    }
    return context.shown.call(reports($, context), e, (input) => controlCall($, input, next))
  })
  on('classic.PermissionRequest', async ($, e, next) => controlPermission($, e, next))
  on('session.append', async ($, e, next) => {
    const context = await route($, registry, 'session.append', e)
    return context.shown.append(reports($, context), e, next)
  })
  on('session.compact', async ($, e, next) => {
    const context = await route($, registry, 'session.compact', e)
    return context.shown.compact(reports($, context), e, next)
  })
  on('session.measure', async ($, e, next) => {
    const context = await route($, registry, 'session.measure', e)
    return context.shown.measure(reports($, context), e, next)
  })
  on('classic.PostToolUse', async ($, e, next) => {
    return boundary($, await route($, registry, 'classic.PostToolUse', e), e, next)
  })
  on('classic.PostToolUseFailure', async ($, e, next) => {
    return boundary($, await route($, registry, 'classic.PostToolUseFailure', e), e, next)
  })
  on('session.end', async ($, e, next) => {
    registry.cleared = e.reason === 'clear' ? e.sessionId : null
    return endSession($, await route($, registry, 'session.end', e), e, next)
  })
  on('classic.SessionStart', async ($, e, next) => {
    await startControls($)
    await worldTools($, registry)
    const context = await session($, registry, await $.session.id(), e.source === 'clear' || e.source === 'resume')
    await transcript($, context, e.transcript_path)
    return next(e)
  })
}

// The file is whole before the call leaves the host, so the node reads it
// whole. Its name spells each UTF-16 unit of the id in four hex digits.
async function proveCall($, context, id) {
  if (typeof id !== 'string' || id.length === 0) throw new Error('the call has no tool_use_id to identify it by')
  let name = ''
  for (let i = 0; i < id.length; i++) name += id.charCodeAt(i).toString(16).padStart(4, '0')
  await $.fs.write(WORLD + '/claude-calls/' + name + '.json', JSON.stringify({ tool_use_id: id, registration: context.registration }))
}

async function worldTools($, registry) {
  registry.worldTools = new Set()
  const connected = await $.mcp.connect('fastctx')
  if (!connected.isConnected) return
  // connect answers the server's name as /mcp lists it, which for a
  // deduplicated server is the name it runs under; a tool carries the
  // name's tool-name spelling, every character outside [A-Za-z0-9_-] made
  // `_` (`claude.ai Gmail`, `claude_ai_Gmail` in the mod reference; 2.1.289
  // lists `plugin:fastctx-world:fastctx` and `plugin_fastctx-world_fastctx`).
  const spelled = connected.server.replace(/[^A-Za-z0-9_-]/g, '_')
  const usage = await $.session.usage({ breakdown: 'summary' })
  if (usage.context.breakdown === undefined) throw new Error('the host did not provide its MCP tool ownership map')
  const owners = new Map()
  for (const tool of usage.context.breakdown.mcpTools) {
    if (owners.has(tool.name) && owners.get(tool.name) !== tool.serverName) owners.set(tool.name, null)
    else if (!owners.has(tool.name)) owners.set(tool.name, tool.serverName)
  }
  registry.worldTools = new Set([...owners].filter(([, server]) => server === connected.server || server === spelled).map(([name]) => name))
}
