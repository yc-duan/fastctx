import { test, expect, mock } from 'claude-code/testing'
import type { On, SessionUsage } from 'claude-code'

function fixtureUi(on: On, lines: string[] = []) {
  on('ui.log', (_, e) => { lines.push(e.text); return { value: undefined } })
  on('ui.status', () => ({ value: undefined }))
  on('ui.toast', () => ({ value: undefined }))
  on('ui.invalidate', () => ({ value: undefined }))
  on('command.list', () => ({ value: [] }))
  return lines
}

// The World's hooks act only in a session it holds (`world-layers.md`): the
// launch settings it starts a session with name its conversation.
const heldByWorld = (_: unknown, e: { source?: string }) => ({
  value: e.source === 'flag' ? { env: { FASTCTX_CONVERSATION: 'fixture-conversation' } } : {},
})
const worldTool = 'mcp__configured_World_alias__run'
// What connect answers; the tools carry its tool-name spelling.
const serverName = 'configured:World alias'
const spelledName = 'configured_World_alias'
function usage(loaded = false): SessionUsage {
  return { startedAt: 0, rateLimits: [], context: { window: 200000, breakdown: {
    categories: [], totalTokens: 0, maxTokens: 200000, rawMaxTokens: 200000,
    autocompactSource: 'model-default', percentage: 0, gridRows: [], model: 'fixture',
    memoryFiles: [], agents: [], isAutoCompactEnabled: false, apiUsage: null,
    mcpTools: [
      { name: worldTool, serverName: spelledName, tokens: 1, isLoaded: loaded },
      { name: 'mcp__github__run', serverName: 'GitHub', tokens: 1, isLoaded: true },
      { name: 'mcp__plugin_fastctx-world_fastctx__lookalike', serverName: 'Other', tokens: 1, isLoaded: true },
    ],
  } } }
}

async function* guard(_$: any, _e: any, next: any) {
  if (_e.argv[1] === 'claude-session-policy') {
    yield { stream: 'stdout' as const, text: JSON.stringify({ model: null, effort: null }) }
    return { value: { code: 0, signal: null } }
  }
  yield { stream: 'stdout' as const, text: JSON.stringify({ process: { pid: 124, token: 'activation' }, parent: { pid: 123, token: 'fixture-lifetime' } }) + '\n' }
  await new Promise<void>((resolve) => next.signal.addEventListener('abort', () => resolve(), { once: true }))
  return { value: { code: 0, signal: null } }
}

// The official loader and engine must admit the published module. A VM-only
// test cannot enforce the scanner's rules or the engine's stream lifetime.
test('loads, streams without waiting for disk, and retains the old session after clear', async ($, on) => {
  const clock = mock.clock(on)
  mock.env(on, {})
  fixtureUi(on)
  on('mcp.connect', () => ({ value: { isConnected: true, server: serverName } }))
  on('session.usage', () => ({ value: usage(true) }))
  on('config.list', () => ({ value: [] }))
  on('session.messages', () => ({ value: [] }))
  const files = new Map<string, string>()
  let sessionId = 'first'
  let blockWrites = false
  let releaseWrite!: () => void
  let writing!: () => void
  const blocked = new Promise<void>((resolve) => { releaseWrite = resolve })
  const entered = new Promise<void>((resolve) => { writing = resolve })
  const pieces = [
    { kind: 'thinking', index: 0, text: '  思考\r\n' },
    { kind: 'text', index: 1, text: 'same' },
    { kind: 'text', index: 1, text: 'same' },
    { kind: 'text', index: 1, text: '\u0000🦊\n' },
  ] as const
  on('session.id', () => ({ value: sessionId }))
  on('session.cwd', () => ({ value: '/fixture' }))
  on('settings.read', heldByWorld)
  on('session.model', () => ({ value: 'fixture' }))
  on('session.surfaces', () => ({ value: ['terminal', 'vscode'] }))
  on('session.version', () => ({ value: { version: '2.1.289' } }))
  on('process.run', (_, e) => ({ value: { exitCode: 0, stdout: JSON.stringify(e.argv[1] === 'claude-session-directory' ? e.argv[2] + '/claude/' + e.argv[3] : { pid: 123, token: 'fixture-lifetime' }), stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }))
  on('process.spawn', guard)
  on('fs.write', async (_, e) => {
    if (blockWrites && e.path.replaceAll('\\', '/').endsWith('.json') && JSON.parse(e.text).kind === 'batch') {
      writing()
      await blocked
    }
    files.set(e.path.replaceAll('\\', '/'), e.text)
    return { value: undefined }
  })
  on('fs.read', (_, e) => {
    if (!files.has(e.path.replaceAll('\\', '/'))) return { deny: 'fixture: file unavailable' }
    return { value: files.get(e.path.replaceAll('\\', '/'))! }
  })
  on('fs.list', (_, e) => ({ value: [...files.keys()]
    .filter((path) => path.startsWith(e.path.replaceAll('\\', '/') + '/') && !path.slice(e.path.replaceAll('\\', '/').length + 1).includes('/'))
    .map((path) => ({ name: path.slice(e.path.replaceAll('\\', '/').length + 1), kind: 'file', size: files.get(path)!.length, mtimeMs: 0, isLink: false })) }))
  on('session.start', (_, e) => ({ cwd: e.cwd }))
  on('session.end', (_, e) => ({ sessionId: e.sessionId }))
  on('classic.SessionStart', () => ({}))
  on('turn.start', (_, e) => ({ turnId: e.turnId }))
  on('turn.complete', (_, e) => ({ text: e.answer }))
  on('turn.step', async function* (_, e) {
    for (const piece of pieces) yield piece
    return { turnId: e.turnId, index: e.index, answer: 'engine result', toolUses: [], stopReason: 'end_turn', usage: null }
  })

  await $.session.start({ cwd: '/fixture', surface: 'terminal', isInteractive: true })
  expect([...files.keys()].some((path) => path.endsWith('/first/session.json'))).toBe(true)
  await $.turn.start({ turnId: 'old', text: 'work' })
  blockWrites = true
  const stream = $.turn.step({ turnId: 'old', index: 0, model: 'fixture', messageCount: 1 })
  const iterator = stream[Symbol.asyncIterator]()
  expect((await iterator.next()).value).toEqual(pieces[0])
  const settling = clock.settle()
  await entered
  await $.session.end({ sessionId: 'first', reason: 'clear', resume: { id: 'first' } })
  sessionId = 'second'
  await $.classic.SessionStart({ source: 'clear' })
  await $.turn.start({ turnId: 'new', text: 'new work' })
  for (const piece of pieces.slice(1)) expect((await iterator.next()).value).toEqual(piece)
  const completed = await iterator.next()
  expect(completed.done).toBe(true)
  expect(completed.value).toEqual(expect.objectContaining({ answer: 'engine result' }))
  // Only now let the World's first stream write finish. The host already got
  // every original piece and the result while that write was blocked.
  releaseWrite()
  await settling
  await clock.settle()
  await $.turn.complete({ turnId: 'old', answer: 'engine result', reason: 'answer', isAborted: false, durationMs: 0 })
  const batches = [...files.entries()].filter(([path, body]) => path.includes('/events/') && path.endsWith('.json') && JSON.parse(body).kind === 'batch')
  expect(batches.length > 0).toBe(true)
  expect(batches.every(([path]) => path.includes('/first/'))).toBe(true)
  const observations = batches.flatMap(([, body]) => JSON.parse(body).events)
  const deltas = observations.filter((entry) => entry.happened.event === 'grew')
  expect(deltas.map((entry) => entry.happened.text)).toEqual(pieces.map((piece) => piece.text))
  expect(new Set(deltas.map((entry) => entry.at)).size).toBe(pieces.length)
  expect(observations.filter((entry) => entry.happened.event === 'finished').map((entry) => entry.happened.item.text))
    .toEqual(['  思考\r\n', 'samesame\u0000🦊\n'])
  expect(batches.length < observations.length).toBe(true)
  expect(JSON.parse(files.get([...files.keys()].find((path) => path.endsWith('/second/activity.json'))!)!).turn).toBe('new')
})

// The official engine checks the ownership mapping, that foreign calls are
// left alone, and that a World call goes through only once its proof is written.
test('proves only World calls, before they run, and distinguishes clear from resume', async ($, on) => {
  mock.clock(on)
  mock.env(on, {})
  fixtureUi(on)
  const files = new Map<string, string>()
  let sessionId = 'one'
  let failProof = false
  let failReports = false
  let executed = 0
  const proofs = () => [...files.keys()].filter((path) => path.includes('/calls/'))
  const proof = (id: string) => {
    let name = ''
    for (let i = 0; i < id.length; i++) name += id.charCodeAt(i).toString(16).padStart(4, '0')
    const path = proofs().find((path) => path.endsWith('/calls/' + name + '.json'))
    return path === undefined ? undefined : JSON.parse(files.get(path)!)
  }
  on('mcp.connect', () => ({ value: { isConnected: true, server: serverName } }))
  on('session.usage', () => ({ value: usage() }))
  on('session.id', () => ({ value: sessionId }))
  on('session.cwd', () => ({ value: '/fixture' }))
  on('settings.read', heldByWorld)
  on('session.model', () => ({ value: 'fixture' }))
  on('session.surfaces', () => ({ value: ['terminal'] }))
  on('session.version', () => ({ value: { version: '2.1.289' } }))
  on('process.run', (_, e) => ({ value: { exitCode: 0, stdout: JSON.stringify(e.argv[1] === 'claude-session-directory' ? e.argv[2] + '/claude/' + e.argv[3] : { pid: 123, token: 'fixture-lifetime' }), stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }))
  on('process.spawn', guard)
  on('config.list', () => ({ value: [] }))
  on('session.messages', () => ({ value: [] }))
  on('fs.write', (_, e) => {
    const path = e.path.replaceAll('\\', '/')
    if (failProof && path.includes('/calls/')) return { deny: 'fixture: disk refused' }
    if (failReports && (path.includes('/events/') || path.endsWith('/presentation.json'))) return { deny: 'fixture: reports unavailable' }
    files.set(path, e.text)
    return { value: undefined }
  })
  on('fs.read', (_, e) => {
    const path = e.path.replaceAll('\\', '/')
    if (!files.has(path)) return { deny: 'fixture: file unavailable' }
    return { value: files.get(path)! }
  })
  on('fs.list', (_, e) => {
    const directory = e.path.replaceAll('\\', '/')
    return { value: [...files.keys()]
      .filter((path) => path.startsWith(directory + '/') && !path.slice(directory.length + 1).includes('/'))
      .map((path) => ({ name: path.slice(directory.length + 1), kind: 'file', size: files.get(path)!.length, mtimeMs: 0, isLink: false })) }
  })
  on('session.start', (_, e) => ({ cwd: e.cwd }))
  on('session.end', (_, e) => ({ sessionId: e.sessionId }))
  on('classic.SessionStart', () => ({}))
  on('classic.PostToolUse', () => ({}))
  on('classic.PostToolUseFailure', () => ({}))
  on('tool.call', (_, e) => {
    if (e.tool === worldTool) {
      const written = proof(e.tool_use_id!)
      expect(written.tool_use_id).toBe(e.tool_use_id)
      expect(written.registration.session).toBe(sessionId)
      executed += 1
    } else {
      expect(e.command).toBe('same bytes')
    }
    return { result: { raw: '  exact \r\n🦊' } }
  })

  await $.session.start({ cwd: '/fixture', surface: 'terminal', isInteractive: true })
  failProof = true
  for (const tool of ['mcp__github__run', 'mcp__computer_use__js', 'mcp__plugin_fastctx-world_fastctx__lookalike']) {
    expect((await $.tool.call({ tool, command: 'same bytes' })).result).toEqual({ raw: '  exact \r\n🦊' })
    await $.classic.PostToolUse({ tool_name: tool, tool_use_id: 'foreign', tool_input: {}, tool_response: {} })
    await $.classic.PostToolUseFailure({ tool_name: tool, tool_use_id: 'foreign', tool_input: {}, error: 'fixture' })
  }
  expect(proofs().length).toBe(0)
  failProof = false
  await $.tool.call({ tool: worldTool, command: 'first' })
  await $.tool.call({ tool: worldTool, command: 'child', agentId: 'child' })
  await $.session.end({ sessionId: 'one', reason: 'clear', resume: { id: 'one' } })
  sessionId = 'two'
  await $.classic.SessionStart({ source: 'clear' })
  await $.tool.call({ tool: worldTool, command: 'cleared' })
  const registration = (id: string) => JSON.parse(files.get([...files.keys()].find((path) => path.endsWith('/' + id + '/session.json'))!)!)
  expect(registration('two').previous).toBe('one')
  expect(registration('two').at > registration('one').at).toBe(true)
  await $.session.end({ sessionId: 'two', reason: 'resume', resume: { id: 'two' } })
  sessionId = 'unrelated'
  await $.classic.SessionStart({ source: 'resume' })
  await $.tool.call({ tool: worldTool, command: 'resumed' })
  expect(registration('unrelated').previous).toBe(null)
  expect(registration('unrelated').at > registration('two').at).toBe(true)
  expect(executed).toBe(4)
  expect(proofs().length).toBe(4)
  failProof = true
  const refused = await $.tool.call({ tool: worldTool, command: 'must not run' })
  expect(refused).toEqual(expect.objectContaining({ deny: expect.any(String) }))
  expect(refused.deny).toContain('fixture: disk refused')
  expect(executed).toBe(4)
  failProof = false
  failReports = true
  expect((await $.tool.call({ tool: worldTool, command: 'proved despite reporting failure' })).result).toEqual({ raw: '  exact \r\n🦊' })
  expect(executed).toBe(5)
  expect(proofs().length).toBe(5)
})

// The native chain must still run once when the World cannot initialize or
// cannot persist observations. No fixture refers to the plugin's location.
test('World failures preserve native operations before and after initialization', async ($, on) => {
  mock.clock(on)
  mock.env(on, {})
  const lines = fixtureUi(on)
  const stored = mock.session(on)
  const files = new Map<string, string>()
  let sessionId = 'unavailable-at-start'
  let unavailable = true
  let tools = 0, compactions = 0, modelRequests = 0, spawns = 0
  const classics: string[] = []
  on('mcp.connect', () => ({ value: { isConnected: false, reason: 'disabled', message: 'fixture' } }))
  on('session.id', () => ({ value: sessionId }))
  on('session.cwd', () => ({ value: '/fixture' }))
  on('settings.read', heldByWorld)
  on('session.model', () => ({ value: 'fixture' }))
  on('session.surfaces', () => ({ value: ['terminal'] }))
  on('session.version', () => ({ value: { version: '2.1.294' } }))
  on('session.messages', () => ({ value: [] }))
  on('config.list', () => ({ value: [] }))
  on('agent.list', () => ({ value: [] }))
  on('session.usage', () => ({ value: usage() }))
  on('process.run', (_, e) => ({ value: { exitCode: 0, stdout: JSON.stringify(e.argv[1] === 'claude-session-directory' ? e.argv[2] + '/claude/' + e.argv[3] : { pid: 123, token: 'fixture-lifetime' }), stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }))
  on('process.spawn', async function* (_, e, next) {
    if (e.argv[1] === 'claude-control-guard') {
      yield { stream: 'stdout', text: JSON.stringify({ process: { pid: 124, token: 'activation' }, parent: { pid: 123, token: 'fixture-lifetime' } }) + '\n' }
      await new Promise<void>((resolve) => next.signal.addEventListener('abort', () => resolve(), { once: true }))
      return { value: { code: 0, signal: null } }
    }
    yield { stream: 'stderr', text: 'fixture: node unavailable' }
    return { value: { code: 1, signal: null } }
  })
  on('fs.write', (_, e) => {
    if (unavailable) return { deny: 'fixture: disk unavailable' }
    files.set(e.path.replaceAll('\\', '/'), e.text)
    return { value: undefined }
  })
  on('fs.read', (_, e) => {
    const path = e.path.replaceAll('\\', '/')
    return unavailable || !files.has(path) ? { deny: 'fixture: file unavailable' } : { value: files.get(path)! }
  })
  on('fs.list', (_, e) => {
    if (unavailable) return { deny: 'fixture: directory unavailable' }
    const prefix = e.path.replaceAll('\\', '/') + '/'
    return { value: [...files.keys()].filter((path) => path.startsWith(prefix) && !path.slice(prefix.length).includes('/'))
      .map((path) => ({ name: path.slice(prefix.length), kind: 'file', size: files.get(path)!.length, mtimeMs: 0, isLink: false })) }
  })
  on('session.start', (_, e) => ({ cwd: e.cwd }))
  on('session.end', (_, e) => ({ sessionId: e.sessionId }))
  on('turn.start', (_, e) => ({ turnId: e.turnId }))
  on('turn.complete', (_, e) => ({ text: e.answer }))
  on('tool.call', () => { tools++; return { result: { native: true } } })
  on('agent.spawn', () => { spawns++; return { model: 'native-model', agentId: 'native-agent' } })
  on('turn.step', async function* (_, e) {
    modelRequests++
    yield { kind: 'text', index: 0, text: 'native response' }
    return { turnId: e.turnId, index: e.index, answer: 'native response', toolUses: [], stopReason: 'end_turn', usage: null }
  })
  let invokingNativeTool = false
  on('tool.check', () => ({ decision: invokingNativeTool ? 'allow' : 'ask', reason: 'native permission' }))
  on('session.compact', () => { compactions++; return { messages: [{ role: 'user', text: 'native summary', toolUses: [] }] } })
  on('prompt.section', () => ({ text: 'native environment' }))
  on('classic.Elicitation', () => { classics.push('Elicitation'); return {} })
  on('classic.SubagentStart', () => { classics.push('SubagentStart'); return {} })
  on('classic.SubagentStop', () => { classics.push('SubagentStop'); return {} })
  on('classic.UserPromptSubmit', () => { classics.push('UserPromptSubmit'); return {} })
  on('classic.PostToolUse', () => { classics.push('PostToolUse'); return {} })
  on('classic.PostToolUseFailure', () => { classics.push('PostToolUseFailure'); return {} })
  on('classic.SessionStart', () => { classics.push('SessionStart'); return {} })
  on('classic.PostModelSwitch', () => { classics.push('PostModelSwitch'); return {} })

  for (const initialized of [false, true]) {
    sessionId = initialized ? 'unavailable-after-start' : 'unavailable-at-start'
    unavailable = !initialized
    await $.session.start({ cwd: '/fixture', surface: 'terminal', isInteractive: true })
    unavailable = true
    await $.turn.start({ turnId: sessionId, text: 'native prompt' })
    invokingNativeTool = true
    expect((await $.tool.call({ tool: 'Read', path: '/fixture/file' })).result).toEqual({ native: true })
    invokingNativeTool = false
    expect((await $.tool.check({ tool: 'Read', tool_use_id: sessionId, input: { path: '/fixture/file' } })).decision).toBe('ask')
    expect(await $.agent.spawn({ prompt: 'native task', description: 'native', subagentType: 'Explore' }))
      .toEqual({ model: 'native-model', agentId: 'native-agent' })
    const appended = await $.session.append({ door: 'prompt', origin: { kind: 'composer' }, uuid: 'native-' + sessionId,
      message: { type: 'user', role: 'user', content: [{ type: 'text', text: sessionId }] } })
    expect(appended.deny).toBe(undefined)
    expect((await $.session.compact({ trigger: 'auto', messages: [] })).messages![0].text).toBe('native summary')
    expect((await $.prompt.section({ name: 'env_info_simple', text: 'native environment' })).text).toBe('native environment')
    const stream = $.turn.step({ turnId: sessionId, index: 0, model: 'native-fixture', messageCount: 1 })
    const pieces = []
    // The testing kit returns a plain generator, unlike a hook's next stream.
    // Reading its terminal value also verifies the native request's result.
    let terminal = await stream.next()
    while (!terminal.done) {
      pieces.push(terminal.value)
      terminal = await stream.next()
    }
    expect(pieces).toEqual([{ kind: 'text', index: 0, text: 'native response' }])
    expect(terminal.value).toEqual(expect.objectContaining({ turnId: sessionId, index: 0, answer: 'native response',
      toolUses: [], stopReason: 'end_turn', usage: null }))
    await $.classic.Elicitation({ mcp_server_name: 'fixture', message: 'native form', mode: 'form', requested_schema: { type: 'object' } })
    await $.classic.SubagentStart({ agent_id: 'child', agent_type: 'Explore' })
    await $.classic.SubagentStop({ agent_id: 'child', agent_type: 'Explore', agent_transcript_path: '', stop_hook_active: false })
    await $.classic.UserPromptSubmit({ prompt: 'native prompt', session_title: 'fixture title' })
    await $.classic.PostToolUse({ tool_name: 'Read', tool_use_id: sessionId, tool_input: {}, tool_response: {} })
    await $.classic.PostToolUseFailure({ tool_name: 'Read', tool_use_id: sessionId, tool_input: {}, error: 'native failure' })
    await $.classic.SessionStart({ source: 'resume' })
    await $.classic.PostModelSwitch({ from_model: 'fixture-old', to_model: 'fixture-new', requested_model: null, source: 'auto',
      context_tokens: 0, prompt_cache_warm: false, cache_ttl: '5m', estimated_cache_write_usd: 0, pricing: 'default' })
    await $.turn.complete({ turnId: sessionId, answer: 'native answer', reason: 'answer', isAborted: false, durationMs: 0 })
    await $.session.end({ sessionId, reason: 'prompt_input_exit', resume: { id: sessionId } })
  }
  expect(tools).toBe(2)
  expect(spawns).toBe(2)
  expect(compactions).toBe(2)
  expect(modelRequests).toBe(2)
  expect(stored.appended().length).toBe(2)
  expect(classics).toEqual(Array.from({ length: 2 }).flatMap(() => ['Elicitation', 'SubagentStart', 'SubagentStop', 'UserPromptSubmit', 'PostToolUse', 'PostToolUseFailure', 'SessionStart', 'PostModelSwitch']))
  expect(lines.some((line) => line.includes('disk unavailable'))).toBe(true)
})

// A session the user opened outside Fastctx is given the World's run and
// nothing more (`world-layers.md`): the World's hooks step aside in it,
// reserving no directory and writing nothing of the session.
test('steps aside in a session the World does not hold', async ($, on) => {
  mock.clock(on)
  mock.env(on, {})
  fixtureUi(on)
  on('mcp.connect', () => ({ value: { isConnected: true, server: serverName } }))
  on('session.usage', () => ({ value: usage(true) }))
  on('config.list', () => ({ value: [] }))
  on('session.messages', () => ({ value: [] }))
  on('session.id', () => ({ value: 'own' }))
  on('session.cwd', () => ({ value: '/fixture' }))
  on('settings.read', () => ({ value: {} }))
  on('session.model', () => ({ value: 'fixture' }))
  on('session.surfaces', () => ({ value: ['terminal'] }))
  on('session.version', () => ({ value: { version: '2.1.289' } }))
  const ran: string[][] = []
  on('process.run', (_, e) => {
    ran.push(e.argv)
    return { value: { exitCode: 0, stdout: '{}', stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }
  })
  on('process.spawn', guard)
  const written: string[] = []
  on('fs.write', (_, e) => {
    written.push(e.path)
    return { value: undefined }
  })
  on('fs.read', () => ({ deny: 'fixture: file unavailable' }))
  on('fs.list', () => ({ value: [] }))
  on('session.start', (_, e) => ({ cwd: e.cwd }))
  on('turn.start', (_, e) => ({ turnId: e.turnId }))
  on('turn.complete', (_, e) => ({ text: e.answer }))
  await $.session.start({ cwd: '/fixture', surface: 'terminal', isInteractive: true })
  await $.turn.start({ turnId: 'own-turn', text: 'work' })
  await $.turn.complete({ turnId: 'own-turn', answer: 'done', reason: 'answer', isAborted: false, durationMs: 0 })
  expect(ran.filter((argv) => argv.includes('claude-session-directory'))).toEqual([])
  expect(written).toEqual([])
})
