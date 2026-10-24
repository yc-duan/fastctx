// A pause retains old input; a new explicit user message may still start ordinary conversation work.
async function readGoal($, context) {
  try {
    const files = await $.fs.list(context.dir)
    let report = context.goalStopReport
    if (!report && files.some((entry) => entry.name === 'goal-stop.json')) {
      report = JSON.parse(await $.fs.read(context.dir + '/goal-stop.json')).report
      if (typeof report !== 'string') throw new Error('the goal stop report is invalid')
    }
    let goalStopping = context.goalStopping === true
    if (report && context.goalStopAcknowledged !== report) {
      const applied = await $.fs.list(context.dir + '/applied')
      goalStopping = !applied.some((entry) => entry.name === report)
    }
    // The node freezes queued input before acknowledging an interrupted turn.
    // Read its state after the acknowledgment so a stale active view cannot flush it.
    let state
    if (!files.some((entry) => entry.name === 'goal.json')) {
      state = { active: false, paused: false, held: [] }
    } else {
      state = JSON.parse(await $.fs.read(context.dir + '/goal.json'))
      if (typeof state.paused !== 'boolean' || typeof state.active !== 'boolean') throw new Error('the goal state is invalid')
      if (state.held !== undefined && state.held !== null && (!Array.isArray(state.held) || state.held.some((id) => typeof id !== 'string'))) throw new Error('the goal held inputs are invalid')
    }
    context.goal = state
    context.goalStopping = goalStopping
    if (report && !goalStopping) {
      context.goalStopAcknowledged = report
      context.goalStopReport = null
    }
  } catch (error) {
    worldFailure($, 'goal state', error)
  }
  return context.goal
}

function goalPermits(context, delivery) {
  if (context.goalStopping === true) return false
  if (context.goal?.paused !== true) return true
  return delivery.as_user === true && Array.isArray(context.goal.held) && !context.goal.held.includes(delivery.id)
}

async function goalPermitsAll($, context, deliveries) {
  await readGoal($, context)
  return deliveries.every((delivery) => goalPermits(context, delivery))
}
