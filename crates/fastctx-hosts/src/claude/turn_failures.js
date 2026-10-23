// Classic prompt ids can cover several turns. Ambiguous failures stay unassigned.
function notePrompt(context, input) {
  if (input.agent_id !== undefined || typeof input.prompt_id !== 'string') return
  context.activePrompt = input.prompt_id
  context.promptTurns ??= new Map()
  if (!context.promptTurns.has(input.prompt_id)) context.promptTurns.set(input.prompt_id, new Set())
}

function noteTurnPrompt(context, input) {
  if (input.agentId !== undefined || !context.activePrompt) return
  context.promptTurns.get(context.activePrompt)?.add(input.turnId)
}

function captureStopFailure(context, input) {
  const candidates = context?.promptTurns?.get(input.prompt_id)
  const turn = input.agent_id === undefined && candidates?.size === 1 ? candidates.values().next().value : null
  const error = { category: input.error ?? null, message: input.error_details ?? null,
    session_id: input.session_id, prompt_id: input.prompt_id ?? null, agent_id: input.agent_id ?? null }
  if (turn !== null) {
    context.turnFailures ??= new Map()
    context.turnFailures.set(turn, error)
  }
  return { turn, error }
}

async function reportTurnFailure($, context, failure) {
  await event($, context, 'session', { happened: { event: 'changed', turn: failure.turn,
    change: { kind: 'metadata', metadata: { fastctx_turn_failure: failure } } } })
}

async function noteStepStop($, context, input, result) {
  if (input.agentId !== undefined || result?.stopReason !== 'model_context_window_exceeded') return
  const error = { category: result.stopReason, message: result.stopReason }
  context.turnFailures ??= new Map()
  context.turnFailures.set(input.turnId, error)
  await reportTurnFailure($, context, { turn: input.turnId, error })
    .catch((failure) => worldFailure($, 'turn failure report', failure))
}

function completedFailure(context, input) {
  const error = context.turnFailures?.get(input.turnId) ?? null
  if (input.reason === 'error' || error?.category === 'model_context_window_exceeded') return error
  return null
}
