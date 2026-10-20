// Background-safe command knowledge, read from Claude Code 2.1.294 and each
// entry run as the interface runs it in a 2.1.303 background session
// (2026-10-20). Offsets below are zero-based characters in 2.1.294's
// decoded Bun modules, not byte offsets. Left out: what waits on an
// interactive picker (keybindings), what moves the conversation to another
// session the World then loses (branch, fork, clear, reset, new; the
// interface forks and starts conversations itself), and what opens a
// browser on the agent's machine (stickers, radio).
// command.describe supplies the host's final hints; list has no execution type.
// Unknown built-ins and unknown plugin registrations are not assumed safe.
const commandDescriptions = new Map()
const commandPolicies = {
  // exe-230444374.js:3664 I3n, 4807 ROs. Empty arguments generate a name.
  rename: { arguments: 'optional', args: '[name]', route: 'native' },
  // Request overrides leave the user's saved host preference alone.
  model: { arguments: 'required', args: '<model>', route: 'model' },
  config: { arguments: 'optional', args: '[<key>=<value>]', route: 'config' },
  // exe-249103290.js:10230 At; exe-212767153.js:1504/3077. Empty => picker.
  goal: { arguments: 'required', args: '<condition> | clear', route: 'native' },
  // exe-249514666.js:18956 io; exe-231223233.js:1968 J$t. Empty => picker.
  autocompact: { arguments: 'required', args: 'auto | <100k–1M tokens>', route: 'native' },
  // exe-249257650.js:17253 wo; exe-212711171.js:13242 zDe. Other effort
  // changes can wait on a cost confirmation that the public API cannot close.
  effort: { arguments: 'required', args: 'status | current | help | ultracode [on|off]', route: 'effort' },
  // exe-249156246.js:28688 In serializes a report then completes.
  context: { arguments: 'optional', args: '[all]', route: 'native' },
  // exe-230416942.js:13002 v returns a local text result.
  'reload-skills': { arguments: 'none', args: null, route: 'native' },
  // d.ts:2836 session.compact has instructions and an explicit skip result.
  compact: { arguments: 'optional', args: '[instructions]', route: 'compact' },
  advisor: { arguments: 'required', args: '<model> | off | unset', route: 'native' },
  fast: { arguments: 'required', args: 'on | off', route: 'fast' },
  mcp: { arguments: 'required', args: 'enable | disable | reconnect <server>', route: 'mcp' },
  import: { arguments: 'required', args: '<source> --yes', route: 'import' },
  export: { arguments: 'required', args: '<filename>', route: 'native' },
  plugin: { arguments: 'required', args: 'help | list | validate <directory>', route: 'plugin' },
  plugins: { arguments: 'required', args: 'help | list | validate <directory>', route: 'plugin' },
  marketplace: { arguments: 'required', args: 'help | list | validate <directory>', route: 'plugin' },
  design: { arguments: 'optional', args: '<brief> | types | hub', route: 'design' },
  vim: { arguments: 'optional', args: null, route: 'editor' },
  'output-style': { arguments: 'required', args: '<style>', route: 'output_style' },
  plan: { arguments: 'required', args: '<description>', route: 'plan' },
  btw: { arguments: 'required', args: '<question>', route: 'side_answer' },
  ultraplan: { arguments: 'none', args: null, route: 'native' },
  'code-review': { arguments: 'optional', args: '[target] [effort]', route: 'review' },
  review: { arguments: 'optional', args: '[target] [effort]', route: 'review' },
}

for (const name of ['color', 'design-consent', 'design-revoke', 'install-slack-app', 'voice',
  'terminal-setup', 'tui', 'agents', 'reload-plugins', 'heapdump', 'exit', 'quit', 'list-agents',
  'peers', 'brief', 'low-priority', 'focus', 'claim-credit']) {
  commandPolicies[name] = { arguments: 'optional', args: null, route: 'native' }
}
for (const name of ['background', 'bg', 'stop']) commandPolicies[name] = { arguments: 'optional', args: null, route: 'native' }
commandPolicies.subtask = { arguments: 'required', args: '<task>', route: 'native' }

// Prompt expanders inspected at exe-226157729.js:231528 U8e and their
// registrars. Commands with panel subcommands have separate argument policies.
const promptCommands = new Map([
  // exe-226157729.js registrars; null means no argumentHint was defined.
  ['design-sync', '[<project hint, e.g. "Acme DS">]'], // 116892
  ['dataviz', null], // 106356
  ['artifact-diagramming', null], // 30742
  ['workshop', null], // 28514
  ['prototype', null], // 193032
  ['whiteboard', null], // 190745
  ['artifact-report', null], ['artifact-data-table', null], ['artifact-explainer', null], // 33677
  ['artifact-components', null], // 29447
  ['doc', null], // 187939
  ['artifact-pr-review', '[pr number or url]'], // 198139
  ['artifact-capabilities', null], // 25441
  ['update-config', null], // 229162
  ['verify', null], // 231136
  ['debug', '[issue description]'], // 106911
  ['commit', '[guidance]'], // 103677
  ['pr', '[guidance]'], // 196421
  ['simplify', '[<target>]'], // 201761
  ['batch', '<instruction>'], // 40806; empty input only returns usage guidance.
  ['fewer-permission-prompts', null], // 176415
  ['doctor', '[prompt-audit [<path>]]'], ['checkup', '[prompt-audit [<path>]]'], // 166281
  ['explain-usage', null], // 166906
  ['slides', '[what the deck is about]'], // exe-226127618.js:2979
  ['setup-claude', null], ['setup-cowork', null], // exe-243810416.js:6403
  ['loop', '[interval] [prompt]'], ['proactive', '[interval] [prompt]'], // exe-243817263.js:23135
  ['schedule', null], ['routines', null], // exe-243841148.js:32283
  ['claude-api', null], // exe-226137934.js:11655
  ['claude-code-docs', '[question]'], // exe-243885648.js:11427
  ['workflow-authoring', null], // exe-239978437.js:914
  ['run', null], // exe-243796297.js:6644
  ['run-skill-generator', null], // exe-243803384.js:6425
  ['init', null], // exe-216985381.js:1175337
  ['commit-push-pr', null], // exe-216985381.js:1145148
  ['team-onboarding', null], // exe-216985381.js:1193557
  ['security-review', null], // exe-216985381.js:1214250
  ['statusline', null], // exe-216985381.js:1654569
  ['insights', null], ['plugin-authoring', null],
])

async function controlCatalog($, state) {
  const commands = await $.command.list()
  const counts = new Map()
  for (const command of commands) counts.set(command.name, (counts.get(command.name) ?? 0) + 1)
  const offered = []
  for (const command of commands) {
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
    if (command.name === 'compact' && state.busy !== false) continue
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
    const result = await $.session.compact(instructions === undefined ? undefined : { instructions })
    if (result.skip !== undefined) return { kind: 'refused', said: result.skip }
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
