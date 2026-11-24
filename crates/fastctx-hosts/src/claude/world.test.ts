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

// A production session's start hook waited for whole turns, and every block
// it streamed was shown twice: as streamed, and as the row Claude Code stored
// (2026-11-11). A row takes the block its step streamed by the step itself,
// whatever the plugin knows of the turn.
test('a stored row takes the block its step streamed while the turn is not known yet', async ($, on) => {
  const clock = mock.clock(on)
  mock.env(on, {})
  fixtureUi(on)
  on('mcp.connect', () => ({ value: { isConnected: true, server: serverName } }))
  on('session.usage', () => ({ value: usage(true) }))
  on('config.list', () => ({ value: [] }))
  on('session.messages', () => ({ value: [] }))
  const files = new Map<string, string>()
  on('session.id', () => ({ value: 'lagging' }))
  on('session.cwd', () => ({ value: '/fixture' }))
  on('settings.read', heldByWorld)
  on('session.model', () => ({ value: 'fixture' }))
  on('session.surfaces', () => ({ value: ['terminal'] }))
  on('session.version', () => ({ value: { version: '2.1.322' } }))
  on('process.run', (_, e) => ({ value: { exitCode: 0, stdout: JSON.stringify(e.argv[1] === 'claude-session-directory' ? e.argv[2] + '/claude/' + e.argv[3] : { pid: 123, token: 'fixture-lifetime' }), stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }))
  on('process.spawn', guard)
  on('fs.write', (_, e) => {
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
  on('turn.step', async function* (_, e) {
    yield { kind: 'thinking', index: 0, text: 'weighing' }
    yield { kind: 'text', index: 1, text: 'once' }
    return { turnId: e.turnId, index: e.index, answer: 'once', toolUses: [], stopReason: 'end_turn', usage: null }
  })

  await $.session.start({ cwd: '/fixture', surface: 'terminal', isInteractive: true })
  // The host runs a turn whose start the plugin has not taken in: no
  // `turn.start` reaches it. Each block's row is stored as the block closes,
  // before the step's end.
  const stream = $.turn.step({ turnId: 'host-turn', index: 0, model: 'fixture', messageCount: 1 })
  expect((await stream.next()).value).toEqual({ kind: 'thinking', index: 0, text: 'weighing' })
  expect((await stream.next()).value).toEqual({ kind: 'text', index: 1, text: 'once' })
  const row = (uuid: string, block: { type: 'thinking'; thinking: string; signature: string } | { type: 'text'; text: string }) =>
    $.session.append({ door: 'response', origin: { kind: 'model', model: 'fixture' }, uuid,
      message: { type: 'assistant', role: 'assistant', content: [block] } })
  await row('row-thinking', { type: 'thinking', thinking: 'weighing', signature: 'fixture' })
  await row('row-text', { type: 'text', text: 'once' })
  expect((await stream.next()).done).toBe(true)
  await clock.settle()

  const told = [...files.entries()]
    .filter(([path]) => path.includes('/events/') && path.endsWith('.json'))
    .flatMap(([, body]) => {
      const report = JSON.parse(body)
      return report.kind === 'batch'
        ? report.events.filter((entry: { kind: string }) => entry.kind === 'session').map((entry: { happened: unknown }) => entry.happened)
        : report.kind === 'session' ? [report.happened] : []
    }) as { event: string; turn: string; item?: { id: string; draft?: string; text?: string } }[]
  const renamed = told.filter((happened) => happened.event === 'finished' && happened.item?.draft !== undefined)
  expect(renamed.map((happened) => [happened.item!.id, happened.item!.draft, happened.turn])).toEqual([
    [JSON.stringify(['row-thinking', 0]), JSON.stringify(['host-turn', 0, 0, 'thinking']), 'host-turn'],
    [JSON.stringify(['row-text', 0]), JSON.stringify(['host-turn', 0, 1, 'text']), 'host-turn'],
  ])
  // Neither row is told again as a block of its own.
  expect(told.filter((happened) => happened.event === 'began' && happened.item?.id.startsWith('["row-'))).toEqual([])
})

// Reports go to the node on its loopback channel where it says it listens
// (`channel.js`), none of them to a file; once the channel is not reached,
// this session's reports go to files, and stay there so none overtakes one
// still in a file.
test('hands reports to the node on its channel, and to files once it is not reached', async ($, on) => {
  const clock = mock.clock(on)
  mock.env(on, {})
  fixtureUi(on)
  on('mcp.connect', () => ({ value: { isConnected: true, server: serverName } }))
  on('session.usage', () => ({ value: usage(true) }))
  on('config.list', () => ({ value: [] }))
  on('session.messages', () => ({ value: [] }))
  const files = new Map<string, string>()
  on('session.id', () => ({ value: 'channelled' }))
  on('session.cwd', () => ({ value: '/fixture' }))
  on('settings.read', heldByWorld)
  on('session.model', () => ({ value: 'fixture' }))
  on('session.surfaces', () => ({ value: ['terminal'] }))
  on('session.version', () => ({ value: { version: '2.1.322' } }))
  on('process.run', (_, e) => {
    if (e.argv[1] === 'claude-session-directory') {
      const directory = e.argv[2] + '/claude/' + e.argv[3]
      files.set(directory + '/channel.json', JSON.stringify({ version: 2, port: 4321, token: 'fixture-token' }))
      return { value: { exitCode: 0, stdout: JSON.stringify(directory), stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }
    }
    return { value: { exitCode: 0, stdout: JSON.stringify({ pid: 123, token: 'fixture-lifetime' }), stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }
  })
  on('process.spawn', guard)
  on('fs.write', (_, e) => {
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
  on('turn.start', (_, e) => ({ turnId: e.turnId }))
  on('turn.step', async function* (_, e) {
    yield { kind: 'text', index: 0, text: 'said ' + e.index }
    return { turnId: e.turnId, index: e.index, answer: 'said', toolUses: [], stopReason: 'end_turn', usage: null }
  })
  const handed: { name: string; report: { kind: string } }[] = []
  const asked: string[] = []
  let reached = true
  let cut = false
  on('http.fetch', (_, e) => {
    asked.push(e.url)
    expect(e.url.startsWith('http://127.0.0.1:4321/v1/claude/channelled/')).toBe(true)
    expect(e.init?.headers?.authorization).toBe('Bearer fixture-token')
    if (cut && e.url.endsWith('/reports')) {
      cut = false
      return { deny: 'failed: turn-abort' }
    }
    if (!reached) return { deny: 'fixture: the node is not reached' }
    if (e.url.endsWith('/reports')) {
      expect(e.init?.method).toBe('POST')
      const { reports } = JSON.parse(e.init?.body as string) as { reports: { name: string; report: { kind: string } }[] }
      const first = handed.length + 1
      handed.push(...reports)
      // Taken at once and dealt with in turn: each report gets a receipt,
      // and each answer says how far the queue dealt, here up to the
      // report before.
      const receipts = reports.map((_, index) => first + index)
      const dealt = Math.max(0, handed.length - 1)
      return { value: { status: 200, ok: true, headers: {}, text: JSON.stringify({ received: reports.map((report) => report.name), queue: 1, receipts, dealt }) } }
    }
    if (e.url.endsWith('/subagents')) return { value: { status: 200, ok: true, headers: {}, text: JSON.stringify({ configuration: null }) } }
    // A wait the node holds: nothing new while the test runs.
    return new Promise(() => {})
  })
  const filed = () => [...files.keys()].filter((path) => path.includes('/events/') && path.endsWith('.json'))

  await $.session.start({ cwd: '/fixture', surface: 'terminal', isInteractive: true })
  await $.turn.start({ turnId: 'reached', text: 'work' })
  for await (const _ of $.turn.step({ turnId: 'reached', index: 0, model: 'fixture', messageCount: 1 })) { /* drained */ }
  await clock.settle()
  expect(handed.some((report) => report.report.kind === 'turn_start')).toBe(true)
  expect(handed.some((report) => report.report.kind === 'batch')).toBe(true)
  expect(filed()).toEqual([])
  expect(asked.some((url) => url.includes('/wake?seen='))).toBe(true)

  // A stop cuts short one hand-over: that one goes to a file, and the
  // channel carries the next as before.
  cut = true
  const before = handed.length
  for await (const _ of $.turn.step({ turnId: 'reached', index: 1, model: 'fixture', messageCount: 2 })) { /* drained */ }
  await clock.settle()
  expect(filed().length > 0).toBe(true)
  for await (const _ of $.turn.step({ turnId: 'reached', index: 2, model: 'fixture', messageCount: 3 })) { /* drained */ }
  await clock.settle()
  expect(handed.length > before).toBe(true)
  for (const path of filed()) files.delete(path)

  reached = false
  for await (const _ of $.turn.step({ turnId: 'reached', index: 3, model: 'fixture', messageCount: 4 })) { /* drained */ }
  await clock.settle()
  const after = filed().length
  expect(after > 0).toBe(true)
  // The last report the node took and never said it applied went to a file
  // too, before the first one written there.
  const last = handed[handed.length - 1].name
  expect(filed().some((path) => path.endsWith('/events/' + last))).toBe(true)
  reached = true
  for await (const _ of $.turn.step({ turnId: 'reached', index: 4, model: 'fixture', messageCount: 5 })) { /* drained */ }
  await clock.settle()
  expect(filed().length > after).toBe(true)
})

// The official engine checks the ownership mapping, that foreign calls are
// left alone, and that a World call goes through only once its proof is written.
test('proves only World calls, before they run, and distinguishes clear from resume', async ($, on) => {
  mock.clock(on)
  mock.env(on, {})
  fixtureUi(on)
  const files = new Map<string, string>()
  let sessionId = 'one'
  let listed = false
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
  on('session.usage', () => {
    const current = usage(listed)
    if (!listed) {
      const breakdown = current.context.breakdown!
      breakdown.mcpTools = breakdown.mcpTools.filter((tool) => tool.name !== worldTool)
    }
    return { value: current }
  })
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
  listed = true
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

// What the World leaves while a turn's tools run goes in at the opening
// after the step's last result, read or not by then: an opening a look
// missed, or a row the session did not keep, left words in the inbox until
// the turn ended (2026-11-15 review). What rides with the next turn and came
// in this one waits for the next.
// Each settling waits out the helpers' idle second: it takes its time.
test('what waits goes in at the opening of the step it waited through', { timeoutMs: 60000 }, async ($, on) => {
  const clock = mock.clock(on)
  mock.env(on, {})
  fixtureUi(on)
  on('mcp.connect', () => ({ value: { isConnected: true, server: serverName } }))
  on('session.usage', () => ({ value: usage(true) }))
  on('config.list', () => ({ value: [] }))
  on('session.messages', () => ({ value: [] }))
  const files = new Map<string, string>()
  on('session.id', () => ({ value: 'opening' }))
  on('session.cwd', () => ({ value: '/fixture' }))
  on('settings.read', heldByWorld)
  on('session.model', () => ({ value: 'fixture' }))
  on('session.surfaces', () => ({ value: ['terminal'] }))
  on('session.version', () => ({ value: { version: '2.1.322' } }))
  on('process.run', (_, e) => ({ value: { exitCode: 0, stdout: JSON.stringify(e.argv[1] === 'claude-session-directory' ? e.argv[2] + '/claude/' + e.argv[3] : { pid: 123, token: 'fixture-lifetime' }), stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }))
  // The session's helpers say `look` when the test says the node left
  // something.
  const lookers = new Set<() => void>()
  const sayLook = () => { for (const look of [...lookers]) look() }
  on('process.spawn', async function* (_, e, next) {
    if (e.argv[1] === 'claude-session-policy') {
      yield { stream: 'stdout' as const, text: JSON.stringify({ model: null, effort: null }) }
      return { value: { code: 0, signal: null } }
    }
    yield { stream: 'stdout' as const, text: JSON.stringify({ process: { pid: 124, token: 'activation' }, parent: { pid: 123, token: 'fixture-lifetime' } }) + '\n' }
    while (!next.signal.aborted) {
      await new Promise<void>((resolve) => {
        const look = () => { lookers.delete(look); resolve() }
        lookers.add(look)
        next.signal.addEventListener('abort', look, { once: true })
      })
      if (!next.signal.aborted) yield { stream: 'stdout' as const, text: 'look\n' }
    }
    return { value: { code: 0, signal: null } }
  })
  on('fs.write', (_, e) => {
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
  on('turn.start', (_, e) => ({ turnId: e.turnId }))
  on('turn.complete', (_, e) => ({ text: e.answer }))
  on('turn.step', async function* (_, e) {
    yield { kind: 'text' as const, index: 0, text: 'step ' + e.index }
    return { turnId: e.turnId, index: e.index, answer: '', toolUses: [{ name: 'Read', input: {} }, { name: 'Read', input: {} }], stopReason: 'tool_use', usage: null }
  })
  const stored = mock.session(on)
  let refuseOnce = false
  on('session.append', { door: 'note' }, (_, e, next) => {
    if (refuseOnce && JSON.stringify(e.message).includes('kept the second time')) {
      refuseOnce = false
      return { deny: 'fixture: the row was not kept' }
    }
    return next(e)
  })
  const has = (text: string) => stored.appended().filter((row) => JSON.stringify(row.message).includes(text)).length
  const step = async (turnId: string, index: number) => {
    for await (const _ of $.turn.step({ turnId, index, model: 'fixture', messageCount: 1 })) { /* streamed */ }
  }
  // A row's hooks wait on the batched report's timer: the clock moves while
  // it is kept.
  const settled = async <T>(doing: Promise<T>) => {
    await new Promise((resolve) => setTimeout(resolve, 30))
    await clock.settle()
    return doing
  }
  const result = (id: string) => settled($.session.append({ door: 'tool-result', origin: { kind: 'tool', tool: 'Read', tool_use_id: id }, uuid: 'result-' + id,
    message: { type: 'user', content: [{ type: 'tool_result', tool_use_id: id, content: id }] } }))

  await $.session.start({ cwd: '/fixture', surface: 'terminal', isInteractive: true })
  await clock.settle()
  const directory = [...files.keys()].find((path) => path.endsWith('/opening/session.json'))!.replace('/session.json', '')
  const leave = (id: string, delivery: object) => files.set(directory + '/inbox/' + id + '.json', JSON.stringify({ id, ...delivery }))
  await $.turn.start({ turnId: 'T1', text: 'work' })

  // Left before the results and not read yet: the opening reads it, and
  // nothing goes between the two results of one response.
  await step('T1', 0)
  leave('a', { text: 'left unread', as_user: true })
  await result('A')
  expect(has('left unread')).toBe(0)
  await result('B')
  expect(has('left unread')).toBe(1)
  expect(files.has(directory + '/receipts/a')).toBe(true)

  // Read just after an opening that had nothing: it still goes in there.
  await step('T1', 1)
  await result('C')
  await result('D')
  leave('b', { text: 'read after the opening', as_user: true })
  sayLook()
  await settled(Promise.resolve())
  await settled(Promise.resolve())
  expect(has('read after the opening')).toBe(1)

  // A row the session refused is held again, and goes in at the next
  // opening; the job the user stopped, which came in this turn, waits for
  // the next turn.
  await step('T1', 2)
  leave('c', { text: 'kept the second time', as_user: true })
  leave('d', { text: 'the stopped job ended', along: 'job-1', after_stop: true, at: new Date().toISOString() })
  leave('f', { text: 'the focus moved', along: 'focus' })
  sayLook()
  await settled(Promise.resolve())
  await settled(Promise.resolve())
  refuseOnce = true
  await result('E')
  await result('F')
  await settled(Promise.resolve())
  expect(has('kept the second time')).toBe(0)
  expect(files.has(directory + '/receipts/c')).toBe(false)
  await step('T1', 3)
  await result('G')
  await result('H')
  expect(has('kept the second time')).toBe(1)
  expect(files.has(directory + '/receipts/c')).toBe(true)
  // What rides with the next turn goes in a running one as before; only a
  // job stopped after the turn began waits.
  expect(has('the focus moved')).toBe(1)
  expect(has('the stopped job ended')).toBe(0)
  await settled($.turn.complete({ turnId: 'T1', answer: 'done', reason: 'answer', isAborted: false, durationMs: 0 }))
  await clock.settle()
  expect(has('the stopped job ended')).toBe(0)

  await $.turn.start({ turnId: 'T2', text: 'more work' })
  await step('T2', 0)
  await result('I')
  await result('J')
  expect(has('the stopped job ended')).toBe(1)
  expect(files.has(directory + '/receipts/d')).toBe(true)
})

// Words taken out of what is held to start a turn, whose submission then
// failed before the prompt went (its attempt not written), are held again
// and go at the next look: none is left outside both the session and what is
// held (2026-11-15 review).
test('words whose submission failed on the way are held again', { timeoutMs: 60000 }, async ($, on) => {
  const clock = mock.clock(on)
  mock.env(on, {})
  fixtureUi(on)
  on('mcp.connect', () => ({ value: { isConnected: true, server: serverName } }))
  on('session.usage', () => ({ value: usage(true) }))
  on('config.list', () => ({ value: [] }))
  on('session.messages', () => ({ value: [] }))
  const files = new Map<string, string>()
  on('session.id', () => ({ value: 'submitting' }))
  on('session.cwd', () => ({ value: '/fixture' }))
  on('settings.read', heldByWorld)
  on('session.model', () => ({ value: 'fixture' }))
  on('session.surfaces', () => ({ value: ['terminal'] }))
  on('session.version', () => ({ value: { version: '2.1.322' } }))
  // The words wait in the inbox when the session starts: its first look
  // takes them.
  let attemptsFail = true
  on('process.run', (_, e) => {
    if (e.argv[1] === 'claude-session-directory') {
      const directory = e.argv[2] + '/claude/' + e.argv[3]
      files.set(directory + '/inbox/w.json', JSON.stringify({ id: 'w', text: 'start a turn', as_user: true }))
      return { value: { exitCode: 0, stdout: JSON.stringify(directory), stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }
    }
    return { value: { exitCode: 0, stdout: JSON.stringify({ pid: 123, token: 'fixture-lifetime' }), stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }
  })
  on('process.spawn', guard)
  on('fs.write', (_, e) => {
    if (attemptsFail && e.path.replaceAll('\\', '/').includes('/attempts/')) return { deny: 'fixture: the disk refused it' }
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
  const submitted: string[] = []
  on('prompt.submit', (_, e) => {
    submitted.push(e.text)
    return { text: e.text }
  })

  await $.session.start({ cwd: '/fixture', surface: 'terminal', isInteractive: true })
  // The look the session's start left on the clock: its attempt is refused.
  await clock.advance(10)
  expect(submitted).toEqual([])
  expect(files.has([...files.keys()].find((path) => path.endsWith('/submitting/session.json'))!.replace('/session.json', '/receipts/w'))).toBe(false)
  // The look the failure asks for again later.
  attemptsFail = false
  await clock.advance(5000)
  expect(submitted).toEqual(['start a turn'])
})

// A late opening a look began, held up writing its attempt while the next
// step begins and its results all land, leaves that step's opening to it:
// the words go in there, not a step later (2026-11-15 second review).
test('a late opening held up does not take the next step its opening', { timeoutMs: 60000 }, async ($, on) => {
  const clock = mock.clock(on)
  mock.env(on, {})
  fixtureUi(on)
  on('mcp.connect', () => ({ value: { isConnected: true, server: serverName } }))
  on('session.usage', () => ({ value: usage(true) }))
  on('config.list', () => ({ value: [] }))
  on('session.messages', () => ({ value: [] }))
  const files = new Map<string, string>()
  on('session.id', () => ({ value: 'crossing' }))
  on('session.cwd', () => ({ value: '/fixture' }))
  on('settings.read', heldByWorld)
  on('session.model', () => ({ value: 'fixture' }))
  on('session.surfaces', () => ({ value: ['terminal'] }))
  on('session.version', () => ({ value: { version: '2.1.322' } }))
  on('process.run', (_, e) => ({ value: { exitCode: 0, stdout: JSON.stringify(e.argv[1] === 'claude-session-directory' ? e.argv[2] + '/claude/' + e.argv[3] : { pid: 123, token: 'fixture-lifetime' }), stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }))
  const lookers = new Set<() => void>()
  const sayLook = () => { for (const look of [...lookers]) look() }
  on('process.spawn', async function* (_, e, next) {
    if (e.argv[1] === 'claude-session-policy') {
      yield { stream: 'stdout' as const, text: JSON.stringify({ model: null, effort: null }) }
      return { value: { code: 0, signal: null } }
    }
    yield { stream: 'stdout' as const, text: JSON.stringify({ process: { pid: 124, token: 'activation' }, parent: { pid: 123, token: 'fixture-lifetime' } }) + '\n' }
    while (!next.signal.aborted) {
      await new Promise<void>((resolve) => {
        const look = () => { lookers.delete(look); resolve() }
        lookers.add(look)
        next.signal.addEventListener('abort', look, { once: true })
      })
      if (!next.signal.aborted) yield { stream: 'stdout' as const, text: 'look\n' }
    }
    return { value: { code: 0, signal: null } }
  })
  let holdAttempts = false
  let release!: () => void
  const released = new Promise<void>((resolve) => { release = resolve })
  on('fs.write', async (_, e) => {
    if (holdAttempts && e.path.replaceAll('\\', '/').includes('/attempts/')) await released
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
  on('turn.start', (_, e) => ({ turnId: e.turnId }))
  on('turn.step', async function* (_, e) {
    yield { kind: 'text' as const, index: 0, text: 'step ' + e.index }
    return { turnId: e.turnId, index: e.index, answer: '', toolUses: [{ name: 'Read', input: {} }, { name: 'Read', input: {} }], stopReason: 'tool_use', usage: null }
  })
  const stored = mock.session(on)
  const has = (text: string) => stored.appended().filter((row) => JSON.stringify(row.message).includes(text)).length
  const settled = async <T>(doing: Promise<T>) => {
    await new Promise((resolve) => setTimeout(resolve, 30))
    await clock.settle()
    return doing
  }
  const step = async (turnId: string, index: number) => {
    for await (const _ of $.turn.step({ turnId, index, model: 'fixture', messageCount: 1 })) { /* streamed */ }
  }
  const result = (id: string) => $.session.append({ door: 'tool-result', origin: { kind: 'tool', tool: 'Read', tool_use_id: id }, uuid: 'result-' + id,
    message: { type: 'user', content: [{ type: 'tool_result', tool_use_id: id, content: id }] } })

  await $.session.start({ cwd: '/fixture', surface: 'terminal', isInteractive: true })
  await clock.settle()
  const directory = [...files.keys()].find((path) => path.endsWith('/crossing/session.json'))!.replace('/session.json', '')
  await $.turn.start({ turnId: 'T1', text: 'work' })
  await step('T1', 0)
  await settled(result('A'))
  await settled(result('B'))
  // Read after that opening: the late opening it begins is held up.
  holdAttempts = true
  files.set(directory + '/inbox/w.json', JSON.stringify({ id: 'w', text: 'came between the steps', as_user: true }))
  sayLook()
  await settled(Promise.resolve())
  expect(has('came between the steps')).toBe(0)
  // The next step begins, and both its results land while it is held up.
  await step('T1', 1)
  await settled(result('C'))
  const last = result('D')
  await settled(Promise.resolve())
  holdAttempts = false
  release()
  await settled(last)
  await settled(Promise.resolve())
  expect(has('came between the steps')).toBe(1)
  expect(files.has(directory + '/receipts/w')).toBe(true)
})
