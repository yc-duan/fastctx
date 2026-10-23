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

async function controlRunNative($, state, command, args) {
  if (state.nativeCommand !== undefined) {
    return { kind: 'refused', said: 'Claude Code is still awaiting /' + state.nativeCommand + '; its public command API has no cancellation handle.' }
  }
  state.nativeCommand = command
  try { return controlCommandResult(await $.command.run({ command, args })) }
  finally { delete state.nativeCommand }
}

async function controlCompact($, state, instructions) {
  if (state.busy === true) throw new Error('Claude Code can compact only between turns')
  if (state.compacting) throw new Error('another context operation is still running')
  state.compacting = true
  try {
    const parent = state.lastRawUuid
    const result = await $.session.compact(instructions === undefined ? undefined : { instructions })
    if (result.skip !== undefined) return { kind: 'refused', said: result.skip }
    // The host skips this plugin's own compact hook for this public method.
    await compactedCheckpoint($, state, result.messages, parent)
    if (state.busy === null) { state.busy = false; state.turn = null }
    return { kind: 'done', said: null }
  } finally { state.compacting = false }
}

async function controlConfig($, args) {
  const rows = await $.config.list()
  if (!args.trim()) return { kind: 'done', said: JSON.stringify(rows) }
  // One row per call preserves host denials and partial-effect attribution.
  const equal = args.indexOf('=')
  if (equal < 1) throw new Error('/config takes <key>=<value>')
  const key = args.slice(0, equal).trim()
  const raw = args.slice(equal + 1).trim()
  const row = rows.find((row) => row.key === key)
  if (!row) throw new Error('Claude Code has no configuration row ' + key)
  let value
  if (row.kind === 'boolean') {
    if (raw !== 'true' && raw !== 'false') throw new Error(key + ' takes true or false')
    value = raw === 'true'
  } else if (row.kind === 'number') {
    value = Number(raw)
    if (!raw || !Number.isFinite(value)) throw new Error(key + ' takes a number')
  } else value = raw
  const result = await $.config.set({ key, value })
  return result.deny !== undefined ? { kind: 'refused', said: result.deny }
    : { kind: 'done', said: JSON.stringify({ key, value: result.value }) }
}

async function controlCommand($, state, control) {
  if (await assistedCommand($, control.name)) throw new Error(assistedCommandReason)
  if (control.path !== undefined && control.path !== null) {
    throw new Error('Claude Code does not expose skill paths or a path-selecting command API')
  }
  const command = (await controlCatalog($, state)).find((entry) => entry.name === control.name)
  if (!command) throw new Error('/' + control.name + ' is not an unambiguous background-safe command in this Claude Code version')
  const args = control.args
  if (command.arguments === 'required' && !args.trim()) throw new Error('/' + command.name + ' requires arguments')
  if (command.arguments === 'none' && args.trim()) throw new Error('/' + command.name + ' takes no arguments')
  if (command.route === 'model') return controlSelection($, state, args.trim(), null)
  if (command.route === 'config') return controlConfig($, args)
  if (command.route === 'compact') return controlCompact($, state, args)
  if (command.route === 'side_answer') return { kind: 'side_answer', reply: await $.model.fork({ prompt: args }) }
  if (command.route === 'editor' || command.route === 'output_style') {
    const key = command.route === 'editor' ? 'editor' : 'outputStyle'
    const row = (await $.config.list()).find((row) => row.key === key)
    if (row === undefined) throw new Error('Claude Code has no configuration row ' + key)
    const value = command.route === 'editor' && !args.trim() ? (row.value === 'vim' ? 'normal' : 'vim') : args.trim()
    const result = await $.config.set({ key, value })
    return result.deny !== undefined ? { kind: 'refused', said: result.deny } : { kind: 'done', said: null }
  }
  if (command.route === 'effort' && !['status', 'current', 'help', '-h', '--help'].includes(args.trim().toLowerCase())
      && !/^ultracode(?:\s+(?:on|off))?$/i.test(args.trim())) {
    throw new Error('This effort change may require a Claude Code terminal confirmation; use ' + command.args)
  }
  if (command.route === 'fast') {
    if (!['on', 'off'].includes(args.trim().toLowerCase())) throw new Error('/fast requires on or off')
    const result = await $.config.set({ key: 'fast', value: args.trim().toLowerCase() === 'on' })
    return result.deny !== undefined ? { kind: 'refused', said: result.deny } : { kind: 'done', said: null }
  }
  if (command.route === 'mcp' && !/^(enable|disable|reconnect)\s+\S/.test(args.trim())) throw new Error('/mcp requires an operation and server')
  if (command.route === 'import' && !/(^|\s)--yes(?:=\S+)?(?:\s|$)/.test(args)) throw new Error('/import requires --yes to finish in the background')
  if (command.route === 'plugin' && !/^(help|--help|-h|list|ls|validate|tag)(\s|$)|^(marketplace|market)\s+list(?:\s|$)/i.test(args.trim())) throw new Error('this plugin operation requires a terminal')
  if (command.route === 'design' && /^(sync|login|consent|revoke)$/i.test(args.trim())) throw new Error('this design subcommand requires a terminal')
  if (command.route === 'review' && /^ultra(?:\s|$)/i.test(args.trim())) throw new Error('ultrareview requires a terminal confirmation')
  if (command.route === 'plan' && /^open$/i.test(args.trim())) throw new Error('opening the plan requires a local editor')
  return controlRunNative($, state, command.name, args)
}
