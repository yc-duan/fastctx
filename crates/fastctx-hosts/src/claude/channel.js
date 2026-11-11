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
  // What the node took and has not said it applied, in the order it went.
  context.unconfirmed = []
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

// Hands one report to the node: whether it took it. It takes it at once
// and applies it in turn, so nothing here waits for that; it is kept here
// until the node says it applied it. A report it did not take, or a channel
// not reached, turns this session's reports to files, those it took and
// has not applied first (`channelSpill`).
async function channelReport($, context, name, report) {
  const channel = context.channel
  if (!context.channelReports || channel === null) return false
  try {
    const body = '{"reports":[{"name":' + JSON.stringify(name + '.json') + ',"sequence":'
      + JSON.stringify(report.sequence ?? null) + ',"report":' + report.body + '}]}'
    const answer = await channelAsk($, channel, 'reports', { method: 'POST', body })
    if (!answer.ok) throw new Error('the node answered ' + answer.status + ': ' + answer.text)
    const told = JSON.parse(answer.text)
    if (!told.received.includes(name + '.json')) throw new Error('the node did not take ' + name)
    context.unconfirmed.push({ name, report })
    channelConfirmed(context, told.applied)
    return true
  } catch (error) {
    if (context.channelReports) {
      context.channelReports = false
      $.ui.log('World channel: this session reports through files from now: ' + String(error), { to: 'debug' })
    }
    return false
  }
}

// The node applied every report up to `applied`: they are let go of.
function channelConfirmed(context, applied) {
  if (typeof applied !== 'string') return
  const through = applied.replace(/\.json$/, '')
  context.unconfirmed = context.unconfirmed.filter((held) => held.name > through)
}

// What the node took and has not said it applied goes to files, in order,
// before anything after it: the node may have gone with it. One it did apply
// is passed over there by its marker.
async function channelSpill($, context) {
  const held = context.unconfirmed ?? []
  context.unconfirmed = []
  for (const { name, report } of held) {
    if (report.sequence !== undefined && report.sequence !== null) await $.fs.write(context.dir + '/applied/' + report.sequence + '.sequence', '')
    await writeReport($, context.dir + '/events/' + name, report.body)
    await $.fs.write(context.dir + '/events/' + name + '.done', '')
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
