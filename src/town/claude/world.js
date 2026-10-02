// FastCtx World mod for Claude Code.
//
// It tells the World this session exists and when it is busy, and it delivers the words the
// World holds for the session: at once in a new turn when the session is idle, next to the next
// tool result when it is busy, and at the end of the turn for anything still held then.
//
// It talks to the FastCtx node only through files under the session's own directory:
//   session.json      written once when the session starts
//   events/<n>.json   one file per event (turn start and end, tool calls, session end)
//   inbox/<id>.json   deliveries the node writes; read here, never modified
//   receipts/<id>     written here once a delivery was handed to the model
// Only these mod APIs are used: on(), $.session.*, $.env.get, $.fs.read/write/list,
// $.clock.every and $.prompt.submit.

const WORLD = '__FASTCTX_WORLD_DIR__'
const POLL_MS = 2000

let dir = null
let busy = false
let sequence = 0
const handled = new Set()
const held = []

function stamp() {
  return new Date().toISOString()
}

async function event($, kind, fields) {
  if (!dir) return
  sequence += 1
  const name = String(Date.now()).padStart(15, '0') + '-' + String(sequence).padStart(6, '0') + '.json'
  await $.fs.write(dir + '/events/' + name, JSON.stringify({ kind, at: stamp(), ...fields }))
}

async function receipt($, delivery, how) {
  await $.fs.write(dir + '/receipts/' + delivery.id, JSON.stringify({ at: stamp(), how }))
}

// What the model reads when the World speaks on its own account; the user's own words are
// submitted as they are, as the user.
function framed(delivery) {
  return 'The fastctx plugin delivers a message from the World.\n\n' + delivery.text
}

async function poll($) {
  let entries = []
  try {
    entries = await $.fs.list(dir + '/inbox')
  } catch (error) {
    return
  }
  const fresh = entries
    .map((entry) => entry.name)
    .filter((name) => name.endsWith('.json') && !handled.has(name))
    .sort()
  for (const name of fresh) {
    handled.add(name)
    let delivery = null
    try {
      delivery = JSON.parse(await $.fs.read(dir + '/inbox/' + name))
    } catch (error) {
      await event($, 'unreadable', { file: name, error: String(error) })
      continue
    }
    if (delivery.as_user === true) {
      // The user's own words, typed on one of the user's devices: only these go in as the user.
      await $.prompt.submit({ text: delivery.text, asUser: true })
      await receipt($, delivery, 'submitted as the user')
    } else if (busy) {
      held.push(delivery)
    } else {
      await $.prompt.submit({ text: framed(delivery) })
      await receipt($, delivery, 'submitted')
    }
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
    if (held.length > 0) {
      const deliveries = held.splice(0, held.length)
      await $.prompt.submit({ text: deliveries.map(framed).join('\n\n') })
      for (const delivery of deliveries) await receipt($, delivery, 'submitted at the end of a turn')
    }
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
