import { test, expect, mock } from 'claude-code/testing'
import type { SessionUsage } from 'claude-code'

const worldTool = 'mcp__deduplicated_alias__run'
const serverName = 'configured World alias'
function usage(): SessionUsage {
  return { startedAt: 0, rateLimits: [], context: { window: 200000, breakdown: {
    categories: [], totalTokens: 0, maxTokens: 200000, rawMaxTokens: 200000,
    autocompactSource: 'model-default', percentage: 0, gridRows: [], model: 'fixture',
    memoryFiles: [], agents: [], isAutoCompactEnabled: false, apiUsage: null,
    mcpTools: [
      { name: worldTool, serverName, tokens: 1, isLoaded: false },
      { name: 'mcp__github__run', serverName: 'GitHub', tokens: 1, isLoaded: true },
      { name: 'mcp__plugin_fastctx-world_fastctx__lookalike', serverName: 'Other', tokens: 1, isLoaded: true },
    ],
  } } }
}

// The official loader and engine must admit the published module. A VM-only
// test cannot enforce the scanner's rules or the engine's stream lifetime.
test('loads, streams without waiting for disk, and retains the old session after clear', async ($, on) => {
  const clock = mock.clock(on)
  mock.env(on, {})
  on('mcp.connect', () => ({ value: { isConnected: true, server: serverName } }))
  on('session.usage', () => ({ value: usage() }))
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
  on('session.model', () => ({ value: 'fixture' }))
  on('session.surfaces', () => ({ value: ['terminal', 'vscode'] }))
  on('session.version', () => ({ value: { version: '2.1.289' } }))
  on('process.run', () => ({ value: { exitCode: 0, stdout: JSON.stringify({ pid: 123, token: 'fixture-lifetime' }), stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }))
  on('fs.write', async (_, e) => {
    if (blockWrites && e.path.replaceAll('\\', '/').endsWith('.json') && JSON.parse(e.text).kind === 'batch') {
      writing()
      await blocked
    }
    files.set(e.path.replaceAll('\\', '/'), e.text)
    return { value: undefined }
  })
  on('fs.read', (_, e) => {
    if (!files.has(e.path.replaceAll('\\', '/'))) throw new Error('missing fixture file: ' + e.path.replaceAll('\\', '/'))
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

// The official engine checks the ownership mapping, untouched foreign calls,
// and the lease's lifetime around both successful and refused World calls.
test('leases only World calls, releases them, and distinguishes clear from resume', async ($, on) => {
  mock.clock(on)
  mock.env(on, {})
  const files = new Map<string, string>()
  const active = new Map<string, { registration: { session: string }, owner: { pid: number, token: string } }>()
  let sessionId = 'one'
  let failProof = false
  let executed = 0
  let helper = 123
  let pairingOperations = 0
  on('mcp.connect', () => ({ value: { isConnected: true, server: serverName } }))
  on('session.usage', () => ({ value: usage() }))
  on('session.id', () => ({ value: sessionId }))
  on('session.cwd', () => ({ value: '/fixture' }))
  on('session.model', () => ({ value: 'fixture' }))
  on('session.surfaces', () => ({ value: ['terminal'] }))
  on('session.version', () => ({ value: { version: '2.1.289' } }))
  on('process.run', (_, e) => {
    if (e.argv[1] === 'claude-call') {
      pairingOperations += 1
      const id = e.argv[4], owner = JSON.parse(e.init.stdin!)
      expect(active.get(id)?.owner).toEqual(owner)
      active.delete(id)
      return { value: { exitCode: 0, stdout: '', stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }
    }
    return { value: { exitCode: 0, stdout: JSON.stringify({ pid: 123, token: 'fixture-lifetime' }), stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }
  })
  on('process.spawn', async function* (_, e) {
    pairingOperations += 1
    const id = e.argv[4]
    if (failProof || active.has(id)) {
      yield { stream: 'stderr', text: 'claim refused' }
      return { value: { code: 1, signal: null } }
    }
    const entry = { registration: JSON.parse(e.input!), owner: { pid: ++helper, token: 'fixture-helper-' + helper } }
    active.set(id, entry)
    try {
      yield { stream: 'stdout', text: JSON.stringify(entry.owner) + '\n' }
    } finally {
      if (active.get(id) === entry) active.delete(id)
    }
    return { value: { code: 0, signal: null } }
  })
  on('fs.write', (_, e) => {
    const path = e.path.replaceAll('\\', '/')
    files.set(path, e.text)
    return { value: undefined }
  })
  on('fs.read', (_, e) => {
    const path = e.path.replaceAll('\\', '/')
    if (!files.has(path)) throw new Error('missing fixture file: ' + path)
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
      expect(active.get(e.tool_use_id!)?.registration.session).toBe(sessionId)
      executed += 1
    } else {
      expect(active.size).toBe(0)
      expect(e.command).toBe('same bytes')
    }
    return { result: { raw: '  exact \r\n🦊' } }
  })

  await $.session.start({ cwd: '/fixture', surface: 'terminal', isInteractive: true })
  const before = pairingOperations
  failProof = true
  for (const tool of ['mcp__github__run', 'mcp__computer_use__js', 'mcp__plugin_fastctx-world_fastctx__lookalike']) {
    expect((await $.tool.call({ tool, command: 'same bytes' })).result).toEqual({ raw: '  exact \r\n🦊' })
    await $.classic.PostToolUse({ tool_name: tool, tool_use_id: 'foreign', tool_input: {}, tool_response: {} })
    await $.classic.PostToolUseFailure({ tool_name: tool, tool_use_id: 'foreign', tool_input: {}, error: 'fixture' })
  }
  expect(pairingOperations).toBe(before)
  failProof = false
  await $.tool.call({ tool: worldTool, command: 'first' })
  expect(active.size).toBe(0)
  await $.tool.call({ tool: worldTool, command: 'child', agentId: 'child' })
  expect(active.size).toBe(0)
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
  expect(active.size).toBe(0)
  failProof = true
  const refused = await $.tool.call({ tool: worldTool, command: 'must not run' })
  expect(refused.deny).toContain('could not identify this MCP call')
  expect(executed).toBe(4)
  expect(active.size).toBe(0)
})
