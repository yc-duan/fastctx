// Conversation controls use Claude Code's public methods. Human requests
// wait in a process stream: unlike a Promise or clock.sleep, pulling that
// stream does not consume the hook's ten-second execution budget.
// Embedded into world.js when publishing: the official scanner follows $
// into functions in that file, never across an import.
const sessions = new Map()
const turns = new Map()
const controlStarting = new Map()
// What else a session's helper wakes besides its controls: world.js's look.
const wakes = new Map()
// The reservation of each session's directory, as world.js makes it. The
// node moves the directory into its conversation as it reserves it, and
// Windows refuses to move a directory while a watch is open inside it: the
// controls make and watch nothing there before that is done.
const reservations = new Map()

async function controlReserved(sessionId) {
  const reserving = reservations.get(sessionId)
  // 2026-10-21: an early control hook must not recreate a directory while the node moves it.
  if (!reserving) throw new Error('the Claude session directory has not been reserved yet')
  await reserving
}

function controlWake(sessionId, look) {
  wakes.set(sessionId, look)
}

function controlBytes(text) {
  let size = 0
  for (const c of text) {
    const n = c.codePointAt(0)
    size += n <= 0x7f ? 1 : n <= 0x7ff ? 2 : n <= 0xffff ? 3 : 4
  }
  return size
}

async function controlWrite($, path, value) {
  const text = JSON.stringify(value)
  if (controlBytes(text) > FILE_BYTES) throw new Error('Claude Code limits a control exchange file to 4 MiB')
  await $.fs.write(path, text)
}

async function controlNames($, path) {
  return (await $.fs.list(path)).map((entry) => entry.name)
}

function controlSame(left, right) {
  if (left === right) return true
  if (left === null || right === null || typeof left !== 'object' || typeof right !== 'object') return false
  const keys = Object.keys(left)
  return keys.length === Object.keys(right).length && keys.every((key) => Object.hasOwn(right, key) && controlSame(left[key], right[key]))
}

async function controlBegin($, sessions) {
  const sessionId = await $.session.id()
  const known = sessions.get(sessionId)
  if (known && !known.closed) return known
  const pending = controlStarting.get(sessionId)
  if (pending) return pending
  const creating = controlOpen($, sessions, sessionId)
  controlStarting.set(sessionId, creating)
  try { return await creating } finally { controlStarting.delete(sessionId) }
}

// Null once fastctx is being removed: no directory is made and no helper is
// started, whose program the removal must delete.
async function controlOpen($, sessions, sessionId) {
  if (await removed($, true)) return null
  await controlReserved(sessionId)
  const directory = WORLD + '/claude/' + sessionId + '/control'
  await $.fs.write(directory + '/inbox/.created', '')
  await $.fs.write(directory + '/replies/.created', '')
  await $.fs.write(directory + '/requests/.created', '')
  await $.fs.write(directory + '/closed/.created', '')
  const files = await controlNames($, directory)
  const prior = files.includes('host.json') ? JSON.parse(await $.fs.read(directory + '/host.json')) : null
  // Besides witnessing this activation's lifetime, the helper says `look`
  // whenever the node leaves the session something, and `removed` when
  // fastctx's removal begins (then it ends).
  const guard = $.process.spawn({ argv: [FASTCTX, 'claude-control-guard', '--watch', WORLD + '/claude/' + sessionId, '--root', ROOT] })
  try {
    let header = '', stderr = ''
    while (!header.includes('\n')) {
      const piece = await guard.next()
      if (piece.done) throw new Error('the control lifetime helper ended before identifying itself: ' + stderr)
      if (piece.value.stream === 'stdout') header += piece.value.text
      else stderr += piece.value.text
    }
    const identity = JSON.parse(header.slice(0, header.indexOf('\n')))
    const host = {
      session: sessionId, process: identity.process, parent: identity.parent,
      command_contract: 2,
      handoff_contract: 1,
      assisted: await assistedSession($),
      // Persisted before polling: even a reload in the same clock tick or
      // after a clock rollback gets a different answer address.
      instance: String(Math.max(Date.now(), Number(prior?.instance ?? 0) + 1)),
    }
    const state = { directory, host, closed: false, ready: false, busy: false, turn: null, looking: false,
      selection: files.includes('selection.json') ? JSON.parse(await $.fs.read(directory + '/selection.json')) : {},
      routing: files.includes('routing.json') ? JSON.parse(await $.fs.read(directory + '/routing.json')) : null,
      checkpoints: files.includes('checkpoints.json') ? await readReport($, directory + '/checkpoints') : { messages: [], turns: {}, full: false, uuids: [] },
      rewind: null, compacting: false, automaticModel: false,
      sequence: 0, calls: new Map(), replies: new Map(), handled: new Set(), pending: new Map(), starting: new Set(), timer: null, guard,
      heard: header.slice(header.indexOf('\n') + 1), again: false }
    const closed = await controlNames($, directory + '/closed')
    state.lastUuid = state.checkpoints.last_uuid ?? undefined
    state.lastRawUuid = state.checkpoints.last_raw_uuid ?? undefined
    if (files.includes('activity.json') && prior && !closed.includes(prior.instance + '.json') && controlSame(prior.parent, host.parent)) {
      const controlActivity = JSON.parse(await $.fs.read(directory + '/activity.json'))
      state.turn = controlActivity.turn
      // There is no public current-turn getter. Events may have happened
      // while the module was absent, so the old activity is not a fact now.
      state.busy = null
    }
    await controlWrite($, directory + '/host.json', host)
    await controlWrite($, directory + '/activity.json', { instance: host.instance, turn: null })
    await $.fs.write(directory + '/host.done', '')
    sessions.set(sessionId, state)
    const offered = await controlExecute($, state, { do: 'offers' })
    await writeReport($, directory + '/catalog-' + host.instance, JSON.stringify(offered))
    await $.fs.write(directory + '/catalog-' + host.instance + '.done', '')
    void controlWatchGuard($, state)
    // Controls asked for before this activation are taken at once.
    void controlPoll($, state)
    return state
  } catch (error) {
    const partial = sessions.get(sessionId)
    if (partial?.guard === guard) {
      partial.closed = true
      partial.timer?.cancel()
      restartControls($, partial)
    }
    await guard.return()
    throw error
  }
}

function restartControls($, state) {
  if (gone.seen || sessions.get(state.host.session) !== state) return
  state.timer = $.clock.after(LOOK_MS, async () => {
    state.timer = null
    if (gone.seen || sessions.get(state.host.session) !== state || await $.session.id() !== state.host.session) return
    try { await controlBegin($, sessions) }
    catch (error) { worldFailure($, 'control restart', error); restartControls($, state) }
  })
}

async function controlWatchGuard($, state) {
  try {
    controlHeard($, state, '')
    for await (const piece of state.guard) {
      if (piece.stream === 'stderr') $.ui.log('World control lifetime helper: ' + piece.text, { to: 'debug' })
      else controlHeard($, state, piece.text)
    }
    if (!state.closed && !gone.seen) throw new Error('the World control lifetime helper exited')
  } catch (error) {
    if (!state.closed) $.ui.log(String(error), { to: 'debug' })
  } finally {
    // The node observes the child itself. No filesystem call from an
    // already-unloaded module is needed to release its pending controls.
    const lost = !state.closed && !gone.seen
    state.closed = true
    state.timer?.cancel()
    if (lost) restartControls($, state)
  }
}

// One line per wake-up from the helper.
function controlHeard($, state, text) {
  state.heard += text
  let end
  while ((end = state.heard.indexOf('\n')) >= 0) {
    const said = state.heard.slice(0, end).trim()
    state.heard = state.heard.slice(end + 1)
    if (said === 'removed') gone.seen = true
    else if (said !== 'look') continue
    void controlPoll($, state)
    const look = wakes.get(state.host.session)
    if (look !== undefined) void look()
  }
}

// Tries the controls again a while later when a poll could not finish.
function controlRetry($, state) {
  if (state.timer !== null || state.closed || gone.seen) return
  state.timer = $.clock.after(LOOK_MS, () => {
    state.timer = null
    void controlPoll($, state)
  })
}

async function controlPoll($, state) {
  if (state.closed) return
  if (state.looking) {
    state.again = true
    return
  }
  if (await removed($)) {
    state.closed = true
    state.timer?.cancel()
    await state.guard.return()
    return
  }
  state.looking = true
  try {
    do {
      state.again = false
      if (await $.session.id() !== state.host.session) return
      if (!state.ready) {
        const usage = await $.session.usage({ breakdown: 'summary' })
        const tools = usage.context.breakdown?.mcpTools ?? []
        let server = 'fastctx'
        if (!state.host.assisted) {
          const connected = await $.mcp.connect('fastctx')
          if (!connected.isConnected) { controlRetry($, state); return }
          server = connected.server
        }
        const owned = server.replace(/[^A-Za-z0-9_-]/g, '_')
        if (!tools.some((tool) => tool.isLoaded && tool.serverName.replace(/[^A-Za-z0-9_-]/g, '_') === owned)) {
          controlRetry($, state)
          return
        }
        await controlWrite($, state.directory + '/ready-' + state.host.instance + '.json', { instance: state.host.instance, mcp: true })
        await $.fs.write(state.directory + '/ready-' + state.host.instance + '.done', '')
        state.ready = true
      }
      const present = (await controlNames($, state.directory + '/inbox')).sort()
      state.handled = new Set([...state.handled].filter((file) => present.includes(file)))
      for (const file of present) {
        if (!file.endsWith('.json') || state.handled.has(file)) continue
        const request = await readReport($, state.directory + '/inbox/' + file.slice(0, -5))
        if (request.instance !== state.host.instance && request.handoff_contract !== 1) continue
        state.handled.add(file)
        // The scanner alone is serialized. A command waiting in the host must
        // not hold the scanner, offers, config writes or question answers.
        const pending = controlReply($, state, file, request)
        state.pending.set(file, pending)
        void pending.catch((error) => {
          $.ui.log(String(error), { to: 'debug' })
          controlRetry($, state)
        }).finally(() => state.pending.delete(file))
      }
    } while (state.again)
  } catch (error) {
    $.ui.log('World controls: ' + String(error), { to: 'debug' })
    controlRetry($, state)
  } finally {
    state.looking = false
  }
}

async function controlRelease($, state, file, request) {
  const releasing = $.process.spawn({ argv: [FASTCTX, 'claude-control-claim', WORLD],
    input: JSON.stringify({ key: file.slice(0, -5), host: state.host, controller: request.controller, release: true }) })
  let stdout = '', stderr = ''
  for await (const piece of releasing) {
    if (piece.stream === 'stdout') stdout += piece.text
    else stderr += piece.text
  }
  if ((await releasing.result).code !== 0 || !JSON.parse(stdout).claimed) {
    const error = new Error('The unstarted control claim could not be released: ' + stderr)
    error.uncertain = true
    throw error
  }
}

async function controlReply($, state, file, request) {
  let reply = state.replies.get(file)
  if (reply === undefined) {
    if (state.closed) return
    let finishStarting, entered = false, claimed = false
    const starting = new Promise(resolve => { finishStarting = resolve })
    state.starting.add(starting)
    try {
      const nativeContext = nativeGoalContexts.get(state.host.session)
      if (nativeContext && !nativeContext.busy) {
        try { await clearNativeGoal($, nativeContext) }
        catch (error) { throw controlRefusal(String(error)) }
        if (!nativeContext.nativeGoalCleared) throw controlRefusal('Claude native goal cleanup did not finish; the control was not sent')
      }
      const owner = await $.process.run([FASTCTX, 'claude-control-owner', WORLD])
      if (owner.exitCode !== 0) throw new Error('checking the World node: ' + owner.stderr)
      if (!controlSame(JSON.parse(owner.stdout), request.controller)) throw new Error('the World node that requested this control exited')
      if (request.handoff_contract === 1) {
        if (state.closed) return
        const claiming = $.process.spawn({ argv: [FASTCTX, 'claude-control-claim', WORLD],
          input: JSON.stringify({ key: file.slice(0, -5), host: state.host, controller: request.controller }) })
        let stdout = '', stderr = ''
        for await (const piece of claiming) {
          if (piece.stream === 'stdout') stdout += piece.text
          else stderr += piece.text
        }
        if ((await claiming.result).code !== 0) {
          state.handled.delete(file)
          controlRetry($, state)
          $.ui.log('claiming the Claude control: ' + stderr, { to: 'debug' })
          return
        }
        if (!JSON.parse(stdout).claimed) { state.handled.delete(file); return }
        claimed = true
        if (state.closed) { await controlRelease($, state, file, request); return }
      }
      if (state.closed) return
      entered = true
      const executing = controlExecute($, state, { ...request.control, controller: request.controller, receipt: request.handoff_contract === 1 ? file.slice(0, -5) : undefined }, request.handoff_contract === 1)
      // These commands can enter session.end while their own call is pending.
      if (request.control.do === 'command' && ['exit', 'quit', 'reload-plugins'].includes(request.control.name)) finishStarting()
      reply = { ok: await executing }
    } catch (error) {
      // qp() rejects these native entry points before invoking the operation.
      // A session can unbind just before its session.end hook reaches us.
      const unbound = String(error).includes('is not available in this mode: no session is bound in this process')
        && /\$\.(config\.(list|set)|command\.(list|run)|session\.(model|usage|messages|compact|append)|model\.(fork|complete)|agent\.list) is not available/.test(String(error))
      if (claimed && unbound) { await controlRelease($, state, file, request); return }
      if (state.closed && !entered && error.uncertain !== true) return
      reply = { error: String(error) }
      if (request.handoff_contract === 1 && error.notRun === true) reply.not_run = true
      if (request.handoff_contract === 1 && error.uncertain === true) reply.uncertain = true
    } finally {
      finishStarting()
      state.starting.delete(starting)
    }
  }
  // A failed publication retries only publication, never the host operation.
  state.replies.set(file, reply)
  const path = state.directory + '/replies/' + file.slice(0, -5)
  try {
    await writeReport($, path, JSON.stringify(reply))
    await $.fs.write(path + '.done', '')
    state.replies.delete(file)
  } catch (error) {
    state.handled.delete(file)
    throw error
  }
}

async function controlExecute($, state, control, handoff = false) {
  if (state.closed) throw new Error('this Claude conversation ended')
  if (control.do === 'usage') return $.session.usage({ breakdown: 'summary' })
  if (control.do === 'goal_compact') {
    const automatic = (await $.config.list()).find((row) => row.key === 'autoCompact')
    if (!automatic) return { kind: 'refused', said: 'Claude did not report its automatic compaction setting.' }
    if (automatic.value === false) return { kind: 'refused', said: 'Automatic compaction is disabled.' }
    return controlCompact($, state, undefined, control)
  }
  if (control.do === 'offers') {
    const commands = await controlCatalog($, state)
    const rows = await $.config.list()
    const usage = await $.session.usage({ breakdown: 'summary' })
    // Availability is independent of the current turn or process lifetime.
    const controls = ['command', 'compact', 'model', 'agents', 'agent_messages']
    if (commands.some((command) => command.name === 'rename')) controls.push('rename')
    const offered = {
      checkpoints: Object.entries(state.checkpoints.turns).map(([turn, point]) => ({ turn,
        rewind: false, fork: point.after > 0 && point.after_uuid != null })),
      history_limits: { files_restored: false, compacted_turns_reachable: false, empty_prefix_allowed: false, complete_prefix: state.checkpoints.full === true },
      controls, commands: commands.map((command) => ({ name: command.name, description: command.description,
        args: command.args, argument_forms: command.argument_forms, arguments: command.arguments, source: command.source })),
      models: modelOffers(rows),
      model: state.selection.model ?? await $.session.model(), effort: state.selection.effort ?? (typeof state.observedEffort === 'string' ? state.observedEffort : null), service_tier: null,
      automatic_model: state.automaticModel ? state.routedModel ?? state.routing?.model ?? null : state.routing?.model ?? null,
      usage: {
        context: usage.context.tokens === undefined ? null : { used: usage.context.tokens, window: usage.context.window },
        limits: usage.rateLimits.map((limit) => ({ name: limit.kind, used_percent: limit.percentUsed, resets_at: limit.resetsAt ?? null })),
        cost_usd: usage.cost?.usd ?? null, raw: usage,
      },
    }
    return offered
  }
  if (control.do === 'compact') {
    return controlCompact($, state, undefined, control)
  }
  if (control.do === 'command') {
    return controlCommand($, state, control)
  }
  if (control.do === 'rename') {
    if (!control.name.trim()) throw new Error('a conversation name cannot be empty')
    return controlCommand($, state, { name: 'rename', args: control.name, receipt: control.receipt })
  }
  if (control.do === 'model') {
    if (control.model != null || control.effort != null) return controlSelection($, state, control.model, control.effort)
    const offers = await controlExecute($, state, { do: 'offers' })
    return { kind: 'model_menu', models: offers.models, model: offers.model, effort: offers.effort, service_tier: null }
  }
  if (control.do === 'rewind') return controlRewind($, state, control)
  if (control.do === 'selection') return { kind: 'selected', model: state.selection.model ?? null, effort: state.selection.effort ?? null }
  if (control.do === 'checkpoint') {
    if (state.busy !== false) throw new Error('Claude Code can fork from a turn only between turns')
    if (control.through === state.completedTurn) await controlCheckpoint($, state, control.through, false)
    const count = state.checkpoints.turns[control.through]?.after
    if (count === undefined) throw new Error('this turn has no retained Claude message checkpoint')
    if (state.checkpoints.full !== true) throw new Error('this session has no fully observed message prefix')
    return { hashes: state.checkpoints.messages.slice(0, count), retained_until: state.checkpoints.turns[control.through].after_uuid }
  }
  if (control.do === 'checkpoint_export') {
    if (handoff) return controlExportCheckpoint($, state)
    await controlCheckpoint($, state, null, true)
    return state.checkpoints.full === true ? state.checkpoints : null
  }
  if (control.do === 'fork_prepare') {
    // An idle CLI fork keeps its history in memory until the first new row.
    // A native system notice persists it without a prompt or model request.
    const result = await $.session.append({ message: { type: 'system', content: [{ type: 'text', text: 'Conversation forked in the World.' }] } })
    if (result.deny !== undefined) throw new Error(result.deny)
    return { kind: 'done', said: null }
  }
  if (control.do === 'checkpoint_import') {
    const retained = handoff ? await controlRetained($, state) : null
    const current = retained?.messages ?? await $.session.messages()
    const hashes = retained?.hashes ?? await messageFingerprints(current)
    if (retained?.policy.synthetic_tail === true
      && retained.policy.retained_messages === control.checkpoints.messages.length + 1) {
      const inherited = hashes.slice(0, -1)
      const expectedPrefix = control.checkpoints.messages.slice(-inherited.length)
      if (inherited.every((hash, index) => expectedPrefix[index] === hash)) {
        control.checkpoints.messages.push(hashes[hashes.length - 1])
        control.checkpoints.uuids.push(null)
      }
    }
    // A compaction's result that no transcript row anchors stays in the
    // source's memory, and a fork, rebuilt from the transcript, does not
    // carry it (2.1.303). Only such leading messages may be missing; the
    // turns are counted from what the fork keeps.
    const missing = control.checkpoints.messages.length - hashes.length
    if (current.length < 4096 && missing > 0 && control.checkpoints.uuids?.length === control.checkpoints.messages.length
      && control.checkpoints.uuids.slice(0, missing).every((uuid) => uuid == null)) {
      control.checkpoints = checkpointsFrom(control.checkpoints, missing)
    }
    const expected = control.checkpoints.messages
    const expectedTail = current.length >= 4096 ? expected.slice(-hashes.length) : expected
    if (hashes.length !== expectedTail.length || !hashes.every((hash, index) => expectedTail[index] === hash)) throw new Error('the fork does not carry the same retained message context')
    let observed = retained?.policy
    if (observed === undefined) {
      const policy = $.process.spawn({ argv: [FASTCTX, 'claude-session-policy', WORLD, state.host.session, '--messages', String(expected.length)],
        input: JSON.stringify({ tail: current.map(messageProjection) }) })
      let stdout = '', stderr = ''
      for await (const piece of policy) { if (piece.stream === 'stdout') stdout += piece.text; else stderr += piece.text }
      if ((await policy.result).code !== 0) throw new Error('reading the fork context: ' + stderr)
      observed = JSON.parse(stdout)
    }
    if (observed.context_uuids == null || observed.context_uuids.length !== expected.length) throw new Error('the fork has not persisted its retained context')
    state.checkpoints = control.checkpoints
    for (const point of Object.values(state.checkpoints.turns)) {
      point.before_uuid = point.before > 0 ? observed.context_uuids[point.before - 1] : null
      point.after_uuid = point.after > 0 ? observed.context_uuids[point.after - 1] : null
    }
    state.lastUuid = observed.last_uuid
    state.lastRawUuid = observed.context_tail
    state.checkpoints.last_uuid = state.lastUuid
    state.checkpoints.last_raw_uuid = state.lastRawUuid
    state.checkpoints.uuids = observed.context_uuids
    await saveCheckpoints($, state)
    return { kind: 'done', said: null }
  }
  if (control.do === 'rebased') {
    state.lastUuid = control.last_uuid
    state.lastRawUuid = control.context_tail
    state.checkpoints.uuids = control.uuids
    state.checkpoints.last_uuid = state.lastUuid
    state.checkpoints.last_raw_uuid = state.lastRawUuid
    delete state.checkpoints.compacted_parent
    await saveCheckpoints($, state)
    return { kind: 'done', said: null }
  }
  if (control.do === 'agents') return { kind: 'agents', agents: await $.agent.list() }
  if (control.do === 'agent_messages') {
    const messages = await $.session.messages({ agentId: control.agent_id, as: 'api' })
    if (!Array.isArray(messages)) return { kind: 'refused', said: messages.deny }
    return { kind: 'agent_messages', agent_id: control.agent_id, messages, recent_only: true }
  }
  throw new Error('Claude Code has no mod control for ' + control.do)
}

async function controlQuestion($, state, name, params, call) {
  const key = 'r-' + String(++state.sequence)
  const agent = params.agentId ?? params.agent_id ?? call?.agentId
  const request = { key, host: state.host, turn: agent === undefined && state.busy === true ? state.turn : null,
    request: name, params }
  if (call !== undefined) call.requests.push(key)
  let child
  try {
    child = $.process.spawn({ argv: [FASTCTX, 'claude-request', state.directory], input: JSON.stringify(request) })
    if (call !== undefined) {
      call.children ??= new Set()
      call.children.add(child)
    }
    let stdout = '', stderr = ''
    for await (const piece of child) {
      if (piece.stream === 'stdout') stdout += piece.text
      else stderr += piece.text
    }
    const result = await child.result
    if (result.code !== 0) throw new Error('the World could not receive an answer: ' + stderr)
    const reply = JSON.parse(stdout)
    if (reply.kind === 'pass') worldNotice($, 'fastctx left ' + name + ' to Claude Code: ' + reply.reason)
    return reply
  } finally {
    if (call !== undefined) {
      call.children?.delete(child)
      call.requests = call.requests.filter((held) => held !== key)
    }
  }
}

async function drawnSomewhere($) {
  try {
    return (await $.session.surfaces()).length > 0
  } catch (error) {
    worldFailure($, 'session.surfaces', error)
    return false
  }
}

async function controlCancelCall($, state, call) {
  // A native answer must release the helper even when cancellation files
  // cannot be written. Stream.return stops this call's own child directly.
  for (const child of call.children ?? []) {
    try { await child.return() } catch (error) { worldFailure($, 'request cancellation', error) }
  }
  for (const key of [...call.requests]) {
    try {
      await controlWrite($, state.directory + '/requests/' + state.host.instance + '-' + key + '/cancel.json',
        { reason: 'the tool call finished in Claude Code' })
    } catch (error) { worldFailure($, 'request cancellation', error) }
  }
}

async function startControls($) { await controlBegin($, sessions) }

async function startControlTurn($, e) {
    if (e.agentId !== undefined) return
    const state = await controlBegin($, sessions)
    if (state === null) return
    // The choices this turn runs with are read as it starts: the node keeps
    // one made while the session was not running in the same files.
    await state.selecting
    const files = await controlNames($, state.directory)
    if (files.includes('selection.json')) state.selection = JSON.parse(await $.fs.read(state.directory + '/selection.json')) ?? {}
    if (files.includes('routing.json')) state.routing = JSON.parse(await $.fs.read(state.directory + '/routing.json'))
    state.busy = true
    state.turn = e.turnId
    state.engineTurn = e.turnId
    state.engineModel = undefined
    state.automaticModel = state.routing !== null
    state.routedModel = state.routing?.model
    turns.set(e.turnId, state)
    if (state.promptCheckpoint !== undefined && state.promptCheckpoint !== null) {
      state.checkpoints.turns[e.turnId] = { before: state.promptCheckpoint, before_uuid: state.promptUuid ?? null }
      delete state.promptCheckpoint
      await saveCheckpoints($, state)
    }
    await controlWrite($, state.directory + '/activity.json', { instance: state.host.instance, turn: state.turn })
}

async function completeControlTurn($, e) {
    if (e.agentId !== undefined) return
    const state = turns.get(e.turnId) ?? [...sessions.values()].find((entry) => entry.turn === e.turnId)
    if (state) {
      state.completedTurn = e.turnId
      await controlCheckpoint($, state, e.turnId, false)
      await controlWrite($, state.directory + '/ended/' + encodeURIComponent(e.turnId) + '.json', { instance: state.host.instance })
      if (state.turn === e.turnId) {
        state.busy = false
        state.turn = null
        await controlWrite($, state.directory + '/activity.json', { instance: state.host.instance, turn: null })
      }
    }
}

async function endControls($, e) {
    nativeGoalContexts.delete(e.sessionId)
    const state = sessions.get(e.sessionId)
    if (state) {
      state.closed = true
      state.timer?.cancel()
      // Native precondition refusals release their claim before its witness
      // ends. Self-ending commands remove themselves before invoking the end.
      await Promise.all([...state.starting])
      try {
        await controlWrite($, state.directory + '/closed/' + state.host.instance + '.json', { reason: e.reason })
      } finally {
        await state.guard.return()
      }
    }
}

async function controlCall($, e, next) {
    const state = await controlBegin($, sessions)
    if (state === null) return next(e)
    const { tool, tool_use_id, agentId } = e
    const call = { tool, agentId, requests: [] }
    state.calls.set(tool_use_id, call)
    try {
      return await next(e)
    } finally {
      call.finished = true
      state.calls.delete(tool_use_id)
      await controlCancelCall($, state, call)
    }
}

async function controlPermission($, e, next) {
    const state = await controlBegin($, sessions)
    if (state === null) return next(e)
    const verdict = await next(e)
    if (verdict.decision !== 'ask' || e.tool_use_id === undefined || e.ceiling === 'ask') return verdict
    // A session drawn in a window of Claude Code's own, a terminal, its app,
    // an editor or a phone, asks there. Its permission dialog is drawn by the
    // engine alone, so the World cannot answer it alongside, and holding the
    // ask for the World would leave that window waiting on nothing. Only a
    // session drawn nowhere, as one the World runs in the background, is asked
    // in the World.
    if (await drawnSomewhere($)) return verdict
    const call = state.calls.get(e.tool_use_id) ?? { tool: e.tool, agentId: e.agentId, requests: [] }
    try {
      const reply = await controlQuestion($, state, 'tool.check', { ...e, verdict }, call)
      return reply.kind === 'answer' ? reply.answer : verdict
    } finally { await controlCancelCall($, state, call) }
}
