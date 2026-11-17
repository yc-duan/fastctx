// The loopback channel to the node (its `claude::channel`): reports go to it
// in memory as they come, and a request it holds is answered the moment it
// leaves the session something (words, a stop, a control), the file the
// control helper watches no longer the only way to hear of it. The files
// serve where the channel is not reached, as they do for a mod loaded
// before it: once one of a session's reports has gone to a file, every
// later one does, so none overtakes another.
//
// A report the node took is kept here until the node says it dealt with it
// (applied it, or wrote it to the session's files itself). A node that
// started again listens elsewhere and knows nothing of what its predecessor
// took: what is kept goes to the new one first, and the node applies each
// report once whichever way it came.

// How long a wait that failed rests before the channel is asked again,
// from where `channel.json` says it is then.
const CHANNEL_AGAIN_MS = 2000
// The most reports, and bytes of them, kept for a node that has not said it
// dealt with them; past either, the session's reports go to files.
const UNCONFIRMED_MOST = 512
const UNCONFIRMED_BYTES = 32 * 1024 * 1024

// Where the channel is, as the node told this session's directory.
async function channelOpen($, context) {
  const told = await channelTold($, context)
  context.channel = told
  context.channelReports = told !== null
  // What the node took and has not said it dealt with, in the order it went,
  // each with the channel that took it.
  context.unconfirmed = []
  if (told !== null) void channelWait($, context)
}

async function channelTold($, context) {
  try {
    const told = JSON.parse(await $.fs.read(context.dir + '/channel.json'))
    if (told.version !== 2 || !Number.isInteger(told.port) || typeof told.token !== 'string') return null
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

function channelEntry(name, report) {
  return '{"name":' + JSON.stringify(name + '.json') + ',"sequence":' + JSON.stringify(report.sequence ?? null)
    + ',"report":' + report.body + '}'
}

// Hands reports to the node in one request: whether it took every one. The
// reports kept for a channel that is not the current one go first.
async function channelHand($, context, handing) {
  const channel = context.channel
  const body = '{"reports":[' + handing.map(({ name, report }) => channelEntry(name, report)).join(',') + ']}'
  const answer = await channelAsk($, channel, 'reports', { method: 'POST', body })
  if (!answer.ok) throw new Error('the node answered ' + answer.status + ': ' + answer.text)
  const told = JSON.parse(answer.text)
  for (const { name } of handing) {
    if (!told.received.includes(name + '.json')) throw new Error('the node did not take ' + name)
  }
  if (!Number.isInteger(told.queue) || !Number.isInteger(told.dealt) || !Array.isArray(told.receipts) || told.receipts.length !== handing.length) {
    throw new Error('the node gave no receipts for what it took')
  }
  return told
}

// Hands one report to the node: whether it took it. It takes it at once and
// deals with it in turn, so nothing here waits for that. A report it did not
// take, a channel not reached, or more kept than the bounds allow, turns this
// session's reports to files, those kept first (`channelSpill`).
async function channelReport($, context, name, report) {
  const channel = context.channel
  if (!context.channelReports || channel === null) return false
  const again = context.unconfirmed.filter((held) => held.channel !== channel)
  const bytes = context.unconfirmed.reduce((sum, held) => sum + held.report.body.length, report.body.length)
  if (context.unconfirmed.length >= UNCONFIRMED_MOST || bytes > UNCONFIRMED_BYTES) {
    channelToFiles($, context, 'the node has not dealt with ' + context.unconfirmed.length + ' reports')
    return false
  }
  try {
    const handing = [...again, { name, report }]
    const told = await channelHand($, context, handing)
    again.forEach((held, index) => channelTaken(held, channel, told, index))
    const taken = { name, report, channel }
    channelTaken(taken, channel, told, again.length)
    context.unconfirmed.push(taken)
    channelConfirmed(context, told, channel)
    // Kept reports a gone queue took are handed again at the next look,
    // whether or not another report comes.
    if (channelUnsettled(context)) retry($, context)
    return true
  } catch (error) {
    // A stop cuts short what the turn's hooks asked for, this hand-over
    // too: whether the node took it is not known, so it goes to a file, and
    // the node, applying the file first, passes over the hand-over should
    // it have arrived. The channel itself is as good as before.
    if (/abort/i.test(String(error))) return false
    channelToFiles($, context, String(error))
    return false
  }
}

function channelToFiles($, context, why) {
  if (!context.channelReports) return
  context.channelReports = false
  $.ui.log('World channel: this session reports through files from now: ' + why, { to: 'debug' })
}

// Notes that the node took `held` on `channel`, the `index`th of what it
// answered `told` for: the receipt it gave and the queue that gave it. The
// node deals with reports in the order its queue took them, which is not
// their names' order when one is handed again after a later one.
function channelTaken(held, channel, told, index) {
  held.channel = channel
  held.queue = told.queue
  held.receipt = told.receipts[index]
}

// The node's queue `told.queue` dealt with every report it took up to
// receipt `told.dealt`: those are let go of, and only those. A report a
// queue that is gone took was never dealt with by this one, whatever its
// receipts say: it is handed again, in turn (`channelSettle`).
function channelConfirmed(context, told, channel) {
  for (const held of context.unconfirmed) {
    if (held.channel === channel && held.queue !== told.queue) held.channel = null
  }
  context.unconfirmed = context.unconfirmed.filter(
    (held) => held.channel !== channel || held.queue !== told.queue || held.receipt > told.dealt,
  )
}

// What the node took and has not said it dealt with goes to files, in order,
// before anything after it: the node may have gone with it. One it did apply
// is passed over there by its marker. Each leaves the kept ones only once its
// file is whole, so a write that fails leaves it to be written again.
async function channelSpill($, context) {
  const held = context.unconfirmed ?? []
  while (held.length > 0) {
    const { name, report } = held[0]
    if (report.sequence !== undefined && report.sequence !== null) await $.fs.write(context.dir + '/applied/' + report.sequence + '.sequence', '')
    await writeReport($, context.dir + '/events/' + name, report.body)
    await $.fs.write(context.dir + '/events/' + name + '.done', '')
    held.shift()
  }
}

// The channel moved (the node started again) or the node gave up on the
// session's reports: what is kept goes to the node now, in its turn among the
// session's writes, rather than waiting for the next report.
function channelSettle($, context) {
  if ((context.unconfirmed ?? []).length === 0) return
  const settled = context.writing.then(async () => {
    const channel = context.channel
    const again = context.unconfirmed.filter((held) => held.channel !== channel)
    if (context.channelReports && channel !== null && again.length > 0) {
      try {
        const told = await channelHand($, context, again)
        again.forEach((held, index) => channelTaken(held, channel, told, index))
        channelConfirmed(context, told, channel)
        if (channelUnsettled(context)) retry($, context)
        return
      } catch (error) {
        // Given up on the way: tried again at the next look.
        if (/abort/i.test(String(error))) {
          retry($, context)
          return
        }
        channelToFiles($, context, String(error))
      }
    }
    if (!context.channelReports) await channelSpill($, context)
  })
  context.writing = settled.catch((error) => {
    $.ui.log('World channel: keeping what the node took for later: ' + String(error), { to: 'debug' })
    retry($, context)
  })
}

// Whether reports kept for the node still wait to be handed again: taken
// on a channel that is gone, or kept while the session reports through
// files. Each look settles them (`channelSettle`) until none do.
function channelUnsettled(context) {
  const held = context.unconfirmed ?? []
  if (held.length === 0) return false
  if (!context.channelReports) return true
  return held.some((report) => report.channel !== context.channel)
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
// listens elsewhere, with another token, and gets what is kept for it.
async function channelWait($, context) {
  let seen = 0
  while (!context.closed && !gone.seen && context.channel !== null) {
    let woken = null
    try {
      const answer = await channelAsk($, context.channel, 'wake?seen=' + seen)
      if (answer.ok) {
        const told = JSON.parse(answer.text)
        woken = told.wake
        if (told.reports === 'refused') {
          channelToFiles($, context, 'the node refused the channel for this session')
          channelSettle($, context)
        }
      }
    } catch {
      woken = null
    }
    if (woken === null) {
      await new Promise((resolve) => { $.clock.after(CHANNEL_AGAIN_MS, resolve) })
      if (context.closed || gone.seen) return
      const told = await channelTold($, context)
      if (told === null) {
        // The node offers no channel this mod can use any more (it stopped,
        // or one of another version started): what it took and did not deal
        // with goes to files, and so does every report after it.
        channelToFiles($, context, 'the node offers no channel this session can use')
        context.channel = null
        channelSettle($, context)
        return
      }
      if (context.channel === null || told.base !== context.channel.base || told.token !== context.channel.token) {
        context.channel = told
        seen = 0
        channelSettle($, context)
      }
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
