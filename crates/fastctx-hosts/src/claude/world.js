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
//   events/<n>.json    one file per event, and
//   events/<n>.done    written after it, once it is whole
//   inbox/<id>.json    what the node has for the session; read, never changed
//   receipts/<id>      written once something was handed to the model
//   stop.json          the turn the node asks to be stopped; read, never changed
// Which turn took what it handed over is an event of its own, before the
// end of that turn: the node owes that turn's end to whoever it was from.
// It uses only on(), $.session.*, $.env.get, $.fs.read/write/list,
// $.clock.every, $.prompt.submit and $.turn.abort. Of the session's inbox address it
// reads the address alone, never the token beside it.

const WORLD = __FASTCTX_WORLD__
// How often the inbox is looked at: words for the session wait at most this
// long before the mod sees them, which is short beside a tool call.
const LOOK_MS = 2000

let dir = null
let busy = false
let turn = null
let sequence = 0
// Inbox files handed over or held in this load of the mod.
const handled = new Set()
// Inbox files already reported as failing, so each is reported once.
const failing = new Set()
// What waits for the next tool boundary or the end of the turn.
const held = []
// What was submitted and waits for the turn that takes it to start.
const submitting = []
// Receipts not written yet; written at the next look.
const owed = []
// One look at a time: a look waits on the session, and the clock does not.
let looking = false
// The last turn the node asked to stop, so it is asked of the session once.
let stopped = null

function stamp() {
  return new Date().toISOString()
}

// Events are written one at a time, each followed by its marker, so only the
// newest can be half written; the node waits for an unmarked event only
// while it is the newest. An event that cannot be written has no other way
// to be reported; the next one tries again.
let writing = Promise.resolve()

function event($, kind, fields) {
  if (!dir) return writing
  sequence += 1
  const name = String(Date.now()).padStart(15, '0') + '-' + String(sequence).padStart(6, '0')
  const body = JSON.stringify({ kind, at: stamp(), ...fields })
  writing = writing.then(async () => {
    try {
      await $.fs.write(dir + '/events/' + name + '.json', body)
      await $.fs.write(dir + '/events/' + name + '.done', '')
    } catch (error) {}
  })
  return writing
}

// A receipt tells the node the model has it. One that cannot be written is
// written at the next look; meanwhile it is not handed over again.
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

// What the model reads when the World speaks on its own account. The user's
// own words go in as they are, as the user's.
function framed(delivery) {
  return 'The fastctx plugin delivers a message from the World.\n\n' + delivery.text
}

// Says which turn took what was handed over.
function handed($, deliveries, turn, how) {
  return event($, 'handed', { deliveries: deliveries.map((delivery) => delivery.id), turn, how })
}

// Submits words as a new turn; the turn that takes them is told when it
// starts. Returns whether they went.
async function submit($, deliveries, prompt) {
  submitting.push(...deliveries)
  try {
    await $.prompt.submit(prompt)
    return true
  } catch (error) {
    for (const delivery of deliveries) {
      const at = submitting.indexOf(delivery)
      if (at >= 0) submitting.splice(at, 1)
    }
    await trouble($, deliveries[0].id, 'it could not be handed over: ' + error)
    return false
  }
}

// Hands everything held to the model in one new turn; on failure it is
// held again, before anything held since.
async function flush($, how) {
  if (held.length === 0) return
  const deliveries = held.splice(0, held.length)
  if (!(await submit($, deliveries, { text: deliveries.map(framed).join('\n\n') }))) {
    held.unshift(...deliveries)
    return
  }
  for (const delivery of deliveries) await receipt($, delivery, how)
}

// The names in one of the session's directories, or null when they cannot
// be listed.
async function listing($, sub) {
  try {
    return (await $.fs.list(dir + '/' + sub)).map((entry) => entry.name)
  } catch (error) {
    return null
  }
}

// Tells the World, once, that an inbox file could not be handed over yet;
// it is tried again at every look until it goes through.
async function trouble($, name, text) {
  if (failing.has(name)) return
  failing.add(name)
  await event($, 'trouble', { file: name, text })
}

async function look($) {
  if (looking) return
  looking = true
  try {
    await looked($)
  } finally {
    looking = false
  }
}

// Stops the running turn when the node asks for it. Only the turn that is
// running can be stopped, and only by its id; a request for a turn that has
// ended is left as it is.
async function stopping($) {
  let asked = null
  try {
    asked = JSON.parse(await $.fs.read(dir + '/stop.json')).turn
  } catch (error) {
    return
  }
  if (!busy || asked !== turn || asked === stopped) return
  stopped = asked
  try {
    await $.turn.abort({ turnId: asked })
  } catch (error) {
    await event($, 'stop_failed', { turn: asked, text: String(error) })
  }
}

async function looked($) {
  await settle($)
  await stopping($)
  // What has a receipt was handed over already, perhaps before this mod
  // loaded again. Without knowing which, anything handed over could repeat.
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
      await trouble($, name, 'it could not be read: ' + error)
      continue
    }
    if (delivery.as_user !== true && busy) {
      held.push(delivery)
      handled.add(name)
      continue
    }
    // Taken before the session is asked, so no other look takes it too.
    handled.add(name)
    // The user's own words, typed on one of the user's machines: only these
    // go in as the user.
    const prompt = delivery.as_user === true ? { text: delivery.text, asUser: true } : { text: framed(delivery) }
    if (!(await submit($, [delivery], prompt))) {
      handled.delete(name)
      continue
    }
    failing.delete(name)
    await receipt($, delivery, delivery.as_user === true ? 'submitted as the user' : 'submitted')
  }
}

// One line of what a tool was used on.
function detail(e) {
  const value = e.command || e.file_path || e.path || e.pattern || e.url || ''
  const first = String(value).trim().split('\n')[0]
  return String(value).trim().includes('\n') ? first + ' …' : first
}

export function register(on) {
  on('session.start', async ($, e, next) => {
    const id = await $.session.id()
    dir = WORLD + '/claude/' + id
    const registration = { session: id, at: stamp() }
    const ask = async (name, get) => {
      try {
        registration[name] = await get()
      } catch (error) {
        registration[name] = null
      }
    }
    await ask('resident', () => $.env.get('FASTCTX_RESIDENT'))
    // Each session has its own inbox address, which its MCP servers inherit:
    // it pairs this registration with the session's connection to the World.
    await ask('inbox_address', () => $.env.get('CLAUDE_CODE_MESSAGING_SOCKET'))
    await ask('entrypoint', () => $.env.get('CLAUDE_CODE_ENTRYPOINT'))
    await ask('cwd', () => $.session.cwd())
    await ask('model', () => $.session.model())
    await ask('surfaces', () => $.session.surfaces())
    await ask('version', () => $.session.version())
    // The receipts directory exists from the start, so failing to list it
    // means a failure, never "nothing handed over yet".
    await $.fs.write(dir + '/receipts/.created', '')
    await $.fs.write(dir + '/session.json', JSON.stringify(registration))
    // What already waits when the session starts again is handed over, not
    // skipped: a closed session's answers are what it should see on return.
    $.clock.every(LOOK_MS, () => look($))
    return next(e)
  })

  on('turn.start', async ($, e, next) => {
    busy = true
    turn = e.turnId || stamp()
    if (submitting.length > 0) await handed($, submitting.splice(0, submitting.length), turn, 'submitted')
    await event($, 'turn_start', { turn })
    return next(e)
  })

  on('turn.complete', async ($, e, next) => {
    busy = false
    const ended = turn || e.turnId || stamp()
    turn = null
    await event($, 'turn_complete', {
      turn: ended,
      aborted: e.isAborted === true,
      ms: e.durationMs || null,
      answer: e.answer || null,
    })
    await flush($, 'submitted at the end of a turn')
    return next(e)
  })

  on('tool.call', async ($, e, next) => {
    await event($, 'tool_start', { turn, tool: e.tool || null, detail: detail(e) })
    const result = await next(e)
    await event($, 'tool_end', { turn, tool: e.tool || null, detail: detail(e), failed: !!(result && result.isError) })
    return result
  })

  // At a tool boundary while busy: what is held goes to the model beside the
  // tool result. The result carries additionalContext as a top-level array
  // of strings; Claude Code skips any other shape without a word.
  on('classic.PostToolUse', async ($, e, next) => {
    const result = await next(e)
    if (held.length === 0) return result
    const deliveries = held.splice(0, held.length)
    const prior = result && Array.isArray(result.additionalContext) ? result.additionalContext : []
    await handed($, deliveries, turn, 'beside a tool result')
    for (const delivery of deliveries) await receipt($, delivery, 'beside a tool result')
    return { ...(result || {}), additionalContext: [...prior, deliveries.map(framed).join('\n\n')] }
  })

  on('session.end', async ($, e, next) => {
    await event($, 'session_end', { reason: e.reason || null })
    return next(e)
  })
}
