// FastCtx World mod for Claude Code.
//
// It tells the World this session exists and when it is busy, and it delivers the words the
// World holds for the session: at once in a new turn when the session is idle, next to the next
// tool result when it is busy, and at the end of the turn for anything still held then.
//
// It talks to the FastCtx node only through files under the session's own directory:
//   session.json      written once when the session starts
//   events/<n>.json   one file per event (turn start and end, tool calls, session end), and
//   events/<n>.done   written after it, once it is whole
//   inbox/<id>.json   deliveries the node writes; read here, never modified
//   receipts/<id>     written here once a delivery was handed to the model
// Only these mod APIs are used: on(), $.session.*, $.env.get, $.fs.read/write/list,
// $.clock.every and $.prompt.submit.

const WORLD = '__FASTCTX_WORLD_DIR__'
const POLL_MS = 2000

let dir = null
let busy = false
let sequence = 0
// Inbox files handed to the model, or held for it, in this session.
const handled = new Set()
const failing = new Set()
// Deliveries waiting for the next tool boundary or the end of the turn.
const held = []
// Receipts that could not be written yet; written at the next look.
const owed = []

function stamp() {
  return new Date().toISOString()
}

// The marker is written after the event, so an event with a marker is whole: the node waits
// for an event without one, and sets aside one that has a marker yet does not parse. An event
// that cannot be written cannot be reported through these same files; the next one tries again.
async function event($, kind, fields) {
  if (!dir) return
  sequence += 1
  const name = String(Date.now()).padStart(15, '0') + '-' + String(sequence).padStart(6, '0')
  try {
    await $.fs.write(dir + '/events/' + name + '.json', JSON.stringify({ kind, at: stamp(), ...fields }))
    await $.fs.write(dir + '/events/' + name + '.done', '')
  } catch (error) {}
}

// A receipt tells the node the model has the delivery. One that cannot be written is owed and
// written at the next look; the delivery is not handed over again meanwhile.
async function receipt($, delivery, how) {
  try {
    await $.fs.write(dir + '/receipts/' + delivery.id, JSON.stringify({ at: stamp(), how }))
  } catch (error) {
    owed.push([delivery, how])
  }
}

async function settle($) {
  for (const [delivery, how] of owed.splice(0, owed.length)) await receipt($, delivery, how)
}

// Hands every held delivery to the model in one new turn; on failure they stay held.
async function flush($, how) {
  if (held.length === 0) return
  const deliveries = held.slice()
  try {
    await $.prompt.submit({ text: deliveries.map(framed).join('\n\n') })
  } catch (error) {
    await trouble($, deliveries[0].id, 'undelivered', error)
    return
  }
  held.splice(0, deliveries.length)
  for (const delivery of deliveries) await receipt($, delivery, how)
}

// What the model reads when the World speaks on its own account; the user's own words are
// submitted as they are, as the user.
function framed(delivery) {
  return 'The fastctx plugin delivers a message from the World.\n\n' + delivery.text
}

// The names in one of the session's directories, or null when they cannot be listed.
async function listing($, sub) {
  try {
    return (await $.fs.list(dir + '/' + sub)).map((entry) => entry.name)
  } catch (error) {
    return null
  }
}

// Tells the World once that a delivery could not be handed over yet; it is tried again at
// every look until it goes through.
async function trouble($, name, kind, error) {
  if (failing.has(name)) return
  failing.add(name)
  await event($, kind, { file: name, error: String(error) })
}

async function poll($) {
  await settle($)
  // A delivery with a receipt was handed over already, perhaps before this mod restarted.
  // Without knowing which have receipts, handing anything over could repeat it.
  const receipts = await listing($, 'receipts')
  if (receipts === null) return
  if (!busy) await flush($, 'submitted')
  const receipted = new Set(receipts)
  const fresh = ((await listing($, 'inbox')) || [])
    .filter((name) => name.endsWith('.json') && !handled.has(name) && !receipted.has(name.slice(0, -5)))
    .sort()
  for (const name of fresh) {
    let delivery = null
    try {
      delivery = JSON.parse(await $.fs.read(dir + '/inbox/' + name))
    } catch (error) {
      await trouble($, name, 'unreadable', error)
      continue
    }
    if (delivery.as_user !== true && busy) {
      held.push(delivery)
      handled.add(name)
      continue
    }
    try {
      // The user's own words, typed on one of the user's devices: only these go in as the user.
      await $.prompt.submit(delivery.as_user === true ? { text: delivery.text, asUser: true } : { text: framed(delivery) })
    } catch (error) {
      await trouble($, name, 'undelivered', error)
      continue
    }
    handled.add(name)
    failing.delete(name)
    await receipt($, delivery, delivery.as_user === true ? 'submitted as the user' : 'submitted')
  }
}

export function register(on) {
  on('session.start', async ($, e, next) => {
    const id = await $.session.id()
    dir = WORLD + '/claude/' + id
    const registration = { session: id, at: stamp() }
    try { registration.resident = await $.env.get('FASTCTX_RESIDENT') } catch (error) { registration.resident = null }
    // The session's inbox address differs per session and the session's MCP servers inherit
    // it, so it pairs this registration with the session's MCP connection. Only the address is
    // read; the token beside it is never touched.
    try { registration.inbox_address = await $.env.get('CLAUDE_CODE_MESSAGING_SOCKET') } catch (error) { registration.inbox_address = null }
    try { registration.cwd = await $.session.cwd() } catch (error) { registration.cwd = null }
    try { registration.model = await $.session.model() } catch (error) { registration.model = null }
    try { registration.surfaces = await $.session.surfaces() } catch (error) { registration.surfaces = null }
    try { registration.version = await $.session.version() } catch (error) { registration.version = null }
    // The receipts directory exists from the start, so failing to list it means a failure,
    // never "nothing handed over yet".
    await $.fs.write(dir + '/receipts/.created', '')
    await $.fs.write(dir + '/session.json', JSON.stringify(registration))
    // Deliveries already waiting when the session (re)starts are delivered, not skipped: a
    // closed session's answers are exactly what it should see when the user opens it again.
    $.clock.every(POLL_MS, () => poll($))
    return next(e)
  })

  on('turn.start', async ($, e, next) => {
    busy = true
    await event($, 'turn_start', { turn: e.turnId || null })
    return next(e)
  })

  on('turn.complete', async ($, e, next) => {
    busy = false
    await event($, 'turn_complete', { aborted: e.isAborted === true, ms: e.durationMs || null, answer: e.answer || null })
    await flush($, 'submitted at the end of a turn')
    return next(e)
  })

  on('tool.call', async ($, e, next) => {
    await event($, 'tool', { tool: e.tool || null })
    return next(e)
  })

  // At a tool boundary while busy: hand held deliveries to the model beside the tool result.
  // The result carries additionalContext as a top-level array of strings; any other shape is
  // silently skipped by Claude Code.
  on('classic.PostToolUse', async ($, e, next) => {
    const result = await next(e)
    if (held.length === 0) return result
    const deliveries = held.splice(0, held.length)
    const prior = result && Array.isArray(result.additionalContext) ? result.additionalContext : []
    for (const delivery of deliveries) await receipt($, delivery, 'beside a tool result')
    return { ...(result || {}), additionalContext: [...prior, deliveries.map(framed).join('\n\n')] }
  })

  on('session.end', async ($, e, next) => {
    await event($, 'session_end', { reason: e.reason || null })
    return next(e)
  })
}
