async function sessionPolicy($, context) {
  if (context.policy !== undefined) return context.policy
  const child = $.process.spawn({ argv: [FASTCTX, 'claude-session-policy', WORLD, context.sessionId] })
  let stdout = '', stderr = ''
  for await (const piece of child) {
    if (piece.stream === 'stdout') stdout += piece.text
    else stderr += piece.text
  }
  const result = await child.result
  if (result.code !== 0) throw new Error('reading the Claude session policy: ' + stderr)
  context.policy = JSON.parse(stdout)
  const state = await controlBegin($, sessions)
  if (state !== null && Object.keys(state.selection).length === 0) {
    if (context.policy.model != null) state.selection.model = context.policy.model
    if (context.policy.effort != null) state.selection.effort = context.policy.effort
  }
  return context.policy
}

async function observeModels($, context) {
  const rows = await $.config.list()
  if (!rows.some((row) => row.key === 'model')) return
  const models = modelOffers(rows)
  await $.fs.write(context.dir + '/models.json', JSON.stringify({ at: stamp(), available: true, models }))
}

async function observeTitle($, context, e) {
  if (e.session_title === undefined || e.agent_id !== undefined) return
  await event($, context, 'session', { happened: { event: 'titled', title: e.session_title } })
}

function agentReports(base, agentId) {
  const wrap = (happened) => ({ event: 'changed', turn: null,
    change: { kind: 'agent_event', agent_id: agentId, happened } })
  return {
    event: async (kind, fields) => {
      if (kind === 'session') return base.event('session', { happened: wrap(fields.happened) })
      return base.event('agent', { agent_id: agentId, report: { kind, ...fields } })
    },
    stream: (happened) => base.stream(wrap(happened)),
    stored: () => {}, thought: () => {}, save: async () => {},
  }
}

function agentPresentation(context, id) {
  context.agents ??= new Map()
  if (!context.agents.has(id)) context.agents.set(id, presentation(() => reserved(context), () => null))
  return context.agents.get(id)
}

async function observeAgents($, context) {
  const agents = await $.agent.list()
  await event($, context, 'session', { happened: { event: 'changed', turn: null, change: { kind: 'agents', agents } } })
}
