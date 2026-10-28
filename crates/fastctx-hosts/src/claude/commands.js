// Background-safe command knowledge, read from Claude Code 2.1.294 and each
// entry run as the interface runs it in a 2.1.303 background session
// (2026-10-20). The policy table is shared with the node. Left out: what waits on an
// interactive picker (keybindings), what moves the conversation to another
// session the World then loses (branch, fork, clear, reset, new; the
// interface forks and starts conversations itself), and what opens a
// browser on the agent's machine (stickers, radio).
// command.describe supplies the host's final hints; list has no execution type.
// Unknown built-ins and unknown plugin registrations are not assumed safe.
const commandDescriptions = new Map()

function controlRefusal(message) {
  const error = new Error(message)
  error.notRun = true
  return error
}
// Shared with the node's catalog for sessions that have not reported yet.
const commandCatalog = __FASTCTX_COMMAND_CATALOG__
const commandPolicies = commandCatalog.policies
const promptCommands = new Map(Object.entries(commandCatalog.prompts))

async function controlCatalog($, state) {
  const assisting = await assistedSession($)
  const commands = await $.command.list()
  const counts = new Map()
  for (const command of commands) counts.set(command.name, (counts.get(command.name) ?? 0) + 1)
  const offered = []
  for (const command of commands) {
    if (command.name === 'goal') continue
    if (assisting && (command.source !== 'builtin' || !assistedCommands.has(command.name))) continue
    // No public path executor exists. Ambiguous names cannot be advertised
    // with a fabricated identity that $.command.run cannot honor.
    if (counts.get(command.name) !== 1) continue
    let policy
    if (command.source === 'builtin') {
      // Known by name in every version the World works with; a built-in
      // this table does not know is left out until it is checked.
      policy = commandPolicies[command.name]
      if (!policy && promptCommands.has(command.name)) {
        policy = { arguments: command.name === 'batch' ? 'required' : 'optional', args: promptCommands.get(command.name), route: 'native' }
      }
    } else if (command.source === 'user' || command.source === 'mcp') {
      // 2.1.294's YY source projection maps Markdown skills/old commands to
      // user and generated MCP prompt commands to mcp (exe-216985381.js).
      policy = { arguments: 'optional', args: '[arguments]', route: 'native' }
    }
    // plugin source also includes $.command.register local implementations.
    // The public catalog supplies no proof that an unknown one is a prompt.
    if (!policy) continue

    const described = commandDescriptions.get(state.host.session)?.get(command.name)
    if (described?.isHidden) continue
    offered.push({ ...command, ...policy, description: described?.description ?? command.description,
      args: described?.argumentHint ?? null, argument_forms: policy.args === null ? [] : [policy.args] })
  }
  return offered
}

function controlCommandResult(result) {
  // In 2.1.294 local-jsx outputs live in transcript messages, not resultText
  // (exe-237744330.js:21435/27600). Missing exitCode is no success evidence.
  if (result.exitCode !== undefined && result.exitCode !== 0) {
    return { kind: 'refused', said: result.text ?? 'Claude Code reported exit code ' + result.exitCode }
  }
  return { kind: 'reported', said: result.text ?? null }
}

async function controlStarted($, state, receipt, action) {
  if (receipt === undefined) return
  const path = state.directory + '/started/' + receipt
  await controlWrite($, path + '.json', { instance: state.host.instance, action })
  await $.fs.write(path + '.done', '')
}

async function controlRunNative($, state, command, args, receipt) {
  if (state.nativeCommand !== undefined) {
    return { kind: 'refused', said: 'Claude Code is still awaiting /' + state.nativeCommand + '; its public command API has no cancellation handle.' }
  }
  state.nativeCommand = command
  try {
    await controlStarted($, state, receipt, command)
    return controlCommandResult(await $.command.run({ command, args }))
  }
  finally { delete state.nativeCommand }
}

async function controlCompactRefresh($, state, control) {
  const refresh = $.process.spawn({ argv: [FASTCTX, 'claude-control-claim', WORLD],
    input: JSON.stringify({ key: control.receipt, host: state.host, controller: control.controller, refresh: true }) })
  let stdout = '', stderr = ''
  for await (const piece of refresh) {
    if (piece.stream === 'stdout') stdout += piece.text
    else stderr += piece.text
  }
  if ((await refresh.result).code !== 0 || !JSON.parse(stdout).claimed) {
    const error = new Error('The compact claim could not be refreshed: ' + stderr)
    error.uncertain = true
    throw error
  }
}

async function controlCompact($, state, instructions, control) {
  if (state.busy === true) throw controlRefusal('Claude Code can compact only between turns')
  if (state.compacting) throw controlRefusal('another context operation is still running')
  state.compacting = true
  const compactingSession = await $.session.id()
  compactedByWorld.add(compactingSession)
  try {
    const until = Date.now() + 120000
    let result, parent
    for (;;) {
      parent = state.lastRawUuid
      try {
        result = await $.session.compact(instructions === undefined ? undefined : { instructions })
        break
      } catch (error) {
        // These native guards throw before any summary request or effect.
        if (String(error).endsWith('$.session.compact: Not enough messages to compact.')) {
          return { kind: 'reported', said: 'Not enough messages to compact.' }
        }
        if (!String(error).endsWith('$.session.compact: a turn is in flight; the conversation compacts between turns')) throw error
        if (control?.receipt === undefined || Date.now() >= until) throw controlRefusal(String(error))
        await controlCompactRefresh($, state, control)
      }
    }
    if (result.skip !== undefined) return { kind: 'refused', said: result.skip }
    // The host skips this plugin's own compact hook for this public method.
    await compactedCheckpoint($, state, result.messages, parent)
    if (state.busy === null) { state.busy = false; state.turn = null }
    return { kind: 'done', said: null }
  } finally {
    state.compacting = false
    compactedByWorld.delete(compactingSession)
  }
}

const sideAnswerInstruction = "The user asks a side question about the conversation below without adding it to that conversation. Answer it from the conversation. Do not continue the conversation's work and do not call tools."
// How much of the retained conversation a side question carries, the latest kept.
const SIDE_ANSWER_CHARS = 300000

function sideAnswerClip(text, limit) {
  return text.length > limit ? text.slice(0, limit) + ' [...]' : text
}

function sideAnswerResult(content) {
  if (typeof content === 'string') return content
  if (!Array.isArray(content)) return ''
  return content.map((block) => block?.type === 'text' ? block.text ?? '' : block?.type === 'image' ? '(an image)' : '').filter(Boolean).join('\n')
}

// The retained conversation as a model reads a transcript: each one's words in
// order, a tool's use and its result in brief, a picture named. Too long, its
// earliest part is left out, and that is said.
function sideAnswerTranscript(messages) {
  const lines = []
  for (const message of Array.isArray(messages) ? messages : []) {
    const who = message?.role === 'assistant' ? 'Assistant' : 'User'
    const blocks = typeof message?.content === 'string' ? [{ type: 'text', text: message.content }] : Array.isArray(message?.content) ? message.content : []
    for (const block of blocks) {
      // The host's own reminders inside a user's message are not the conversation.
      const said = block?.type === 'text' ? (block.text ?? '').replace(/<system-reminder>[\s\S]*?<\/system-reminder>/g, '').trim() : ''
      if (said) lines.push(who + ': ' + said)
      else if (block?.type === 'tool_use') lines.push('Assistant used ' + block.name + ': ' + sideAnswerClip(JSON.stringify(block.input ?? {}), 400))
      else if (block?.type === 'tool_result') lines.push('Result: ' + sideAnswerClip(sideAnswerResult(block.content).trim(), 800))
      else if (block?.type === 'image') lines.push(who + ': (an image)')
    }
  }
  const text = lines.join('\n\n')
  return text.length > SIDE_ANSWER_CHARS ? '(The earlier part of the conversation is left out.)\n\n' + text.slice(text.length - SIDE_ANSWER_CHARS) : text
}

async function controlSideAnswer($, state, question) {
  const forked = await $.model.fork({ prompt: question })
  if (forked.isAnswered !== false || forked.reason !== 'nothing-to-fork') return forked
  // A resumed process has no cached main request to fork. This native result
  // proves no request ran; complete once against its retained public context.
  const conversation = await $.session.messages({ as: 'api' })
  const reply = await $.model.complete({ model: state.selection.model ?? await $.session.model(),
    ...(state.selection.effort == null ? {} : { effort: state.selection.effort }),
    system: sideAnswerInstruction, prompt: '<conversation>\n' + sideAnswerTranscript(conversation) + '\n</conversation>\n\nSide question: ' + question })
  return { ...reply, context_source: 'retained_conversation' }
}

async function controlConfig($, args) {
  const rows = await $.config.list()
  if (!args.trim()) return { kind: 'done', said: JSON.stringify(rows) }
  // One row per call preserves host denials and partial-effect attribution.
  const equal = args.indexOf('=')
  if (equal < 1) throw controlRefusal('/config takes <key>=<value>')
  const key = args.slice(0, equal).trim()
  const raw = args.slice(equal + 1).trim()
  const row = rows.find((row) => row.key === key)
  if (!row) throw controlRefusal('Claude Code has no configuration row ' + key)
  let value
  if (row.kind === 'boolean') {
    if (raw !== 'true' && raw !== 'false') throw controlRefusal(key + ' takes true or false')
    value = raw === 'true'
  } else if (row.kind === 'number') {
    value = Number(raw)
    if (!raw || !Number.isFinite(value)) throw controlRefusal(key + ' takes a number')
  } else value = raw
  const result = await $.config.set({ key, value })
  return result.deny !== undefined ? { kind: 'refused', said: result.deny }
    : { kind: 'done', said: JSON.stringify({ key, value: result.value }) }
}

async function controlCommand($, state, control) {
  if (await assistedCommand($, control.name)) throw controlRefusal(assistedCommandReason)
  if (control.path !== undefined && control.path !== null) {
    throw controlRefusal('Claude Code does not expose skill paths or a path-selecting command API')
  }
  const command = (await controlCatalog($, state)).find((entry) => entry.name === control.name)
  if (!command) throw controlRefusal('/' + control.name + ' is not an unambiguous background-safe command in this Claude Code version')
  const args = control.args
  if (command.arguments === 'required' && !args.trim()) throw controlRefusal('/' + command.name + ' requires arguments')
  if (command.arguments === 'none' && args.trim()) throw controlRefusal('/' + command.name + ' takes no arguments')
  if (command.route === 'model') return controlSelection($, state, args.trim(), null)
  if (command.route === 'config') return controlConfig($, args)
  if (command.route === 'compact') return controlCompact($, state, args, control)
  if (command.route === 'side_answer') return { kind: 'side_answer', reply: await controlSideAnswer($, state, args) }
  if (command.route === 'editor' || command.route === 'output_style') {
    const key = command.route === 'editor' ? 'editor' : 'outputStyle'
    const row = (await $.config.list()).find((row) => row.key === key)
    if (row === undefined) throw controlRefusal('Claude Code has no configuration row ' + key)
    const value = command.route === 'editor' && !args.trim() ? (row.value === 'vim' ? 'normal' : 'vim') : args.trim()
    const result = await $.config.set({ key, value })
    return result.deny !== undefined ? { kind: 'refused', said: result.deny } : { kind: 'done', said: null }
  }
  if (command.route === 'effort' && !['status', 'current', 'help', '-h', '--help'].includes(args.trim().toLowerCase())
      && !/^ultracode(?:\s+(?:on|off))?$/i.test(args.trim())) {
    throw controlRefusal('This effort change may require a Claude Code terminal confirmation; use ' + command.args)
  }
  if (command.route === 'fast') {
    if (!['on', 'off'].includes(args.trim().toLowerCase())) throw controlRefusal('/fast requires on or off')
    const rows = await $.config.list()
    if (!rows.some((row) => row.key === 'fast')) return controlRunNative($, state, command.name, args, control.receipt)
    const result = await $.config.set({ key: 'fast', value: args.trim().toLowerCase() === 'on' })
    return result.deny !== undefined ? { kind: 'refused', said: result.deny } : { kind: 'done', said: null }
  }
  if (command.route === 'mcp' && !/^(enable|disable|reconnect)\s+\S/.test(args.trim())) throw controlRefusal('/mcp requires an operation and server')
  if (command.route === 'import' && !/(^|\s)--yes(?:=\S+)?(?:\s|$)/.test(args)) throw controlRefusal('/import requires --yes to finish in the background')
  if (command.route === 'plugin' && !/^(help|--help|-h|list|ls|validate|tag)(\s|$)|^(marketplace|market)\s+list(?:\s|$)/i.test(args.trim())) throw controlRefusal('this plugin operation requires a terminal')
  if (command.route === 'design' && /^(sync|login|consent|revoke)$/i.test(args.trim())) throw controlRefusal('this design subcommand requires a terminal')
  if (command.route === 'review' && /^ultra(?:\s|$)/i.test(args.trim())) throw controlRefusal('ultrareview requires a terminal confirmation')
  if (command.route === 'plan' && /^open$/i.test(args.trim())) throw controlRefusal('opening the plan requires a local editor')
  return controlRunNative($, state, command.name, args, control.receipt)
}
