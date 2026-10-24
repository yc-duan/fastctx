// The public command API clears a restored native goal and its persistent
// Stop hook. session.start is held by the host, so cleanup starts after that
// callback returns and is retried after a turn. The host may evaluate an old
// restored goal before it first becomes idle; no public goal state API exists.
const NATIVE_GOAL_DISABLED = 'Use the conversation goal in Fastctx.'
const nativeGoalContexts = new Map()

function internalGoalClear($, event) {
  return event.command === 'goal' && event.args.trim() === 'clear' &&
    event.origin?.kind === 'plugin' && event.origin.name === $.plugin.name
}

async function clearNativeGoal($, context) {
  if (context.nativeGoalClearing) return context.nativeGoalClearing
  if (context.nativeGoalCleared || context.closed || context.busy) return
  const clearing = clearNativeGoalOnce($, context)
  context.nativeGoalClearing = clearing
  try { await clearing }
  catch (error) { worldFailure($, 'clearing the native goal', error) }
  finally { context.nativeGoalClearing = null }
}

async function clearNativeGoalOnce($, context) {
  if (await removed($) || await $.session.id() !== context.sessionId) return
  const before = await nativeGoalProgress($, context)
  const result = await $.command.run({ command: 'goal', args: 'clear' })
  if (result.exitCode !== undefined && result.exitCode !== 0) throw new Error(result.text || 'Native goal cleanup failed')
  // command.run resolves before its local-command rows are persisted. Those
  // rows change the parent UUID that a subsequent compact must consume.
  const after = await nativeGoalProgress($, context, before.offset)
  const state = sessions.get(context.sessionId)
  if (state && after.uuid) state.lastRawUuid = after.uuid
  context.nativeGoalCleared = true
}

async function nativeGoalProgress($, context, after) {
  const argv = [FASTCTX, 'claude-native-goal', WORLD, context.sessionId]
  if (after !== undefined) argv.push('--after', String(after))
  const progress = $.process.spawn({ argv })
  let stdout = '', stderr = ''
  for await (const piece of progress) {
    if (piece.stream === 'stdout') stdout += piece.text
    else stderr += piece.text
  }
  if ((await progress.result).code !== 0) throw new Error('Observing native goal cleanup: ' + stderr)
  return JSON.parse(stdout)
}

function scheduleNativeGoalClear($, context) {
  if (context.nativeGoalScheduled || context.nativeGoalCleared) return
  context.nativeGoalScheduled = true
  $.clock.after(0, () => {
    context.nativeGoalScheduled = false
    void clearNativeGoal($, context).catch(error => worldFailure($, 'clearing the native goal', error))
  })
}
