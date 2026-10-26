import { test, expect, mock } from 'claude-code/testing'

// A loader/engine contract test: fixtures stand for host facts. This does
// not assert account eligibility, model execution or CLI forking behavior.
test('uses the public controls and request return types through the official engine', async ($, on) => {
  mock.clock(on)
  mock.env(on, {})
  on('ui.log', () => ({ value: undefined }))
  on('ui.status', () => ({ value: undefined }))
  on('ui.toast', () => ({ value: undefined }))
  on('ui.invalidate', () => ({ value: undefined }))
  const files = new Map<string, string>()
  const markers = new Map<string, () => void>()
  const normalize = (path: string) => path.replaceAll('\\', '/')
  let model = 'opus'
  let busy = false
  let skip = false
  let ownerAlive = true
  // Where the session is drawn: a terminal, or nowhere, as a background one.
  let surfaces: string[] = ['terminal']
  const commands: { command: string; args: string }[] = []
  const questions: any[] = []
  let requestAnswer: any = { kind: 'pass', reason: 'no watcher' }
  let cancelledQuestions = 0
  // The control helper says `look` when the node leaves the session something.
  let wakeGuard = () => {}
  on('mcp.connect', () => ({ value: { isConnected: true, server: 'fixture:world' } }))
  on('session.id', () => ({ value: 'control-fixture' }))
  on('session.cwd', () => ({ value: '/fixture' }))
  on('settings.read', () => ({ value: {} }))
  on('session.model', () => ({ value: model }))
  on('session.messages', () => ({ value: [] }))
  on('session.surfaces', () => ({ value: surfaces }))
  on('session.version', () => ({ value: { version: '2.1.294' } }))
  on('process.run', (_, e) => ({ value: { exitCode: 0, stdout: JSON.stringify(e.argv[1] === 'claude-session-directory' ? e.argv[2] + '/claude/' + e.argv[3] : e.argv[1] === 'claude-control-owner' && !ownerAlive ? null : { pid: 123, token: 'lifetime' }), stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }))
  on('fs.write', (_, e) => { const path = normalize(e.path); files.set(path, e.text); markers.get(path)?.(); return { value: undefined } })
  on('fs.read', (_, e) => {
    const path = normalize(e.path)
    if (!files.has(path)) return { deny: 'fixture: file unavailable' }
    return { value: files.get(path)! }
  })
  on('fs.list', (_, e) => {
    const prefix = normalize(e.path) + '/'
    return { value: [...files.keys()].filter((path) => path.startsWith(prefix) && !path.slice(prefix.length).includes('/'))
      .map((path) => ({ name: path.slice(prefix.length), kind: 'file', size: files.get(path)!.length, mtimeMs: 0, isLink: false })) }
  })
  on('session.start', (_, e) => ({ cwd: e.cwd }))
  on('session.end', (_, e) => ({ sessionId: e.sessionId }))
  on('turn.start', (_, e) => { busy = true; return { turnId: e.turnId } })
  on('turn.complete', (_, e) => { busy = false; return { text: e.answer } })
  on('session.compact', () => {
    if (busy) throw new Error('compact while busy')
    return skip ? { skip: 'host veto' } : { messages: [{ role: 'user', text: 'summary', toolUses: [] }], tokensBefore: 100, tokensAfter: 30 }
  })
  on('command.list', () => ({ value: [
    { name: 'model', description: 'Model', source: 'builtin' },
    { name: 'rename', description: 'Name', source: 'builtin' },
    { name: 'effort', description: 'Effort', source: 'builtin' },
    { name: 'fixture:skill', description: 'Skill', source: 'plugin', plugin: 'fixture' },
  ] }))
  on('config.list', () => ({ value: [{ key: 'model', label: 'Model', kind: 'choice', value: 'opus', options: ['default', 'opus', 'sonnet'], provider: { plugin: 'engine', tier: 'core' }, isLocked: false }] }))
  on('config.set', (_, e) => { if (e.key === 'model') model = String(e.value); return { value: e.value } })
  on('session.usage', () => ({ value: { startedAt: 1, context: { tokens: 123, window: 200000, breakdown: { mcpTools: [{ name: 'mcp__fixture_world__run', serverName: 'fixture_world', isLoaded: true, tokens: 1 }] } }, rateLimits: [{ kind: 'five_hour', percentUsed: 12.5, resetsAt: '2026-10-04T17:00:00Z' }], cost: { usd: 0.125 } } }))
  on('command.run', (_, e) => {
    expect(e.origin).toEqual(expect.objectContaining({ kind: 'plugin' }))
    if (e.command !== 'goal') commands.push({ command: e.command, args: e.args })
    if (e.command === 'model') model = e.args
    if (e.command === 'bad') throw new Error('unknown command')
    return { text: e.command === 'panel' ? 'This command requires the Claude Code terminal' : e.command + ':' + e.args }
  })
  on('process.spawn', async function* (_, e, next) {
    if (e.argv[1] === 'claude-native-goal') {
      yield { stream: 'stdout', text: JSON.stringify({ offset: 0 }) }
      return { value: { code: 0, signal: null } }
    }
    if (e.argv[1] === 'claude-session-policy') {
      yield { stream: 'stdout', text: JSON.stringify({ model: null, effort: null }) }
      return { value: { code: 0, signal: null } }
    }
    if (e.argv[1] === 'claude-control-guard') {
      expect(e.argv[2]).toBe('--watch')
      expect(normalize(e.argv[3])).toBe(normalize(e.argv[5]) + '/world/claude/control-fixture')
      const text = JSON.stringify({ process: { pid: 124, token: 'activation' }, parent: { pid: 123, token: 'lifetime' } }) + '\n'
      yield { stream: 'stdout', text: text.slice(0, 5) }
      yield { stream: 'stdout', text: text.slice(5) }
      while (await new Promise<boolean>((resolve) => {
        wakeGuard = () => resolve(true)
        next.signal.addEventListener('abort', () => resolve(false), { once: true })
      })) yield { stream: 'stdout', text: 'look\n' }
      return { value: { code: 0, signal: null } }
    }
    const request = JSON.parse(e.input!)
    questions.push(request)
    const text = JSON.stringify(requestAnswer)
    yield { stream: 'stdout', text: text.slice(0, 7) }
    yield { stream: 'stdout', text: text.slice(7) }
    return { value: { code: 0, signal: null } }
  })
  on('tool.check', (_, e) => ({ decision: e.tool === 'permission-tool' ? 'ask' : 'allow' }))
  on('tool.call', async (_, e, next) => {
    if (e.tool === 'permission-tool') {
      const decision = await $.classic.PermissionRequest({ tool_name: e.tool, tool_input: { path: '/file' } })
      return { result: decision }
    }
    if (e.tool === 'AskUserQuestion' && requestAnswer.kind === 'answer') {
      await new Promise<void>((resolve) => next.signal.addEventListener('abort', () => { cancelledQuestions++; resolve() }, { once: true }))
      return { result: { cancelled: true } }
    }
    return { result: { native: true } }
  })
  await $.session.start({ cwd: '/fixture', surface: 'terminal', isInteractive: true })
  const hostPath = [...files.keys()].find((path) => path.endsWith('/control/host.json'))!
  const directory = hostPath.slice(0, -'/host.json'.length)
  const host = JSON.parse(files.get(hostPath)!)
  let sequence = 0
  async function invoke(control: any) {
    const key = 'control-' + ++sequence
    files.set(directory + '/inbox/' + key + '.json', JSON.stringify({ instance: host.instance, controller: { pid: 123, token: 'lifetime' }, control }))
    const done = new Promise<void>((resolve) => markers.set(directory + '/replies/' + key + '.done', resolve))
    wakeGuard()
    await done
    markers.delete(directory + '/replies/' + key + '.done')
    expect(files.has(directory + '/replies/' + key + '.done')).toBe(true)
    return JSON.parse(files.get(directory + '/replies/' + key + '.json')!)
  }
  const offers = (await invoke({ do: 'offers' })).ok
  expect(offers.model).toBe('opus')
  expect(offers.effort).toBe(null)
  expect(offers.usage.context).toEqual({ used: 123, window: 200000 })
  expect(offers.usage.cost_usd).toBe(0.125)
  expect(offers.commands.some((command: any) => command.source === 'plugin')).toBe(false)
  expect((await invoke({ do: 'command', name: 'panel', args: '' })).error).toContain('background-safe')
  expect((await invoke({ do: 'command', name: 'bad', args: '' })).error).toContain('background-safe')
  expect((await invoke({ do: 'model', model: 'sonnet', effort: 'high' })).ok).toEqual({ kind: 'selected', model: 'sonnet', effort: 'high' })
  expect(model).toBe('opus')
  expect((await invoke({ do: 'model', model: 'sonnet', effort: null })).ok.model).toBe('sonnet')
  expect(commands).toEqual([])
  expect((await invoke({ do: 'rename', name: '新名字' })).ok.said).toBe('rename:新名字')
  expect((await invoke({ do: 'rename', name: '新名字' })).ok.kind).toBe('reported')
  ownerAlive = false
  const ran = commands.length
  expect((await invoke({ do: 'rename', name: 'abandoned' })).error).toContain('exited')
  expect(commands.length).toBe(ran)
  ownerAlive = true
  expect((await invoke({ do: 'compact' })).ok).toEqual({ kind: 'done', said: null })
  skip = true
  expect((await invoke({ do: 'compact' })).ok).toEqual({ kind: 'refused', said: 'host veto' })
  await $.turn.start({ turnId: 'turn-1', text: 'work' })
  expect((await invoke({ do: 'compact' })).error).toContain('between turns')
  const question = { question: 'Which?', header: 'Pick', options: [{ label: 'A', description: 'a' }, { label: 'B', description: 'b' }], multiSelect: true }
  const native = await $.tool.call({ tool: 'AskUserQuestion', tool_use_id: 'q1', questions: [question] })
  expect(native.result).toEqual({ native: true })
  requestAnswer = { kind: 'answer', answer: { result: { questions: [question], answers: { 'Which?': 'A, B' } } } }
  const answered = await $.tool.call({ tool: 'AskUserQuestion', tool_use_id: 'q2', questions: [question] })
  expect(answered.result).toEqual(requestAnswer.answer.result)
  expect(cancelledQuestions).toBe(1)
  const answeredQuestion = questions.find((request) => request.params.tool_use_id === 'q2')!
  expect(answeredQuestion.native_wait).toBe(true)
  expect(answeredQuestion.params.questions).toEqual([question])
  requestAnswer = { kind: 'answer', answer: { decision: 'allow' } }
  // Drawn in a terminal, the ask stays Claude Code's own: the World is not asked.
  const inTerminal = await $.tool.check({ tool: 'permission-tool', tool_use_id: 'p0', input: { path: '/file' } })
  expect(inTerminal.decision).toBe('ask')
  expect(questions.some((request) => request.params.tool_use_id === 'p0')).toBe(false)
  // Drawn nowhere, the World answers it.
  surfaces = []
  const permission = await $.tool.check({ tool: 'permission-tool', tool_use_id: 'p1', input: { path: '/file' } })
  expect(permission).toEqual(requestAnswer.answer)
  const permissionQuestion = questions.find((request) => request.params.tool_use_id === 'p1')!
  expect(permissionQuestion.request).toBe('tool.check')
  expect(answeredQuestion.key).not.toBe(permissionQuestion.key)
  await $.turn.complete({ turnId: 'turn-1', answer: '', reason: 'answer', isAborted: false, durationMs: 0 })
  await $.session.end({ sessionId: 'control-fixture', reason: 'prompt_input_exit', resume: { id: 'control-fixture' } })
  expect(files.has(directory + '/closed/' + host.instance + '.json')).toBe(true)
})
