// Conversation controls use Claude Code's public methods. Human requests
// wait in a process stream: unlike a Promise or clock.sleep, pulling that
// stream does not consume the hook's ten-second execution budget.
// Embedded into world.js when publishing: the official scanner follows $
// into functions in that file, never across an import.
const sessions = new Map()
const turns = new Map()
const controlStarting = new Map()

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
  const directory = WORLD + '/claude/' + sessionId + '/control'
  await $.fs.write(directory + '/inbox/.created', '')
  await $.fs.write(directory + '/replies/.created', '')
  await $.fs.write(directory + '/requests/.created', '')
  await $.fs.write(directory + '/closed/.created', '')
  const files = await controlNames($, directory)
  const prior = files.includes('host.json') ? JSON.parse(await $.fs.read(directory + '/host.json')) : null
  const guard = $.process.spawn({ argv: [FASTCTX, 'claude-control-guard'] })
  try {
    let header = '', stderr = ''
    while (!header.includes('\n')) {
      const piece = await guard.next()
      if (piece.done) throw new Error('the control lifetime helper ended before identifying itself: ' + stderr)
      if (piece.value.stream === 'stdout') header += piece.value.text
      else stderr += piece.value.text
    }
    const identity = JSON.parse(header)
    const host = {
      session: sessionId, process: identity.process, parent: identity.parent,
      command_contract: 1,
      // Persisted before polling: even a reload in the same clock tick or
      // after a clock rollback gets a different answer address.
      instance: String(Math.max(Date.now(), Number(prior?.instance ?? 0) + 1)),
    }
    const state = { directory, host, closed: false, busy: false, turn: null, looking: false,
      sequence: 0, calls: new Map(), replies: new Map(), handled: new Set(), pending: new Map(), timer: null, guard }
    const closed = await controlNames($, directory + '/closed')
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
    state.timer = $.clock.every(LOOK_MS, () => controlPoll($, state))
    void controlWatchGuard($, state)
    return state
  } catch (error) {
    await guard.return()
    throw error
  }
}

async function controlWatchGuard($, state) {
  try {
    for await (const piece of state.guard) {
      if (piece.stream === 'stderr') $.ui.log('World control lifetime helper: ' + piece.text, { to: 'debug' })
    }
    if (!state.closed) throw new Error('the World control lifetime helper exited')
  } catch (error) {
    if (!state.closed) $.ui.log(String(error), { to: 'debug' })
  } finally {
    // The node observes the child itself. No filesystem call from an
    // already-unloaded module is needed to release its pending controls.
    state.closed = true
    state.timer.cancel()
  }
}

async function controlPoll($, state) {
  if (state.closed || state.looking) return
  if (await removed($)) {
    state.closed = true
    state.timer.cancel()
    await state.guard.return()
    return
  }
  state.looking = true
  try {
    if (await $.session.id() !== state.host.session) return
    const present = (await controlNames($, state.directory + '/inbox')).sort()
    state.handled = new Set([...state.handled].filter((file) => present.includes(file)))
    for (const file of present) {
      if (!file.endsWith('.json') || state.handled.has(file)) continue
      const request = await readReport($, state.directory + '/inbox/' + file.slice(0, -5))
      if (request.instance !== state.host.instance) continue
      state.handled.add(file)
      // The scanner alone is serialized. A command waiting in the host must
      // not hold the scanner, offers, config writes or question answers.
      const pending = controlReply($, state, file, request)
      state.pending.set(file, pending)
      void pending.catch((error) => $.ui.log(String(error), { to: 'debug' }))
        .finally(() => state.pending.delete(file))
    }
  } finally {
    state.looking = false
  }
}

async function controlReply($, state, file, request) {
  let reply = state.replies.get(file)
  if (reply === undefined) {
    try {
      const owner = await $.process.run([FASTCTX, 'claude-control-owner', WORLD])
      if (owner.exitCode !== 0) throw new Error('checking the World node: ' + owner.stderr)
      if (!controlSame(JSON.parse(owner.stdout), request.controller)) throw new Error('the World node that requested this control exited')
      reply = { ok: await controlExecute($, state, request.control) }
    } catch (error) { reply = { error: String(error) } }
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

async function controlExecute($, state, control) {
  if (state.closed) throw new Error('this Claude conversation ended')
  if (control.do === 'offers') {
    const commands = await controlCatalog($, state)
    const rows = await $.config.list()
    const modelRow = rows.find((row) => row.key === 'model')
    const usage = await $.session.usage()
    const controls = ['command']
    if (state.busy === false) controls.push('compact')
    if (modelRow !== undefined) controls.push('model')
    if (commands.some((command) => command.name === 'rename')) controls.push('rename')
    return {
      controls, commands: commands.map((command) => ({ name: command.name, description: command.description,
        args: command.args, arguments: command.arguments, source: command.source })),
      models: (modelRow?.options ?? []).map((id) => ({ id, name: null, description: null,
        efforts: [], default_effort: null, default: id === 'default', service_tiers: [], default_service_tier: null })),
      model: await $.session.model(), effort: null, service_tier: null,
      usage: {
        context: usage.context.tokens === undefined ? null : { used: usage.context.tokens, window: usage.context.window },
        limits: usage.rateLimits.map((limit) => ({ name: limit.kind, used_percent: limit.percentUsed, resets_at: limit.resetsAt ?? null })),
        cost_usd: usage.cost?.usd ?? null, raw: usage,
      },
    }
  }
  if (control.do === 'compact') {
    return controlCompact($, state)
  }
  if (control.do === 'command') {
    return controlCommand($, state, control)
  }
  if (control.do === 'rename') {
    if (!control.name.trim()) throw new Error('a conversation name cannot be empty')
    return controlCommand($, state, { name: 'rename', args: control.name })
  }
  if (control.do === 'model') {
    if (control.effort !== null && control.effort !== undefined) {
      throw new Error('Claude Code exposes no background-safe write of ordinary reasoning effort; its command may require terminal confirmation')
    }
    if (control.model !== null && control.model !== undefined) return controlSetModel($, control.model)
    const offers = await controlExecute($, state, { do: 'offers' })
    return { kind: 'model_menu', models: offers.models, model: offers.model, effort: null, service_tier: null }
  }
  throw new Error('Claude Code has no mod control for ' + control.do)
}

async function controlQuestion($, state, name, params, call, nativeWait = false) {
  const key = 'r-' + String(++state.sequence)
  const agent = params.agentId ?? params.agent_id ?? call?.agentId
  const request = { key, host: state.host, turn: agent === undefined && state.busy === true ? state.turn : null,
    request: name, params, native_wait: nativeWait }
  if (call !== undefined) call.requests.push(key)
  try {
    const child = $.process.spawn({ argv: [FASTCTX, 'claude-request', state.directory], input: JSON.stringify(request) })
    let stdout = '', stderr = ''
    for await (const piece of child) {
      if (piece.stream === 'stdout') stdout += piece.text
      else stderr += piece.text
    }
    const result = await child.result
    if (result.code !== 0) throw new Error('the World could not receive an answer: ' + stderr)
    return JSON.parse(stdout)
  } finally {
    if (call !== undefined) call.requests = call.requests.filter((held) => held !== key)
  }
}

async function controlCancelCall($, state, call) {
  for (const key of [...call.requests]) {
    await controlWrite($, state.directory + '/requests/' + state.host.instance + '-' + key + '/cancel.json',
      { reason: 'the tool call finished in Claude Code' })
  }
}

async function startControls($) { await controlBegin($, sessions) }

async function startControlTurn($, e) {
    const state = await controlBegin($, sessions)
    if (state === null) return
    state.busy = true
    state.turn = e.turnId
    turns.set(e.turnId, state)
    await controlWrite($, state.directory + '/activity.json', { instance: state.host.instance, turn: state.turn })
}

async function completeControlTurn($, e) {
    if (e.agentId !== undefined) return
    const state = turns.get(e.turnId) ?? [...sessions.values()].find((entry) => entry.turn === e.turnId)
    if (state) {
      await controlWrite($, state.directory + '/ended/' + encodeURIComponent(e.turnId) + '.json', { instance: state.host.instance })
      if (state.turn === e.turnId) {
        state.busy = false
        state.turn = null
        await controlWrite($, state.directory + '/activity.json', { instance: state.host.instance, turn: null })
      }
    }
}

async function endControls($, e) {
    const state = sessions.get(e.sessionId)
    if (state) {
      state.closed = true
      state.timer.cancel()
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
      if (tool === 'AskUserQuestion') {
        // Keep the native path running. Its immediate refusal needs no
        // viewer; if it waits, someone opening World later can still answer.
        // A tool.call hook settling first aborts its pending next() branch.
        const native = next(e)
        const world = controlQuestion($, state, 'AskUserQuestion', e, call, true)
          .then((reply) => reply.kind === 'answer' ? reply.answer : native)
        return await Promise.race([native, world])
      }
      return await next(e)
    } finally {
      state.calls.delete(tool_use_id)
      await controlCancelCall($, state, call)
    }
}

async function controlPermission($, e, next) {
    const state = await controlBegin($, sessions)
    if (state === null) return next(e)
    // This event has no tool_use_id. Only a unique live match can be
    // invalidated when the native surface wins the permission race. An
    // ambiguous request stays with Claude Code's own permission UI.
    // Inputs may have been rewritten by an inner hook, so they cannot
    // identify the invocation when the same tool is running in parallel.
    const matches = [...state.calls.values()].filter((call) => call.tool === e.tool_name && call.agentId === e.agent_id)
    if (matches.length !== 1) return next(e)
    const reply = await controlQuestion($, state, 'classic.PermissionRequest', e, matches[0])
    return reply.kind === 'answer' ? reply.answer : next(e)
}
