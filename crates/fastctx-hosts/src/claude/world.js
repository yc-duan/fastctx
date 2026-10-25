import { presentation, observedMilliseconds } from './presentation.js'

// The World's mod for Claude Code.
//
// It tells the World the session exists and what it does, turn by turn and
// tool by tool, and it hands the session what the World has for it: in a
// new turn at once when the session is idle, beside the next tool result
// when it is busy, and at the end of the turn for whatever is still held
// then.
//
// It talks to the fastctx node only through files in the session's own
// directory, reserved by the node before its first write:
//   session.json       written when the session starts
//   transcript.json    where Claude Code keeps the session's own record
//   events/<n>.json    a report or an ordered batch of stream events, and
//   events/<n>.done    written after it, once it is whole
//   inbox/<id>.json    what the node has for the session; read, never changed
//   receipts/<id>      written once something was handed to the model
//   stop.json          the turn the node asks to be stopped; read, never changed
//   focus.md           the machines and folders the user pointed the
//                      conversation at; read, never changed
//   project.md         the record of the conversation's project, with
//                      the user's instructions for it; read, never changed
// Before a call to the World's own tools goes through, it writes which
// session makes it to `calls/<call id>.json` there; the node takes
// that file when the call arrives, and sweeps those of ended sessions.
// Which turn took what it handed over is an event of its own, before the
// end of that turn: the node owes that turn's end to whoever it was from.
// Controls add $.command.list/run and $.config.list. A public request waits
// in $.process.spawn, released when its node, host, turn or viewers leave.
// MCP ownership is resolved from $.mcp.connect and $.session.usage at startup.
// The inbox address identifies native-shell commands, never MCP calls.
//
// Nothing here runs on a clock while all is well: the session's control
// helper says `look` whenever the node leaves the session something (words,
// a stop, a control), and turn boundaries look on their own. A clock runs
// only to try again what failed.

const WORLD = __FASTCTX_WORLD__
const FASTCTX = __FASTCTX_COMMAND__
// How soon what failed is tried again, and how long an answer to whether
// fastctx is being removed is kept.
const LOOK_MS = __FASTCTX_LOOK_MS__

// Claude Code limits each fs read/write to 4 MiB of UTF-8.
const FILE_BYTES = 4 * 1024 * 1024
// How long a session whose start failed waits before an event tries again.
const START_AGAIN_MS = 5000
// fastctx's root holds the World's directory and, from the moment a removal
// starts, its `uninstalled.json`.
const ROOT = WORLD.slice(0, WORLD.lastIndexOf('/'))
const gone = { seen: false, checked: null }

// Whether fastctx is being removed from this computer. From then on the mod
// stands aside for the rest of this process: it writes nothing, starts no
// helper and passes every event on untouched, so an open conversation neither
// puts files back into the directory being removed nor holds a program the
// removal must delete. A cached answer is at most LOOK_MS old; the removal
// waits two looks after writing its marker before it takes any file. Only a
// listing that shows the marker counts: a listing that fails proves nothing.
async function removed($, fresh = false) {
  if (gone.seen) return true
  const now = Date.now()
  if (!fresh && gone.checked !== null && Math.abs(now - gone.checked) < LOOK_MS) return false
  gone.checked = now
  try {
    gone.seen = (await $.fs.list(ROOT)).some((entry) => entry.name === 'uninstalled.json')
  } catch (error) {
    return false
  }
  return gone.seen
}

__FASTCTX_CONTROLS__
__FASTCTX_COMMANDS__
__FASTCTX_NATIVE_GOAL__
__FASTCTX_ASSISTED__
__FASTCTX_ACCOUNT_AGENTS__
__FASTCTX_SELECTION__
__FASTCTX_REWIND__
__FASTCTX_OBSERVATIONS__
__FASTCTX_USAGE__
__FASTCTX_FAILURES__
__FASTCTX_TURN_FAILURES__
__FASTCTX_GOAL__
__FASTCTX_GOAL_QUESTIONS__

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
    // Submissions Claude Code took, oldest first, each waiting for its turn.
    accepted: [],
    // Rows of this plugin's prompts kept since the last turn started.
    entered: 0,
    // Set while this plugin appends a row of its own at a tool boundary.
    appending: false,
    handled: new Set(),
    failing: new Set(),
    held: [],
    // What rode in beside a prompt that has not started its turn yet.
    alongTaken: [],
    submitting: [],
    owed: [],
    looking: false,
    // A look was asked for while one ran; it runs again before it ends.
    again: false,
    // Something a later look can settle failed in this one.
    unsettled: false,
    stopped: null,
    // The user stopped the last turn, which the next one is to hear.
    toldStopped: [],
    stopTaken: [],
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
  const goalStop = kind === 'turn_complete' && (fields.aborted === true || fields.reason === 'aborted') && context.goal?.active === true
  if (goalStop) {
    context.goalStopping = true
    context.goalStopReport = name + '.json'
  }
  context.unwritten.set(name, { body, goalStop })
  const written = context.writing.then(() => writePending($, context, name))
  // A failed report fails its caller; it never counts as an acknowledgment.
  context.writing = written.catch(() => {
    context.failedReports.add(name)
    retry($, context)
  })
  return written
}

async function writePending($, context, name) {
  const report = context.unwritten.get(name)
  // A batch's filename predates its last fragment. Keep the allocator's
  // watermark even after acknowledgment deletes the batch, so reload in the
  // same millisecond (or a clock rollback) cannot reuse fragment timestamps.
  if (report.sequence !== undefined) await $.fs.write(context.dir + '/applied/' + report.sequence + '.sequence', '')
  const path = context.dir + '/events/' + name
  if (report.goalStop) await $.fs.write(context.dir + '/goal-stop.json', JSON.stringify({ report: name + '.json' }))
  await writeReport($, path, report.body)
  await $.fs.write(path + '.done', '')
  context.unwritten.delete(name)
}

// A stream only appends observations in memory. The zero-delay timer starts
// serialization outside the generator; while earlier IO runs, arriving pieces
// share this batch. Other reports seal it so they cannot overtake any piece.
function streamEvent($, context, entry) {
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
    // Keep the batch to be written again; failure must not reject or cancel
    // the host stream, which has already received the same pieces. A stream
    // can expose prose even after session.end.
    context.writing = written.catch(() => {
      context.failedReports.add(name)
      retry($, context)
    })
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
    thought: context.thought,
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
// written at a later look; meanwhile it is not handed over again.
async function receipt($, context, delivery, how, acceptedTurn = context.turn) {
  try {
    await $.fs.write(context.dir + '/receipts/' + delivery.id, JSON.stringify({ at: stamp(), how, turn: acceptedTurn }))
  } catch (error) {
    context.owed.push([delivery, how, acceptedTurn])
    retry($, context)
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

// The user's own words, sent into a turn already running: the model reads
// them beside the tool's result in the words Claude Code itself gives a
// prompt typed while a turn runs (2.1.300).
function userFramed(delivery) {
  return 'The user sent a new message while you were working:\n' + delivery.text
    + '\n\nThis is how Claude Code surfaces messages the user sends mid-turn \u2014 within the running turn, often alongside the next tool result, rather than as a separate conversation turn. Address the message above as you continue this turn.'
}

// Says which turn took what was handed over.
function handed($, context, deliveries, turn, how) {
  return event($, context, 'handed', { deliveries: deliveries.map((delivery) => delivery.id), turn, how })
}

// Submits words as a new turn. Returns whether Claude Code took them.
//
// Which turn takes them is not read from any text: Claude Code keeps the
// prompt as a row stamped with this plugin as its origin, and finishes every
// `session.append` hook on a turn's rows before it starts that turn. So the
// turn that starts after such a row is the one that took it (`entered`,
// `startTurn`), whether the prompt began its turn at once or waited behind a
// running one. Until then nothing else is submitted, so each submission is
// one turn of its own and the order of rows is the order of submissions.
async function submit($, context, deliveries, prompt) {
  if (context.closed || context.submitting.length > 0 || context.accepted.length > 0) return false
  context.submitting.push(...deliveries)
  try {
    if (!(await goalPermitsAll($, context, deliveries))) return false
    if ((await $.session.id()) !== context.sessionId) return false
    for (const delivery of deliveries) {
      await $.fs.write(context.dir + '/attempts/' + delivery.id, JSON.stringify({ at: stamp(), turn: context.turn, state: 'submitting' }))
    }
    // What rides with the next turn goes in just before the prompt that
    // starts it: this plugin's own prompt reaches none of its own hooks.
    await accountAgentsBeforeSubmit($, context)
    await alongBefore($, context)
    if (!(await goalPermitsAll($, context, deliveries))) {
      for (const delivery of deliveries) await $.fs.write(context.dir + '/attempts/' + delivery.id, JSON.stringify({ state: 'refused' }))
      return false
    }
    // Its row and turn can come before the call answers.
    context.accepted.push(deliveries)
    let refusal = null
    try {
      const result = await $.prompt.submit(prompt)
      if (result.drop !== undefined) refusal = result.drop
    } catch (error) {
      refusal = String(error)
    }
    if (refusal === null) return true
    const waiting = context.accepted.indexOf(deliveries)
    // A turn took them already; whatever the call said after that, they went.
    if (waiting < 0) {
      worldFailure($, 'prompt.submit', new Error(refusal))
      return true
    }
    context.accepted.splice(waiting, 1)
    for (const delivery of deliveries) await $.fs.write(context.dir + '/attempts/' + delivery.id, JSON.stringify({ state: 'refused' }))
    context.unsettled = true
    await trouble($, context, deliveries[0].id, 'it could not be handed over: ' + refusal)
    return false
  } finally {
    context.submitting.splice(0, context.submitting.length)
  }
}

// A row of a prompt this plugin submitted was kept; the next turn takes it.
function entered(context) {
  context.entered += 1
}

// Says that `acceptedTurn` took what was submitted.
async function submitted($, context, deliveries, acceptedTurn) {
  const report = handed($, context, deliveries, acceptedTurn, 'submitted')
  await Promise.all([report, ...deliveries.map((delivery) => receipt($, context, delivery, 'submitted', acceptedTurn))])
}

// Hands everything held to the model in one new turn; on failure it is
// held again, before anything held since.
async function flush($, context, how) {
  if (context.closed || context.busy || waiting(context)) return
  await readGoal($, context)
  if (context.closed || context.busy || waiting(context)) return
  // What rides with the next turn starts none: it waits for one.
  const at = context.held.findIndex((delivery) => delivery.along === undefined && goalPermits(context, delivery))
  if (at < 0) return
  const deliveries = context.held.splice(at, 1)
  if (deliveries[0].as_user !== true) {
    while (at < context.held.length && context.held[at].as_user !== true && context.held[at].along === undefined && goalPermits(context, context.held[at])) deliveries.push(...context.held.splice(at, 1))
  }
  const prompt = deliveries[0].as_user === true
    ? { text: deliveries[0].text, asUser: true }
    : { text: deliveries.map(framed).join('\n\n') }
  if (!(await submit($, context, deliveries, prompt))) context.held.splice(at, 0, ...deliveries)
}

// Told again before they went in, these never will: receipts keep them from
// being handed over again, and the node settles them.
async function replaced($, context, deliveries) {
  await Promise.all(deliveries.map((delivery) => receipt($, context, delivery, 'replaced', null)))
  await event($, context, 'replaced', { deliveries: deliveries.map((delivery) => delivery.id) })
}

// What the next turn reads of the last one the user stopped. Claude Code
// ends a turn a plugin stops as one ended to deliver a following message,
// and says so to a call it cut short: the model would not otherwise know
// the user stopped it.
const STOPPED = 'The user stopped your previous turn on purpose. A tool call it cut short may have partly taken effect, whatever its result says about why it ended. The commands it was waiting on were stopped; any already left running in the background go on.'

// What rides with the next turn, the World's words for it: that the user
// stopped the last turn, and what is held to go along. Taken as it is read;
// `keep` puts back what did not go in.
function alongNext(context) {
  const along = context.held.filter((delivery) => delivery.along !== undefined && goalPermits(context, delivery))
  for (const delivery of along) context.held.splice(context.held.indexOf(delivery), 1)
  const stopped = context.toldStopped
  context.toldStopped = []
  const texts = [...(stopped.length > 0 ? [framed({ text: STOPPED })] : []), ...along.map(framed)]
  const keep = () => {
    context.held.unshift(...along)
    context.toldStopped.unshift(...stopped)
  }
  return { along, texts, keep, stopped }
}

// What waits to ride with the next turn goes in as a row of its own just
// before a prompt this plugin submits; the turn that prompt starts takes it
// (`startTurn`), and the model reads both in the one request.
async function alongBefore($, context) {
  await readGoal($, context)
  const { along, texts, keep, stopped } = alongNext(context)
  if (texts.length === 0) return
  let appended
  try {
    context.appending = true
    appended = await $.session.append({ message: { type: 'user', content: [{ type: 'text', text: texts.join('\n\n') }] } })
  } catch (error) {
    keep()
    worldFailure($, 'session.append', error)
    return
  } finally {
    context.appending = false
  }
  if (appended.deny !== undefined) keep()
  else {
    context.alongTaken.push(...along)
    context.stopTaken.push(...stopped)
  }
}

// What waits to ride with the next turn goes in beside a prompt someone typed
// that starts it; the turn it starts takes it (`startTurn`).
async function alongPrompt($, context, e, next) {
  if (e.turnId !== undefined || e.command !== undefined || context.closed) return next(e)
  await accountAgentsBeforeSubmit($, context)
  await readGoal($, context)
  const { along, texts, keep, stopped } = alongNext(context)
  if (texts.length === 0) return next(e)
  let result
  try {
    result = await next({ ...e, context: [...(e.context ?? []), ...texts] })
  } catch (error) {
    keep()
    throw error
  }
  if (result.drop !== undefined) keep()
  else {
    context.alongTaken.push(...along)
    context.stopTaken.push(...stopped)
  }
  return result
}

// Whether a submission is still on its way into a turn of its own.
function waiting(context) {
  return context.submitting.length > 0 || context.accepted.length > 0
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

// Tries again, a while later, what a look could not settle. One try waits at
// a time; a look in between settles it sooner.
function retry($, context) {
  if (context.timer !== null || gone.seen) return
  context.timer = $.clock.after(LOOK_MS, () => {
    context.timer = null
    look($, context).catch((error) => worldFailure($, 'look', error))
  })
}

// Takes what the node left the session: a stop, words, failed reports and
// receipts to write again. Asked while it runs, it runs once more after.
async function look($, context) {
  if (await removed($)) {
    if (context.timer) {
      context.timer.cancel()
      context.timer = null
    }
    return
  }
  const current = !context.closed && (await $.session.id()) === context.sessionId
  if (current) await stopping($, context)
  if (context.looking) {
    context.again = true
    return
  }
  context.looking = true
  try {
    do {
      context.again = false
      context.unsettled = false
      await settle($, context)
      const written = context.writing.then(async () => {
        for (const name of context.failedReports) {
          await writePending($, context, name)
          context.failedReports.delete(name)
        }
      })
      context.writing = written.catch(() => {})
      try {
        await written
        await recoverAccounting($, context)
        if (!context.closed && current) await looked($, context)
      } catch (error) {
        context.unsettled = true
        worldFailure($, 'look', error)
      }
    } while (context.again)
  } finally {
    context.looking = false
  }
  if (context.unsettled || context.failedReports.size > 0 || context.owed.length > 0) retry($, context)
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
    context.toldStopped.push(asked)
  } catch (error) {
    // Claude Code refuses an abort only for a turn that is not running: the
    // turn held as running had ended unseen, and is ended here.
    await staleTurn($, context, asked)
  }
  return true
}

// Ends a turn this plugin held as running that Claude Code says is not:
// the session is idle, and what waited for the turn's end goes on.
async function staleTurn($, context, turn) {
  if (context.turn !== turn) return
  context.busy = false
  context.turn = null
  await completeControlTurn($, { turnId: turn })
  const completed = event($, context, 'turn_complete', { turn, aborted: true, reason: 'aborted', refusal: null, ms: null, answer: null, streamed: true })
  const saved = activity($, context)
  await Promise.all([completed, saved])
  await flush($, context, 'submitted at the end of a turn')
}

async function looked($, context) {
  await settle($, context)
  await readGoal($, context)
  // What has a receipt was handed over already, perhaps before this mod
  // loaded again. Without knowing which, anything handed over could repeat.
  const receipts = await listing($, context, 'receipts')
  if (receipts === null) {
    context.unsettled = true
    return
  }
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
  const read = []
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
      context.unsettled = true
      await trouble($, context, name, 'it could not be read: ' + error)
      continue
    }
    read.push([name, delivery])
  }
  // Arrival time is independent of the producer's ID prefix. Legacy node
  // envelopes retain their previous filename order before dated arrivals.
  read.sort(([nameA, a], [nameB, b]) => {
    const [secondA, fractionA = ''] = (a.at ?? '').replace('Z', '').split('.')
    const [secondB, fractionB = ''] = (b.at ?? '').replace('Z', '').split('.')
    const atA = secondA + '.' + fractionA.padEnd(9, '0')
    const atB = secondB + '.' + fractionB.padEnd(9, '0')
    return atA < atB ? -1 : atA > atB ? 1 : nameA < nameB ? -1 : nameA > nameB ? 1 : 0
  })
  // What the World told again before it went in replaces what it told
  // before: only the latest goes in.
  const replacing = new Set(read.flatMap(([, delivery]) => delivery.replaces ?? []))
  for (const [name, delivery] of read) {
    if (replacing.has(delivery.id)) {
      context.handled.add(name)
      await replaced($, context, [delivery])
      continue
    }
    const older = context.held.filter((held) => (delivery.replaces ?? []).includes(held.id))
    for (const one of older) context.held.splice(context.held.indexOf(one), 1)
    if (older.length > 0) await replaced($, context, older)
    // What rides with the next turn starts none: it waits for one, or goes
    // into the one running at its next tool boundary.
    if (context.busy || waiting(context) || delivery.along !== undefined || !goalPermits(context, delivery)) {
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
    // The turn that takes it writes the receipt together with handed.
  }
}

// The conversation the World started this session for, as the settings it
// was started with (`--settings`) name it: the World's sessions stand in
// their project's directory together, so nothing else tells them apart
// before Claude Code says which session it started. Claude Code's daemon
// starts and wakes a background session with those settings (2.1.296).
async function startedFor($) {
  const flag = await $.settings.read({ source: 'flag' })
  const named = flag.env === undefined || flag.env === null ? undefined : flag.env.FASTCTX_CONVERSATION
  return typeof named === 'string' && named !== '' ? named : null
}

// A background session is told to keep scratch files in its job's directory,
// which exists only on the machine it lives on and only for its owner. The
// World's `run` keeps them in `$FASTCTX_SCRATCH` on whichever machine a script
// runs, so the paragraph naming the job's directory goes and one way remains.
function withoutJobScratch(result) {
  if (typeof result.text !== 'string') return result
  const paragraphs = result.text.split('\n\n')
  const kept = paragraphs.filter((paragraph) => !paragraph.includes('CLAUDE_JOB_DIR'))
  return kept.length === paragraphs.length ? result : { ...result, text: kept.join('\n\n') }
}

// The session's directory, reserved by the node, which moves it into its
// conversation then; the controls wait for it (`reservations`).
async function reserveDirectory($, context) {
  const command = [FASTCTX, 'claude-session-directory', WORLD, context.sessionId]
  command.push('--cwd', await $.session.cwd())
  if (context.previous !== null && context.previous !== undefined) command.push('--previous', context.previous)
  const conversation = await startedFor($)
  if (conversation !== null) command.push('--conversation', conversation)
  const reserved = await $.process.run(command)
  // The program's own words already say what it was reserving.
  if (reserved.exitCode !== 0) throw new Error(reserved.stderr.replace(/^fastctx:\s*/, '').trim() || 'the Claude session directory was not reserved')
  return JSON.parse(reserved.stdout)
}

async function startSession($, context) {
  const id = context.sessionId
  const reserving = reserveDirectory($, context)
  reservations.set(id, reserving)
  context.dir = await reserving
  // A reload can happen in the same clock tick, with old reports still
  // pending. Their reserved names survive in events or applied.
  for (const sub of ['events', 'applied', 'accounting']) {
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
  await initializeAccounting($, context)
  const files = (await $.fs.list(context.dir)).map((entry) => entry.name)
  // A stopped turn remains the next turn's context across host retirement.
  await $.fs.write(context.dir + '/told-stops/.created', '')
  if (files.includes('stop.json')) {
    const requestedStop = JSON.parse(await $.fs.read(context.dir + '/stop.json')).turn
    const stopReceipts = new Set((await $.fs.list(context.dir + '/told-stops')).map((entry) => entry.name))
    if (typeof requestedStop === 'string' && !stopReceipts.has(encodeURIComponent(requestedStop))) {
      context.toldStopped.push(requestedStop)
    }
  }
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
  await observeModels($, context)
  // What already waits when the session starts again is handed over, not
  // skipped: a closed session's answers are what it should see on return.
  // After that, the session's control helper says when there is more.
  controlWake(id, () => look($, context).catch((error) => worldFailure($, 'look', error)))
  $.clock.after(0, () => look($, context).catch((error) => worldFailure($, 'look', error)))
  // Background observations retain this conversation even after /clear.
  context.observe = (happened) => {
    try {
      streamEvent($, context, { kind: 'session', happened })
    } catch (error) {
      worldFailure($, 'observation', error)
    }
  }
  // Claude Code keeps a record before its file has it; the node waits for
  // the last one kept when it reads the file.
  context.stored = (uuid) => streamEvent($, context, { kind: 'stored', uuid })
  // Which live thinking item a stored row ended: the node finds that row in
  // the file for Claude Code's own measure of it and the request's usage.
  context.thought = ({ uuid, turn, item }) => streamEvent($, context, { kind: 'thought', uuid, turn, item })
}

// The session's figures as Claude Code's status line has them, its cost
// total among them; none where it cannot say.
async function sessionUsage($) {
  try {
    return await $.session.usage()
  } catch {
    return null
  }
}

async function startTurn($, context, e, next) {
  // A subagent's run is a turn of its own loop: the main loop's turn, its
  // state and what it takes are untouched (Claude Code reports one since
  // 2.1.300; its end is `completeTurn`'s agent branch).
  if (e.agentId !== undefined) return next(e)
  await startControlTurn($, e)
  context.busy = true
  context.turn = e.turnId
  noteTurnPrompt(context, e)
  // The session's cost total as the turn begins: what it has risen by at
  // the turn's end is what the turn cost (`completeTurn`).
  const usage = await sessionUsage($)
  context.spentBefore = { turn: e.turnId, usd: usage?.cost?.usd }
  // The rows of this plugin's prompts kept since the last turn began are
  // what this turn takes (`submit`).
  const took = context.accepted.splice(0, context.entered)
  context.entered = 0
  // So did what rode in beside its prompt (`alongPrompt`).
  const along = context.alongTaken.splice(0, context.alongTaken.length)
  if (along.length > 0) took.push(along)
  const started = event($, context, 'turn_start', { turn: context.turn, activation: context.registration.instance })
  const saved = activity($, context)
  await Promise.all([started, saved])
  for (const deliveries of took) await submitted($, context, deliveries, e.turnId)
  for (const prior of context.stopTaken) await $.fs.write(context.dir + '/told-stops/' + encodeURIComponent(prior), '')
  context.stopTaken = []
  return next(e)
}

async function completeTurn($, context, e, next) {
  await completeControlTurn($, e)
  if (e.agentId !== undefined) {
    const reported = agentReports(reports($, context), e.agentId)
    const how = e.isAborted ? 'interrupted' : e.reason === 'error' || e.reason === 'refusal' ? 'failed' : 'completed'
    await agentPresentation(context, e.agentId).end(reported, e.turnId)
    await reported.event('session', { happened: { event: 'turn_ended', turn: e.turnId,
      end: { how, reason: e.reason, duration_ms: observedMilliseconds(e.durationMs), last_message: e.answer, error: null } } })
    // Its tokens as Claude Code counts an agent's in its Agent call's answer:
    // the last request's input, cached and fresh, and its output.
    const usage = e.usage
    const tokens = usage == null ? null : ['input_tokens', 'output_tokens', 'cache_read_input_tokens', 'cache_creation_input_tokens']
      .reduce((sum, key) => sum + (typeof usage[key] === 'number' ? usage[key] : 0), 0)
    await event($, context, 'session', { happened: { event: 'changed', turn: null, change: { kind: 'agent_ended', agent_id: e.agentId,
      end: { how, reason: how === 'completed' ? null : e.reason, duration_ms: observedMilliseconds(e.durationMs), tokens, tools: null,
        answer: e.answer ?? null, task: null, told: false } } } })
    await observeAgents($, context)
    return next(e)
  }
  await readGoal($, context)
  await context.shown.end(reports($, context), e.turnId)
  // The session's figures as the turn ends reach the node ahead of the end
  // itself, so its ledger and the account's windows stand as the turn left
  // them; what the cost total rose by since the turn began is the turn's.
  const usage = await stepMeasured($, context, null, true)
  const before = context.spentBefore?.turn === e.turnId ? context.spentBefore.usd : undefined
  const total = usage?.cost?.usd
  const spent = typeof total === 'number' && typeof before === 'number' ? (total >= before ? total - before : total) : null
  if (context.spentBefore?.turn === e.turnId) context.spentBefore = null
  if (context.turn === e.turnId) { context.busy = false; context.turn = null }
  const completed = event($, context, 'turn_complete', {
    turn: e.turnId,
    spent,
    aborted: e.isAborted,
    reason: e.reason,
    refusal: e.reason === 'refusal' ? e.refusal : null,
    error: completedFailure(context, e),
    ms: observedMilliseconds(e.durationMs),
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
  // A first message is read before the model's first request, so before any
  // tool's end: no read after this one begins it.
  context.contextFresh = false
  await contextCurrent($, context)
  if (await stopping($, context)) return result
  await readGoal($, context)
  const acceptedTurn = context.turn
  if (!context.busy || acceptedTurn === null) return result
  // Whatever waits goes in beside this result: the World's telling, and
  // the user's words sent while the turn runs (`world-colleagues.md`).
  const deliveries = context.held.filter((delivery) => goalPermits(context, delivery))
  if (deliveries.length === 0) return result
  const selected = new Set(deliveries)
  for (let i = context.held.length - 1; i >= 0; i--) if (selected.has(context.held[i])) context.held.splice(i, 1)
  try {
    for (const delivery of deliveries) await $.fs.write(context.dir + '/attempts/' + delivery.id, JSON.stringify({ state: 'appending', turn: acceptedTurn }))
  } catch (error) {
    // No native append was attempted. Keep these words ahead of later
    // deliveries; only an uncertain host append must not be replayed.
    context.held.unshift(...deliveries)
    throw error
  }
  if (!(await goalPermitsAll($, context, deliveries)) || !context.busy || context.turn !== acceptedTurn) {
    for (const delivery of deliveries) await $.fs.write(context.dir + '/attempts/' + delivery.id, JSON.stringify({ state: 'refused' }))
    context.held.unshift(...deliveries)
    return result
  }
  try {
    context.appending = true
    let appended
    try {
      const text = deliveries.map((delivery) => (delivery.as_user === true ? userFramed(delivery) : framed(delivery))).join('\n\n')
      appended = await $.session.append({ message: { type: 'user', content: [{ type: 'text', text }] } })
    } finally {
      context.appending = false
    }
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
  for (const deliveries of context.accepted.splice(0, context.accepted.length)) {
    await uncertain($, context, deliveries.map((delivery) => delivery.id), 'the session ended before a turn took what was submitted')
  }
  const along = context.alongTaken.splice(0, context.alongTaken.length)
  if (along.length > 0) await uncertain($, context, along.map((delivery) => delivery.id), 'the session ended before a turn took what went in beside its prompt')
  context.entered = 0
  const { id: name, at } = reserved(context)
  const body = JSON.stringify({ kind: 'session_end', at, instance: context.registration.instance, reason: e.reason, pending: [...context.unwritten.keys()].map((name) => name + '.json') })
  context.unwritten.set(name, { body })
  try {
    await writePending($, context, name)
  } catch (error) {
    context.failedReports.add(name)
    retry($, context)
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

// Named context is read afresh on every prompt rebuild. Claude Code shows
// what a read returns only in a first message it is starting: a new
// conversation's, or the one after a /clear; after a compaction, the one it
// builds from the last read (`contextCompacted`). A read on resume or after
// an invalidation leaves the model with the first message it has, so only a
// fresh read is reported as heard; the node tells the rest at a tool
// boundary.
const SUBAGENT_BLOCK = 'subagentInstructions'
const CONTEXT_PREFIX = 'fastctx:'
const OLD_CONTEXT_NAMES = ['machinesAndFolders', 'userInstructions', 'projectRecord', SUBAGENT_BLOCK]

async function contextBlocks($, context, fresh) {
  const pointer = JSON.parse(await $.fs.read(context.dir + '/context.json'))
  const state = JSON.parse(await $.fs.read(pointer.state))
  const entries = (await $.fs.list(pointer.directory)).filter((entry) => /^[a-z0-9-]{1,32}\.json$/.test(entry.name))
  const blocks = []
  const receipt = { epoch: state.epoch, revisions: {}, versions: {} }
  const order = ['user.json', 'project.json', 'focus.json', 'plan.json', 'goal.json', 'map.json']
  const rank = (entry) => order.includes(entry.name) ? order.indexOf(entry.name) : order.length
  entries.sort((a, b) => rank(a) - rank(b) || a.name.localeCompare(b.name))
  for (const entry of entries) {
    const block = JSON.parse(await $.fs.read(pointer.directory + '/' + entry.name))
    const name = entry.name.slice(0, -5)
    receipt.revisions[name] = block.revision
    receipt.versions[name] = block.version
    if (block.text !== '') blocks.push({ name: CONTEXT_PREFIX + name, text: block.text })
  }
  context.lastRead = { receipt, agents: context.accountAgentRevision }
  if (fresh) await $.fs.write(context.dir + '/context-heard.json', JSON.stringify(receipt))
  return blocks
}

// Sessions this plugin compacts itself (`controlCompact`): Claude Code reads
// the named context again for the first request after such a compaction.
const compactedByWorld = new Set()

// When Claude Code compacts a conversation by itself, it builds the new first
// message from the context it last read, without reading it again: the model
// holds that read now, and nothing a tool answer told it before. A read that
// failed left no blocks of the World's in it. After a compaction the World
// asked for, the read that follows replaces this report.
async function contextCompacted($, context) {
  const read = context.lastRead ?? { receipt: { epoch: 0, revisions: {}, versions: {} }, agents: undefined }
  const compacted = 'claude-compact-' + Date.now() + '-' + Math.random().toString(36).slice(2)
  await $.fs.write(context.dir + '/context-heard.json', JSON.stringify({ ...read.receipt, compacted }))
  await agentsHeard($, context, read.agents)
}

// Keeps that last read current: when a block changed since, Claude Code reads
// them all again. The model sees the read only when a compaction begins a
// first message with it; until then the node tells it at tool boundaries.
async function contextCurrent($, context) {
  const read = context.lastRead?.receipt
  if (read === undefined) return
  try {
    const pointer = JSON.parse(await $.fs.read(context.dir + '/context.json'))
    const entries = (await $.fs.list(pointer.directory)).filter((entry) => /^[a-z0-9-]{1,32}\.json$/.test(entry.name))
    let same = entries.length === Object.keys(read.revisions).length
    for (const entry of entries) {
      if (!same) break
      const block = JSON.parse(await $.fs.read(pointer.directory + '/' + entry.name))
      same = read.revisions[entry.name.slice(0, -5)] === block.revision
    }
    if (!same) await $.ui.invalidate('prompt.context')
  } catch (error) {
    worldFailure($, 'named context', error)
  }
}

async function session($, registry, id, renew = false) {
  await usageOrigin($, id)
  let context = registry.sessions.get(id)
  // A start that failed is tried again after a pause rather than kept for
  // the whole session: what failed may have been passing (#48).
  const retried = context !== undefined && context.failedAt !== undefined && Date.now() - context.failedAt >= START_AGAIN_MS
  if (!context || (renew && context.closed) || retried) {
    context = conversation(id, retried ? context.previous : registry.cleared)
    context.registeredAgents = registry.registeredAccountAgents
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
    if (!retried) registry.cleared = null
    registry.sessions.set(id, context)
    const starting = context
    context.ready = startSession($, context)
    context.ready.catch(() => { starting.failedAt = Date.now() })
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
  const registry = { registeredAccountAgents: new Set(), sessions: new Map(), turns: new Map(), toolOwners: new Map(), cleared: null, worldTools: new Set(), provedCalls: new Set() }
  // Every handler first steps aside once fastctx is being removed. Each keeps
  // its literal on('…') call, the shape the official loader reads.
  on('telemetry.log', { to: 'collector' }, async ($, e, next) => {
    if (e.event === 'api_request' && !await removed($)) {
      try {
        const id = e.attributes['session.id'] ?? await $.session.id()
        const context = await session($, registry, id)
        await pricedRequest($, context, e)
      } catch (error) {
        worldFailure($, 'request accounting', error)
      }
    }
    return next(e)
  }).catch(($, e, next) => continueWithoutWorld($, 'telemetry.log', e, next))
  on('session.start', async ($, e, next) => {
    await usageOrigin($, await $.session.id())
    if (await removed($)) return next(e)
    await worldTools($, registry)
    // The session's directory is reserved before its controls make or
    // watch anything in it (`reservations`).
    const context = await session($, registry, await $.session.id(), true)
    // The session begins with the account's subagents as they are, listed
    // and in its context.
    await refreshAccountAgents($, context)
    await agentsHeardBefore($, context)
    nativeGoalContexts.set(context.sessionId, context)
    await startControls($)
    scheduleNativeGoalClear($, context)
    return next(e)
  }).catch(($, e, next) => continueWithoutWorld($, 'session.start', e, next))
  on('agent.offer', async ($, e, next) => {
    if (await removed($)) return next(e)
    const context = registry.sessions.get(await $.session.id())
    if (!context) return next(e)
    const offered = offerAccountAgent(context, e)
    return offered === null ? next(e) : { isOffered: offered }
  }).catch(($, e, next) => continueWithoutWorld($, 'agent.offer', e, next))
  on('turn.start', async ($, e, next) => {
    if (await removed($)) return next(e)
    return startTurn($, await route($, registry, 'turn.start', e), e, next)
  }).catch(($, e, next) => continueWithoutWorld($, 'turn.start', e, next))
  on('turn.complete', async ($, e, next) => {
    if (await removed($)) return next(e)
    const context = await route($, registry, 'turn.complete', e)
    const result = await completeTurn($, context, e, next)
    await clearNativeGoal($, context)
    return result
  }).catch(($, e, next) => continueWithoutWorld($, 'turn.complete', e, next))
  on('turn.step', async function* ($, e, next) {
    if (await removed($)) {
      const stream = next(e)
      for await (const chunk of stream) yield chunk
      return await stream.result
    }
    const context = registry.turns.get(e.turnId) || await session($, registry, await $.session.id())
    await sessionPolicy($, context)
    const input = await controlStep($, e)
    const base = reports($, context)
    const shown = e.agentId === undefined ? context.shown : agentPresentation(context, e.agentId)
    const reported = e.agentId === undefined ? base : agentReports(base, e.agentId)
    const held = await heldStep($, context, input)
    return yield* measuredStep($, context, shown, reported, input, held ?? next, held !== null)
  }).catch(async function* ($, e, next) { return yield* streamWithoutWorld($, 'turn.step', e, next) })
  on('tool.call', async ($, e, next) => {
    try {
      const refused = await assistedTool($, e)
      if (refused !== null) return refused
    } catch (_) {
      return { deny: 'Fastctx could not verify the assisted conversation policy for this tool call.' }
    }
    if (await removed($)) return next(e)
    registry.provedCalls.delete(e.tool_use_id)
    // Strict MCP tools can arrive after session.start; confirm ownership
    // before issuing their first proof (2.1.303, 2026-10-21).
    if (!registry.worldTools.has(e.tool) && e.tool.startsWith('mcp__fastctx__') && await assistedSession($)) {
      await worldTools($, registry)
    }
    const context = await route($, registry, 'tool.call', e)
    if (registry.worldTools.has(e.tool)) {
      try {
        await proveCall($, context, e.tool_use_id)
        registry.provedCalls.add(e.tool_use_id)
      } catch (error) {
        return unidentifiedWorldCall(error)
      }
    }
    const shown = e.agentId === undefined ? context.shown : agentPresentation(context, e.agentId)
    const reported = e.agentId === undefined ? reports($, context) : agentReports(reports($, context), e.agentId)
    const result = await shown.call(reported, e, (input) => goalQuestion($, context, input, (question) => controlCall($, question, next)))
    registry.provedCalls.delete(e.tool_use_id)
    return result
  }).catch(($, e, next) => continueToolWithoutWorld($, registry, e, next))
  on('tool.check', async ($, e, next) => {
    if (await removed($)) return next(e)
    return controlPermission($, e, next)
  }).catch(($, e, next) => continueWithoutWorld($, 'tool.check', e, next))
  on('session.append', async ($, e, next) => {
    if (await removed($)) return next(e)
    const context = await route($, registry, 'session.append', e)
    const state = e.agentId === undefined ? await controlBegin($, sessions) : null
    // Restoring a fork appends its rows before the new transcript exists.
    // Reading its boundaries here would wait for this very append to finish.
    if (state !== null && state.checkpoints.full !== true) await controlCheckpoint($, state, null, false)
    if (state !== null && e.door === 'prompt' && state.busy === false) {
      state.promptCheckpoint = await controlCheckpoint($, state, null, true)
      state.promptUuid = state.lastUuid
    }
    const shown = e.agentId === undefined ? context.shown : agentPresentation(context, e.agentId)
    const reported = e.agentId === undefined ? reports($, context) : agentReports(reports($, context), e.agentId)
    const result = await shown.append(reported, e, next)
    if (e.agentId === undefined && e.door === 'prompt' && result.deny === undefined && !context.appending
      && e.origin?.kind === 'plugin' && e.origin.name === $.plugin.name) entered(context)
    if (state !== null && result.deny === undefined && ['user', 'assistant', 'attachment', 'system'].includes(result.message.type)) {
      state.lastRawUuid = result.uuid
      state.checkpoints.last_raw_uuid = result.uuid
    }
    if (state !== null && result.deny === undefined && ['user', 'assistant'].includes(result.message.type)) {
      if (await trackCheckpoint($, state, result)) state.lastUuid = result.uuid
      if (!state.busy && state.completedTurn !== undefined && e.door !== 'prompt') {
        await controlCheckpoint($, state, state.completedTurn, false)
      }
    }
    return result
  }).catch(($, e, next) => continueWithoutWorld($, 'session.append', e, next))
  on('session.compact', async ($, e, next) => {
    if (await removed($)) return next(e)
    const context = await route($, registry, 'session.compact', e)
    return context.shown.compact(reports($, context), e, (input) => controlCompaction($, input, next))
  }).catch(($, e, next) => continueWithoutWorld($, 'session.compact', e, next))
  on('prompt.submit', async ($, e, next) => {
    if (await removed($)) return next(e)
    return alongPrompt($, await session($, registry, await $.session.id()), e, next)
  }).catch(($, e, next) => continueWithoutWorld($, 'prompt.submit', e, next))
  on('prompt.context', async ($, e, next) => {
    if (await removed($)) return next(e)
    const result = await next(e)
    const context = await session($, registry, await $.session.id())
    const fresh = context.contextFresh === true
    context.contextFresh = false
    context.lastRead = undefined
    const added = await contextBlocks($, context, fresh)
    if (fresh) await agentsHeard($, context)
    if (context.subagentInstructions) {
      const at = added.findIndex((block) => block.name === CONTEXT_PREFIX + 'user')
      added.splice(at < 0 ? 0 : at + 1, 0, { name: SUBAGENT_BLOCK, text: context.subagentInstructions })
    }
    return { ...result, blocks: [...result.blocks.filter((block) =>
      !block.name.startsWith(CONTEXT_PREFIX) && !OLD_CONTEXT_NAMES.includes(block.name)), ...added] }
  }).catch(($, e, next) => continueWithoutWorld($, 'prompt.context', e, next))
  on('prompt.section', async ($, e, next) => {
    if (await removed($)) return next(e)
    // A section's name may carry a variant after a colon; its base names it.
    if (e.name.split(':')[0] !== 'bg-session' || await startedFor($) === null) return next(e)
    return withoutJobScratch(await next(e))
  }).catch(($, e, next) => continueWithoutWorld($, 'prompt.section', e, next))
  on('classic.PostModelSwitch', async ($, e, next) => {
    if (await removed($)) return next(e)
    await controlModelSwitch($, e)
    const context = await session($, registry, await $.session.id())
    const reported = e.agent_id === undefined ? reports($, context) : agentReports(reports($, context), e.agent_id)
    await reported.event('session', { happened: { event: 'changed', turn: null, change: {
      kind: 'model_switch', from_model: e.from_model, to_model: e.to_model, source: e.source, requested_model: e.requested_model, raw: e } } })
    return next(e)
  }).catch(($, e, next) => continueWithoutWorld($, 'classic.PostModelSwitch', e, next))
  on('classic.Elicitation', async ($, e, next) => {
    if (await removed($)) return next(e)
    await startControls($)
    // Classic mod results cannot carry action/content. The official command
    // hook under next publishes the answerable request with that contract.
    return next(e)
  }).catch(($, e, next) => continueWithoutWorld($, 'classic.Elicitation', e, next))
  on('command.run', async ($, e, next) => {
    if (await removed($)) return next(e)
    if (e.command === 'goal') return internalGoalClear($, e) ? next(e) : { text: NATIVE_GOAL_DISABLED, exitCode: 1 }
    if (await assistedCommand($, e.command)) return { text: assistedCommandReason, exitCode: 1 }
    if (e.command === 'model' && await assistedSession($)) {
      const result = await controlSelection($, await controlBegin($, sessions), e.args.trim(), null)
      return { text: result.said ?? '', exitCode: result.kind === 'refused' ? 1 : 0 }
    }
    return next(e)
  }).catch(() => ({ text: 'Fastctx could not verify the assisted conversation command policy.', exitCode: 1 }))
  on('command.describe', async ($, e, next) => {
    const result = await next(e)
    if (await removed($)) return result
    if (e.command === 'goal' || await assistedCommand($, e.command)) return { ...result, isHidden: true }
    const id = await $.session.id()
    if (!commandDescriptions.has(id)) commandDescriptions.set(id, new Map())
    const descriptions = commandDescriptions.get(id)
    if (!controlSame(descriptions.get(e.command), result)) {
      descriptions.set(e.command, result)
      const context = registry.sessions.get(id)
      if (context?.observe !== undefined) context.observe({ event: 'changed', turn: null, change: { kind: 'commands_changed' } })
    }
    return result
  }).catch(($, e, next) => continueWithoutWorld($, 'command.describe', e, next))
  // Which call started which agent is known only as it starts: the agent's
  // own events carry its id, its Agent call's answer comes when it ends.
  // The agent has started once `next` answers: telling of it never fails
  // the spawn, nor runs it again.
  on('agent.spawn', async ($, e, next) => {
    if (await removed($)) return next(e)
    const result = await next(e)
    if (result.agentId === undefined) return result
    try {
      const context = await session($, registry, await $.session.id())
      context.shown.spawned(result.agentId)
      await event($, context, 'session', { happened: { event: 'changed', turn: null, change: { kind: 'agent_started', agent: {
        id: result.agentId, call: e.tool_use_id || null, parent: e.parentAgentId ?? null, kind: e.subagentType || null,
        name: e.name ?? null, task: e.description || null, prompt: e.prompt || null, model: result.model || null,
        background: e.background === true, live: true } } } })
    } catch (error) {
      worldFailure($, 'agent.spawn', error)
    }
    return result
  }).catch(($, e, next) => continueWithoutWorld($, 'agent.spawn', e, next))
  on('classic.SubagentStart', async ($, e, next) => {
    if (await removed($)) return next(e)
    await observeAgents($, await session($, registry, await $.session.id()))
    return next(e)
  }).catch(($, e, next) => continueWithoutWorld($, 'classic.SubagentStart', e, next))
  on('classic.SubagentStop', async ($, e, next) => {
    if (await removed($)) return next(e)
    await observeAgents($, await session($, registry, await $.session.id()))
    return next(e)
  }).catch(($, e, next) => continueWithoutWorld($, 'classic.SubagentStop', e, next))
  on('classic.UserPromptSubmit', async ($, e, next) => {
    if (await removed($)) return next(e)
    const context = await session($, registry, await $.session.id())
    notePrompt(context, e)
    await observeTitle($, context, e)
    return next(e)
  }).catch(($, e, next) => continueWithoutWorld($, 'classic.UserPromptSubmit', e, next))
  on('classic.StopFailure', async ($, e, next) => {
    // Capture identity before the first await; a new turn may start while the report is written.
    const failure = captureStopFailure(registry.sessions.get(e.session_id), e)
    if (await removed($)) return next(e)
    const context = await session($, registry, e.session_id)
    await reportTurnFailure($, context, failure)
    return next(e)
  }).catch(($, e, next) => continueWithoutWorld($, 'classic.StopFailure', e, next))
  on('session.measure', async ($, e, next) => {
    if (await removed($)) return next(e)
    const context = await route($, registry, 'session.measure', e)
    await stepMeasured($, context, null, true)
    return next(e)
  }).catch(($, e, next) => continueWithoutWorld($, 'session.measure', e, next))
  on('classic.PostToolUse', async ($, e, next) => {
    if (await removed($)) return next(e)
    return boundary($, await route($, registry, 'classic.PostToolUse', e), e, next)
  }).catch(($, e, next) => continueWithoutWorld($, 'classic.PostToolUse', e, next))
  on('classic.PostToolUseFailure', async ($, e, next) => {
    if (await removed($)) return next(e)
    return boundary($, await route($, registry, 'classic.PostToolUseFailure', e), e, next)
  }).catch(($, e, next) => continueWithoutWorld($, 'classic.PostToolUseFailure', e, next))
  on('session.end', async ($, e, next) => {
    if (await removed($)) return next(e)
    registry.cleared = e.reason === 'clear' ? e.sessionId : null
    return endSession($, await route($, registry, 'session.end', e), e, next)
  }).catch(($, e, next) => continueWithoutWorld($, 'session.end', e, next))
  on('classic.SessionStart', async ($, e, next) => {
    if (await removed($)) return next(e)
    await worldTools($, registry)
    const context = await session($, registry, await $.session.id(), e.source === 'clear' || e.source === 'resume')
    // A start and a /clear begin a first message read afresh, and so does a
    // compaction the World asked for, at the request after it; one Claude
    // Code makes by itself builds it from the last read, which a later read
    // does not reach; a resume keeps the one the conversation has.
    const ownCompaction = e.source === 'compact' && compactedByWorld.has(await $.session.id())
    context.contextFresh = e.source !== 'resume' && (e.source !== 'compact' || ownCompaction)
    if (e.source === 'compact') await contextCompacted($, context)
    await startControls($)
    await transcript($, context, e.transcript_path)
    await observeTitle($, context, e)
    return next(e)
  }).catch(($, e, next) => continueWithoutWorld($, 'classic.SessionStart', e, next))
}

// The file is whole before the call leaves the host, so the node reads it
// whole. Its name spells each UTF-16 unit of the id in four hex digits.
async function proveCall($, context, id) {
  if (typeof id !== 'string' || id.length === 0) throw new Error('the call has no tool_use_id to identify it by')
  let name = ''
  for (let i = 0; i < id.length; i++) name += id.charCodeAt(i).toString(16).padStart(4, '0')
  await $.fs.write(context.dir + '/calls/' + name + '.json', JSON.stringify({ tool_use_id: id, registration: context.registration }))
}

async function worldTools($, registry) {
  registry.worldTools = new Set()
  // In strict mode this is an explicit global server. A mod's connect()
  // resolves its plugin's server instead, which strict mode excludes.
  let server = 'fastctx'
  if (!await assistedSession($)) {
    const connected = await $.mcp.connect('fastctx')
    if (!connected.isConnected) return
    server = connected.server
  }
  // connect answers the server's name as /mcp lists it, which for a
  // deduplicated server is the name it runs under; a tool carries the
  // name's tool-name spelling, every character outside [A-Za-z0-9_-] made
  // `_` (`claude.ai Gmail`, `claude_ai_Gmail` in the mod reference; 2.1.289
  // lists `plugin:fastctx-world:fastctx` and `plugin_fastctx-world_fastctx`).
  const spelled = server.replace(/[^A-Za-z0-9_-]/g, '_')
  const usage = await $.session.usage({ breakdown: 'summary' })
  if (usage.context.breakdown === undefined) throw new Error('the host did not provide its MCP tool ownership map')
  const owners = new Map()
  for (const tool of usage.context.breakdown.mcpTools) {
    if (owners.has(tool.name) && owners.get(tool.name) !== tool.serverName) owners.set(tool.name, null)
    else if (!owners.has(tool.name)) owners.set(tool.name, tool.serverName)
  }
  registry.worldTools = new Set([...owners].filter(([, owner]) => owner === server || owner === spelled).map(([name]) => name))
}
