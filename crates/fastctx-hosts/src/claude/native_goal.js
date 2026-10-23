// The public command API clears a restored native goal and its persistent
// Stop hook. session.start is held by the host, so cleanup starts after that
// callback returns and is retried after a turn. The host may evaluate an old
// restored goal before it first becomes idle; no public goal state API exists.
const NATIVE_GOAL_DISABLED = 'Use the conversation goal in Fastctx.'

function internalGoalClear($, event) {
  return event.command === 'goal' && event.args.trim() === 'clear' &&
    event.origin?.kind === 'plugin' && event.origin.name === $.plugin.name
}

async function clearNativeGoal($, context) {
  if (context.nativeGoalCleared || context.nativeGoalClearing || context.closed || context.busy || await removed($)) return
  if (await $.session.id() !== context.sessionId) return
  context.nativeGoalClearing = true
  try {
    const result = await $.command.run({ command: 'goal', args: 'clear' })
    if (result.exitCode !== undefined && result.exitCode !== 0) throw new Error(result.text || 'Native goal cleanup failed')
    context.nativeGoalCleared = true
  } catch (error) {
    worldFailure($, 'clearing the native goal', error)
  } finally {
    context.nativeGoalClearing = false
  }
}

function scheduleNativeGoalClear($, context) {
  if (context.nativeGoalScheduled || context.nativeGoalCleared) return
  context.nativeGoalScheduled = true
  $.clock.after(0, () => {
    context.nativeGoalScheduled = false
    void clearNativeGoal($, context).catch(error => worldFailure($, 'clearing the native goal', error))
  })
}
