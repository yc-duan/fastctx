import { test, expect, mock } from 'claude-code/testing'

// The official loader and engine must admit the published module. A VM-only
// test cannot enforce the scanner's rules or the engine's stream lifetime.
test('loads, streams without waiting for disk, and retains the old session after clear', async ($, on) => {
  const clock = mock.clock(on)
  mock.env(on, {})
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

// This freezes the host boundary: a tool reaches core only after its own
// proof is complete, including tools executed by an Agent loop.
test('pairs each MCP call before execution and distinguishes clear from resume', async ($, on) => {
  mock.clock(on)
  mock.env(on, {})
  const files = new Map<string, string>()
  let sessionId = 'one'
  let failProof = false
  let executed = 0
  on('session.id', () => ({ value: sessionId }))
  on('session.cwd', () => ({ value: '/fixture' }))
  on('session.model', () => ({ value: 'fixture' }))
  on('session.surfaces', () => ({ value: ['terminal'] }))
  on('session.version', () => ({ value: { version: '2.1.289' } }))
  on('process.run', () => ({ value: { exitCode: 0, stdout: JSON.stringify({ pid: 123, token: 'fixture-lifetime' }), stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }))
  on('fs.write', (_, e) => {
    if (failProof && e.path.replaceAll('\\', '/').includes('/calls/')) throw new Error('fixture disk failure')
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
  on('tool.call', (_, e) => {
    const entries = [...files.entries()].filter(([path, body]) => path.includes('/calls/') && path.endsWith('.json') && JSON.parse(body).tool_use_id === e.tool_use_id)
    expect(entries.length).toBe(1)
    const [path, body] = entries[0]
    expect(files.has(path.slice(0, -5) + '.done')).toBe(true)
    expect(JSON.parse(body).registration.session).toBe(sessionId)
    executed += 1
    return { result: 'ran' }
  })

  await $.session.start({ cwd: '/fixture', surface: 'terminal', isInteractive: true })
  await $.tool.call({ tool: 'mcp__fixture__run', command: 'first' })
  await $.tool.call({ tool: 'mcp__fixture__run', command: 'child', agentId: 'child' })
  await $.session.end({ sessionId: 'one', reason: 'clear', resume: { id: 'one' } })
  sessionId = 'two'
  await $.classic.SessionStart({ source: 'clear' })
  await $.tool.call({ tool: 'mcp__fixture__run', command: 'cleared' })
  const registration = (id: string) => JSON.parse(files.get([...files.keys()].find((path) => path.endsWith('/' + id + '/session.json'))!)!)
  expect(registration('two').previous).toBe('one')
  expect(registration('two').at > registration('one').at).toBe(true)
  await $.session.end({ sessionId: 'two', reason: 'resume', resume: { id: 'two' } })
  sessionId = 'unrelated'
  await $.classic.SessionStart({ source: 'resume' })
  await $.tool.call({ tool: 'mcp__fixture__run', command: 'resumed' })
  expect(registration('unrelated').previous).toBe(null)
  expect(registration('unrelated').at > registration('two').at).toBe(true)
  expect(executed).toBe(4)
  failProof = true
  const refused = await $.tool.call({ tool: 'mcp__fixture__run', command: 'must not run' })
  expect(refused.deny).toContain('could not identify this MCP call')
  expect(executed).toBe(4)
})
