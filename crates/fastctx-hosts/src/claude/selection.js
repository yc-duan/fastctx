const effortLevels = ['low', 'medium', 'high', 'xhigh', 'max']

// The model aliases this login's Claude Code offers (`opus`, `fable`, …).
// They only tell the World which families the account has: the World offers
// its own table of full model ids for those families, and never sends an
// alias, which a request's model does not resolve.
function modelOffers(rows) {
  return (rows.find((row) => row.key === 'model')?.options ?? []).filter((id) => id !== 'default').map((id) => ({
    id, name: null, description: null, efforts: [], default_effort: null,
    default: false, service_tiers: [], default_service_tier: null,
  }))
}

async function controlSelection($, state, model, effort) {
  if (model !== null && model !== undefined && !model.trim()) throw controlRefusal('a model cannot be empty')
  if (effort !== null && effort !== undefined && !effortLevels.includes(effort)) throw controlRefusal('unknown Claude reasoning effort: ' + effort)
  const updated = (state.selecting ?? Promise.resolve()).then(async () => {
    const selection = { ...state.selection }
    if (model !== null && model !== undefined) selection.model = model
    if (effort !== null && effort !== undefined) selection.effort = effort
    await controlWrite($, state.directory + '/selection.json', selection)
    state.selection = selection
    if (model !== null && model !== undefined) {
      await controlWrite($, state.directory + '/routing.json', null)
      state.routing = null
      state.automaticModel = false
      state.routedModel = undefined
    }
    return { kind: 'selected', model: selection.model ?? null, effort: selection.effort ?? null }
  })
  state.selecting = updated.catch(() => {})
  return updated
}

async function controlFollowEngine($, state) {
  const updated = (state.selecting ?? Promise.resolve()).then(async () => {
    const selection = { ...state.selection }
    delete selection.model
    await controlWrite($, state.directory + '/selection.json', selection)
    state.selection = selection
    await controlWrite($, state.directory + '/routing.json', null)
    state.routing = null
    state.automaticModel = false
    state.routedModel = undefined
  })
  state.selecting = updated.catch(() => {})
  return updated
}

async function controlStep($, e) {
  if (e.agentId !== undefined) return e
  const state = await controlBegin($, sessions)
  if (state === null) return e
  if (state.engineTurn !== e.turnId) {
    state.engineTurn = e.turnId
    state.engineModel = e.model
  } else if (state.engineModel === undefined) {
    state.engineModel = e.model
  } else if (state.engineModel !== e.model) {
    // A changed engine route wins over the request override for this turn.
    state.automaticModel = true
    state.routedModel = e.model
  }
  state.observedEffort = e.effort
  if (state.automaticModel) return e
  const input = { ...e }
  if (state.selection.model !== undefined) input.model = state.selection.model
  if (state.selection.effort !== undefined) input.effort = state.selection.effort
  return input
}

async function controlModelSwitch($, e) {
  if (e.agent_id !== undefined) return
  const state = await controlBegin($, sessions)
  if (state === null) return
  if (['command', 'picker', 'sdk'].includes(e.source)) {
    // The user chose a model inside Claude Code itself: its engine now runs
    // that choice, so the World stops naming a model for the requests.
    await controlFollowEngine($, state)
  } else if (e.source === 'auto') {
    const routing = { model: e.to_model, source: e.source }
    await controlWrite($, state.directory + '/routing.json', routing)
    state.routing = routing
    state.automaticModel = true
    state.routedModel = e.to_model
  }
}
