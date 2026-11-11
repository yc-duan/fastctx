// The loopback channel to the node (its `claude::channel`): reports go to it
// in memory as they come, and a request it holds is answered the moment it
// leaves the session something (words, a stop, a control), the file the
// control helper watches no longer the only way to hear of it. The files
// serve where the channel is not reached, as they do for a mod loaded
// before it: once one of a session's reports has gone to a file, every
// later one does, so none overtakes another.

// How long a wait that failed rests before the channel is asked again,
// from where `channel.json` says it is then.
const CHANNEL_AGAIN_MS = 2000

// Where the channel is, as the node told this session's directory.
async function channelOpen($, context) {
  const told = await channelTold($, context)
  context.channel = told
  context.channelReports = told !== null
  if (told !== null) void channelWait($, context)
}

async function channelTold($, context) {
  try {
    const told = JSON.parse(await $.fs.read(context.dir + '/channel.json'))
    if (told.version !== 1 || !Number.isInteger(told.port) || typeof told.token !== 'string') return null
    return {
      base: 'http://127.0.0.1:' + told.port + '/v1/claude/' + encodeURIComponent(context.sessionId) + '/',
      token: told.token,
    }
  } catch {
    return null
  }
}

function channelAsk($, channel, what, init = {}) {
  return $.http.fetch(channel.base + what, {
    ...init,
    headers: { authorization: 'Bearer ' + channel.token, 'content-type': 'application/json' },
  })
}

// Hands one report to the node: whether it took it. A report it did not
// take, or a channel not reached, turns this session's reports to files.
async function channelReport($, context, name, report) {
  const channel = context.channel
  if (!context.channelReports || channel === null) return false
  try {
    const body = '{"reports":[{"name":' + JSON.stringify(name + '.json') + ',"sequence":'
      + JSON.stringify(report.sequence ?? null) + ',"report":' + report.body + '}]}'
    const answer = await channelAsk($, channel, 'reports', { method: 'POST', body })
    if (!answer.ok) throw new Error('the node answered ' + answer.status + ': ' + answer.text)
    if (JSON.parse(answer.text).applied.includes(name + '.json')) return true
    throw new Error('the node did not take ' + name)
  } catch (error) {
    if (context.channelReports) {
      context.channelReports = false
      $.ui.log('World channel: this session reports through files from now: ' + String(error), { to: 'debug' })
    }
    return false
  }
}

// The account's subagents for this session, as `fastctx claude-subagents`
// prints them; null where the channel is not reached.
async function channelSubagents($, context) {
  const channel = context.channel
  if (channel === null || channel === undefined) return null
  try {
    const answer = await channelAsk($, channel, 'subagents')
    return answer.ok ? JSON.parse(answer.text).configuration ?? null : null
  } catch {
    return null
  }
}

// Waits for the node to leave the session something, over and over, and
// looks as the control helper's `look` has it look. A wait that fails rests
// a while and asks again where the channel is: a node that started again
// listens elsewhere, with another token.
async function channelWait($, context) {
  let seen = 0
  while (!context.closed && !gone.seen && context.channel !== null) {
    let woken = null
    try {
      const answer = await channelAsk($, context.channel, 'wake?seen=' + seen)
      if (answer.ok) woken = JSON.parse(answer.text).wake
    } catch {
      woken = null
    }
    if (woken === null) {
      await new Promise((resolve) => { $.clock.after(CHANNEL_AGAIN_MS, resolve) })
      if (context.closed || gone.seen) return
      const told = await channelTold($, context)
      if (told === null) continue
      context.channel = told
      continue
    }
    if (woken === seen) continue
    seen = woken
    const state = sessions.get(context.sessionId)
    if (state !== undefined && !state.closed) void controlPoll($, state)
    const looking = wakes.get(context.sessionId)
    if (looking !== undefined) void looking()
  }
}
