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
// $.clock.every, $.prompt.submit, $.process.run and $.turn.abort. Of the session's inbox address it
// reads the address alone, never the token beside it.

function conversation(sessionId, previous) {
  const WORLD = __FASTCTX_WORLD__
  const FASTCTX = __FASTCTX_COMMAND__
  // How often the inbox is looked at: words for the session wait at most this
  // long before the mod sees them, which is short beside a tool call.
  const LOOK_MS = 2000

  let dir = null
  let closed = false
  let timer = null
  let busy = false
  let turn = null
  let sequence = 0
  let logicalTime = 0
  let hostProcess = null
  let requestInFlight = false
  const submissionTurns = []
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

  // Claude Code 2.1.287 limits each fs read/write to 4 MiB of UTF-8.
  const FILE_BYTES = 4 * 1024 * 1024
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

  function stamp() {
    return new Date().toISOString()
  }

  // Events are written one at a time, each followed by its marker, so only the
  // newest can be half written; the node waits for an unmarked event only
  // while it is the newest. An event that cannot be written has no other way
  // to be reported; the next one tries again.
  let writing = Promise.resolve()
  const unwritten = new Map()
  const failedReports = new Set()

  function nextName() {
    const now = Date.now()
    if (now > logicalTime) { logicalTime = now; sequence = 0 }
    sequence += 1
    // Carry at the six-digit field boundary so lexical order remains causal.
    if (sequence === 1000000) { logicalTime += 1; sequence = 0 }
    return String(logicalTime).padStart(15, '0') + '-' + String(sequence).padStart(6, '0')
  }

  function event($, kind, fields) {
    if (!dir) return writing
    const name = nextName()
    const body = JSON.stringify({ kind, at: stamp(), ...fields })
    const destination = dir + '/events/' + name
    unwritten.set(name, body)
    const written = writing.then(async () => {
      await writeReport($, destination, body)
      await $.fs.write(destination + '.done', '')
      unwritten.delete(name)
    })
    // A failed report fails its caller; it never counts as an acknowledgment.
    writing = written.catch(() => { failedReports.add(name) })
    return written
  }

  function activity($) {
    const body = JSON.stringify({ process: hostProcess, turn: busy ? turn : null })
    const saved = writing.then(() => $.fs.write(dir + '/activity.json', body))
    writing = saved.catch(() => {})
    return saved
  }

  // A receipt tells the node the model has it. One that cannot be written is
  // written at the next look; meanwhile it is not handed over again.
  async function receipt($, delivery, how, acceptedTurn = turn) {
    try {
      await $.fs.write(dir + '/receipts/' + delivery.id, JSON.stringify({ at: stamp(), how, turn: acceptedTurn }))
    } catch (error) {
      owed.push([delivery, how, acceptedTurn])
    }
  }

  async function settle($) {
    for (const [delivery, how, acceptedTurn] of owed.splice(0, owed.length)) await receipt($, delivery, how, acceptedTurn)
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
    if (closed || submitting.length > 0) return false
    submitting.push(...deliveries)
    if ((await $.session.id()) !== sessionId) {
      submitting.splice(0, submitting.length)
      return false
    }
    for (const delivery of deliveries) {
      await $.fs.write(dir + '/attempts/' + delivery.id, JSON.stringify({ at: stamp(), turn, state: 'submitting' }))
    }
    try {
      requestInFlight = true
      submissionTurns.splice(0, submissionTurns.length)
      const result = await $.prompt.submit(prompt)
      requestInFlight = false
      if (result.drop !== undefined) throw new Error(result.drop)
      const candidates = submissionTurns.filter((started) => started.text === result.text)
      submitting.splice(0, submitting.length)
      if (candidates.length !== 1) {
        await uncertain($, deliveries.map((delivery) => delivery.id), 'the host did not identify one consuming turn for this submission')
        return true
      }
      const acceptedTurn = candidates[0].turnId
      const report = handed($, deliveries, acceptedTurn, 'submitted')
      await Promise.all([report, ...deliveries.map((delivery) => receipt($, delivery, 'submitted', acceptedTurn))])
      return true
    } catch (error) {
      requestInFlight = false
      if (!deliveries.some((delivery) => submitting.includes(delivery))) throw error
      for (const delivery of deliveries) {
        const at = submitting.indexOf(delivery)
        if (at >= 0) submitting.splice(at, 1)
      }
      for (const delivery of deliveries) await $.fs.write(dir + '/attempts/' + delivery.id, JSON.stringify({ state: 'refused' }))
      await trouble($, deliveries[0].id, 'it could not be handed over: ' + error)
      return false
    }
  }

  // Hands everything held to the model in one new turn; on failure it is
  // held again, before anything held since.
  async function flush($, how) {
    if (closed || busy || submitting.length > 0 || held.length === 0) return
    const deliveries = [held.shift()]
    if (deliveries[0].as_user !== true) {
      while (held.length > 0 && held[0].as_user !== true) deliveries.push(held.shift())
    }
    const prompt = deliveries[0].as_user === true
      ? { text: deliveries[0].text, asUser: true }
      : { text: deliveries.map(framed).join('\n\n') }
    if (!(await submit($, deliveries, prompt))) held.unshift(...deliveries)
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
  async function uncertain($, ids, text) {
    const key = 'uncertain:' + ids.join(',')
    if (failing.has(key)) return
    failing.add(key)
    await event($, 'uncertain', { deliveries: ids, text })
  }

  async function trouble($, name, text) {
    if (failing.has(name)) return
    failing.add(name)
    await event($, 'trouble', { file: name, text })
  }

  async function look($) {
    const current = !closed && (await $.session.id()) === sessionId
    if (current) await stopping($)
    if (looking) return
    looking = true
    try {
      await settle($)
      const retry = writing.then(async () => {
        for (const name of failedReports) {
          await writeReport($, dir + '/events/' + name, unwritten.get(name))
          await $.fs.write(dir + '/events/' + name + '.done', '')
          unwritten.delete(name)
          failedReports.delete(name)
        }
      })
      writing = retry.catch(() => {})
      await retry
      if (closed && timer && owed.length === 0 && unwritten.size === 0) timer.cancel()
      if (closed || !current) return
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
    if (!busy || asked !== turn) return false
    if (asked === stopped) return true
    stopped = asked
    try {
      await $.turn.abort({ turnId: asked })
    } catch (error) {
      await event($, 'stop_failed', { turn: asked, text: String(error) })
    }
    return true
  }

  async function looked($) {
    await settle($)
    // What has a receipt was handed over already, perhaps before this mod
    // loaded again. Without knowing which, anything handed over could repeat.
    const receipts = await listing($, 'receipts')
    if (receipts === null) return
    if (!busy) await flush($, 'submitted')
    const receipted = new Set(receipts)
    // Node acknowledgment writes receipts before retiring handed reports.
    // Scan both, then receipts again, so that transition cannot fall between
    // two snapshots taken while this mod is reloading.
    for (const name of (await listing($, 'events')) || []) {
      if (!name.endsWith('.json')) continue
      try {
        const report = await readReport($, dir + '/events/' + name.slice(0, -5))
        if (report.kind === 'handed') for (const id of report.deliveries) receipted.add(id)
      } catch (error) {
        const latest = await listing($, 'receipts')
        if (latest === null) throw error
        for (const id of latest) receipted.add(id)
      }
    }
    for (const id of (await listing($, 'receipts')) || []) receipted.add(id)
    const attempts = new Set((await listing($, 'attempts')) || [])
    const fresh = ((await listing($, 'inbox')) || [])
      .filter((name) => name.endsWith('.json') && !handled.has(name) && !receipted.has(name.slice(0, -5)))
      .sort()
    for (const name of fresh) {
      if (attempts.has(name.slice(0, -5)) && JSON.parse(await $.fs.read(dir + '/attempts/' + name.slice(0, -5))).state !== 'refused') {
        await uncertain($, [name.slice(0, -5)], 'a prior mod stopped during submission; acceptance is unknown')
        continue
      }
      let delivery = null
      try {
        delivery = JSON.parse(await $.fs.read(dir + '/inbox/' + name))
        if (delivery.text_parts !== undefined) {
          delivery.text = ''
          for (let index = 0; index < delivery.text_parts; index++) {
            delivery.text += await $.fs.read(dir + '/inbox-parts/' + delivery.id + '/' + index)
          }
        }
      } catch (error) {
        await trouble($, name, 'it could not be read: ' + error)
        continue
      }
      if (busy || submitting.length > 0) {
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
      // The turn.start callback writes the receipt together with handed.
      // Successful submission alone does not identify a consuming turn.
    }
  }

  // A tool call's own arguments: its input without the keys the engine
  // strips before the tool runs.
  function argumentsOf(e) {
    const { tool, tool_use_id, consent, agentId, ...input } = e
    return input
  }

  function wire(on) {
    on('session.start', async ($, e, next) => {
      const id = sessionId
      dir = WORLD + '/claude/' + id
      // A reload can happen in the same clock tick, with old reports still
      // pending. Their reserved names survive in events or applied.
      for (const sub of ['events', 'applied']) {
        await $.fs.write(dir + '/' + sub + '/.created', '')
        const names = await listing($, sub)
        if (names === null) throw new Error('cannot recover the report sequence')
        for (const name of names) {
          const match = /^(\d+)-(?:end-)?(\d+)\./.exec(name)
          if (!match) continue
          const time = Number(match[1]), count = Number(match[2])
          if (time > logicalTime) { logicalTime = time; sequence = count }
          else if (time === logicalTime) sequence = Math.max(sequence, count)
        }
      }
      const registration = { session: id, previous, at: stamp() }
      const parent = await $.process.run([FASTCTX, 'parent-process'])
      if (parent.exitCode !== 0) throw new Error('identifying the Claude host: ' + parent.stderr)
      registration.process = JSON.parse(parent.stdout)
      hostProcess = registration.process
      const files = (await $.fs.list(dir)).map((entry) => entry.name)
      if (files.includes('activity.json')) {
        const previous = JSON.parse(await $.fs.read(dir + '/activity.json'))
        if (previous.process && previous.process.pid === hostProcess.pid && previous.process.token === hostProcess.token) {
          turn = previous.turn
          busy = turn !== null
        }
      }
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
      timer = $.clock.every(LOOK_MS, () => look($))
      return next(e)
    })

    on('turn.start', async ($, e, next) => {
      busy = true
      turn = e.turnId
      if (requestInFlight) submissionTurns.push({ turnId: e.turnId, text: e.text })
      const started = event($, 'turn_start', { turn })
      const saved = activity($)
      await Promise.all([started, saved])
      return next(e)
    })

    on('turn.complete', async ($, e, next) => {
      if (e.agentId !== undefined) return next(e)
      if (turn === e.turnId) { busy = false; turn = null }
      const completed = event($, 'turn_complete', {
        turn: e.turnId,
        aborted: e.isAborted,
        reason: e.reason,
        refusal: e.reason === 'refusal' ? e.refusal : null,
        ms: e.durationMs,
        answer: e.answer ?? null,
      })
      const saved = activity($)
      await Promise.all([completed, saved])
      await flush($, 'submitted at the end of a turn')
      return next(e)
    })

    on('tool.call', async ($, e, next) => {
      if (e.agentId !== undefined) return next(e)
      const fields = { turn, item: e.tool_use_id, tool: e.tool, input: argumentsOf(e) }
      await event($, 'tool_start', fields)
      const began = Date.now()
      const result = await next(e)
      const said = result.deny !== undefined ? result.deny : result.text
      await event($, 'tool_end', {
        ...fields,
        output: typeof said === 'string' ? [{ type: 'text', text: said }] : [],
        failed: result.isError === true,
        declined: result.deny !== undefined,
        ms: Date.now() - began,
      })
      return result
    })

    // Both successful and failed main-tool boundaries can append World facts.
    // Appending preserves the text without the classic context-size rewrite.
    async function boundary($, e, next) {
      if (e.agent_id !== undefined) return next(e)
      const result = await next(e)
      if (closed || (await $.session.id()) !== sessionId) return result
      if (await stopping($)) return result
      const acceptedTurn = turn
      if (!busy || acceptedTurn === null) return result
      const deliveries = held.filter((delivery) => delivery.as_user !== true)
      if (deliveries.length === 0) return result
      const selected = new Set(deliveries)
      for (let i = held.length - 1; i >= 0; i--) if (selected.has(held[i])) held.splice(i, 1)
      for (const delivery of deliveries) await $.fs.write(dir + '/attempts/' + delivery.id, JSON.stringify({ state: 'appending', turn: acceptedTurn }))
      try {
        const appended = await $.session.append({ message: { type: 'user', content: [{ type: 'text', text: deliveries.map(framed).join('\n\n') }] } })
        if (appended.deny !== undefined) {
          for (const delivery of deliveries) await $.fs.write(dir + '/attempts/' + delivery.id, JSON.stringify({ state: 'refused' }))
          held.unshift(...deliveries)
          await trouble($, deliveries[0].id, appended.deny)
          return result
        }
        for (const delivery of deliveries) await receipt($, delivery, 'appended at a main tool boundary', acceptedTurn)
        await handed($, deliveries, acceptedTurn, 'appended at a main tool boundary')
      } catch (error) {
        // append may already have crossed the host boundary: retain its
        // evidence rather than replaying an unconfirmed external effect.
        await trouble($, deliveries[0].id, 'append acceptance needs reconciliation: ' + error)
        throw error
      }
      return result
    }
    on('classic.PostToolUse', boundary)
    on('classic.PostToolUseFailure', boundary)

    on('session.end', async ($, e, next) => {
      closed = true
      const name = nextName()
      const path = dir + '/events/' + name
      const body = JSON.stringify({ kind: 'session_end', at: stamp(), reason: e.reason, pending: [...unwritten.keys()].map((name) => name + '.json') })
      unwritten.set(name, body)
      try {
        await writeReport($, path, body)
        await $.fs.write(path + '.done', '')
        unwritten.delete(name)
        if (timer && owed.length === 0 && unwritten.size === 0) timer.cancel()
      } catch (error) {
        failedReports.add(name)
        throw error
      }
      return next(e)
    })
  }

  const handlers = {}
  wire((name, handler) => { handlers[name] = handler })
  return { handlers, ready: null, get closed() { return closed } }
}

export function register(on) {
  const sessions = new Map()
  const turns = new Map()
  let currentSession = null
  async function session($, id, renew = false) {
    let context = sessions.get(id)
    if (!context || (renew && context.closed)) {
      context = conversation(id, currentSession)
      currentSession = id
      sessions.set(id, context)
      context.ready = context.handlers['session.start']($, {}, async () => ({}))
    }
    await context.ready
    return context
  }
  const route = (name) => async ($, e, next) => {
    const id = name === 'session.end' ? e.sessionId : await $.session.id()
    const context = (name === 'turn.complete' && turns.get(e.turnId)) || await session($, id)
    if (name === 'turn.start') turns.set(e.turnId, context)
    if (name === 'session.start') return next(e)
    return context.handlers[name]($, e, next)
  }
  on('session.start', route('session.start'))
  on('turn.start', route('turn.start'))
  on('turn.complete', route('turn.complete'))
  on('tool.call', route('tool.call'))
  on('classic.PostToolUse', route('classic.PostToolUse'))
  on('classic.PostToolUseFailure', route('classic.PostToolUseFailure'))
  on('session.end', route('session.end'))
  on('classic.SessionStart', async ($, e, next) => {
    await session($, await $.session.id(), e.source === 'clear' || e.source === 'resume')
    return next(e)
  })
}
