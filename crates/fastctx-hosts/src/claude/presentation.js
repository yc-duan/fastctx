import { taskSteps } from './steps.js'

export function observedMilliseconds(value) {
  return Number.isSafeInteger(value) && value >= 0 ? value : null
}

// How words the World handed over begin, as `claude::is_framed` knows them:
// its own telling, before 2026-10-09 and since, and the user's words sent
// into a running turn.
const WORLD_FRAMES = [
  'A message from fastctx, not typed by the user:',
  'The fastctx plugin delivers a message from the World.',
  'The user sent a new message while you were working:',
  'The user also sent this message, before the one that follows:',
]

// The user's words as they were said, from a prompt the World's plugin
// submitted for them, one zero-width space before a leading "/" fewer
// (`world.js`, `asTyped`; `claude::as_said`).
function asSaid(text) {
  return /^\u200B+\//.test(text) ? text.slice(1) : text
}

// The session's observable presentation. Transport and inbox delivery live
// in world.js; this module never submits a prompt or changes a host result.
// Each handler receives reporting callbacks bound to its original session.
// stream queues observations without waiting; event/save confirm persistence.
//
// A block of prose begins with its first text or thinking chunk and ends
// when Claude Code stores its row (session.append), which the engine does as
// the block closes, before the next block streams (Claude Code 2.1.296).
// Thinking begins earlier than any chunk shows: the block's start reaches a
// hook only as an opaque engine chunk, and its first thinking chunk comes
// with the first progress estimate, with the first summary, or not at all,
// as the session's thinking display has it. How long it took is Claude
// Code's own figure on the stored row; the node reads it there (`thought`).
export function presentation(reserve, currentTurn) {
  const tools = new Map()
  const requests = new Map()
  const rows = new Set()
  // The agents this conversation started: the notice Claude Code writes the
  // model when one of them ends is shown as that end, as the session's
  // record is read (`subagents.rs`), not as words the user said.
  const agents = new Set()
  // The steps whose streamed blocks may still be claimed by the rows Claude
  // Code stores of them, oldest first: a row can come after the next step
  // began, and the plugin's own idea of the running turn can lag behind a
  // turn the host began (a start hook still waiting), so a row is matched by
  // the steps themselves, not by that idea.
  // Each step also counts the rows stored of its response (`next`) against the
  // highest block index its stream named (`last`): a response's blocks are
  // stored one row each, in order, so the row a step stores next is its block
  // at that index, streamed or not.
  const recent = []
  const RECENT_STEPS = 16
  let latestStep = null
  let agentTurn = null
  // The turn now: the plugin's own idea of it, or the turn the latest step
  // ran in while that idea has not caught up.
  const turnNow = () => currentTurn() ?? latestStep?.turn ?? null
  const steps = taskSteps()
  const emit = (reports, happened) => reports.event('session', { happened })
  const item = (id, kind, fields = {}) => ({ id, kind, started_at: reserve().at, ...fields })
  const shownOf = ({ ended, stored, ...shown }) => shown
  // The earliest streamed block of `kind` no row has claimed, in the oldest
  // recent step that has one and ran in `turn` when a recent step ran in it
  // (an idea of the turn that names none of them has fallen behind, and says
  // nothing). A claim in a step lets go of the steps before it: their rows
  // are all in.
  const claim = (kind, turn) => {
    const known = turn != null && recent.some((step) => step.turn === turn)
    for (const [at, step] of recent.entries()) {
      if (step.failed || (known && step.turn !== turn)) continue
      const shown = [...step.blocks.values()].find((block) => block.kind === kind && !block.stored)
      if (shown === undefined) continue
      recent.splice(0, at)
      return { step, shown }
    }
    return undefined
  }
  // The step whose response a stored row is the next block of: the oldest
  // recent one with blocks still to be stored, or the latest.
  const owner = (turn) => {
    const known = turn != null && recent.some((step) => step.turn === turn)
    for (const [at, step] of recent.entries()) {
      if (step.failed || (known && step.turn !== turn)) continue
      if (step.next <= step.last || at === recent.length - 1) {
        // The earlier steps of its turn have all their rows in.
        for (let index = at - 1; index >= 0; index--) if (recent[index].turn === step.turn) recent.splice(index, 1)
        return step
      }
    }
    return undefined
  }
  const ledger = () => recent.map((step) => ({
    turn: step.turn, failed: step.failed, next: step.next, last: step.last,
    blocks: [...step.blocks.entries()].map(([key, block]) => [key, {
      id: block.id, kind: block.kind, started_at: block.started_at, stored: block.stored === true, ended: block.ended === true,
    }]),
  }))
  const checkpoint = (reports) => reports.save({ tools: [...tools.entries()], requests: [...requests.entries()], tasks: steps.snapshot(), agents: [...agents], recent: ledger() })

  async function completeTool(reports, call, stored) {
    const result = call.result
    const { content, ...header } = stored.block
    await reports.event('tool_stored', {
      turn: call.turn,
      item: call.id,
      tool: call.tool,
      input: call.input,
      started_at: call.started_at,
      content: content ?? null,
      failed: stored.block.is_error === true,
      declined: result?.deny !== undefined,
      ms: observedMilliseconds(call.ms),
      detail: { returned: result, stored: { uuid: stored.uuid, block: header }, requested: requests.get(call.id)?.block },
    })
    tools.delete(call.id)
    requests.delete(call.id)
    await checkpoint(reports)
  }

  return {
    restore(state) {
      for (const [id, call] of state.tools) tools.set(id, call)
      for (const [id, request] of state.requests) requests.set(id, request)
      steps.restore(state.tasks)
      for (const id of state.agents ?? []) agents.add(id)
      // The blocks recent steps streamed, so rows stored after a reload still
      // find the drafts they were shown as.
      for (const kept of state.recent ?? []) {
        const blocks = new Map()
        for (const [key, block] of kept.blocks ?? []) {
          blocks.set(key, { ...block, text: '', ...(block.stored ? {} : { stored: undefined }), ...(block.ended ? {} : { ended: undefined }) })
        }
        recent.push({ turn: kept.turn, blocks, failed: kept.failed === true, next: kept.next ?? 0, last: kept.last ?? -1 })
      }
      while (recent.length > RECENT_STEPS) recent.shift()
      latestStep = recent[recent.length - 1] ?? null
    },
    /** One of the host's own agents started in this conversation. */
    spawned(id) {
      agents.add(id)
    },
    async *step(reports, e, next) {
      const stream = next(e)
      if (e.agentId !== undefined) agentTurn = e.turnId
      reports.stream({ event: 'changed', turn: e.turnId, change: { kind: 'model_request', step: e.index,
        model: e.model, effort: e.effort ?? null } })
      const blocks = new Map()
      let responseReported = false
      let observedUsage = null
      latestStep = { turn: e.turnId, blocks, failed: false, next: 0, last: -1 }
      recent.push(latestStep)
      if (recent.length > RECENT_STEPS) recent.shift()
      const step = latestStep
      try {
        for await (const chunk of stream) {
          if (Number.isInteger(chunk.index) && chunk.index > step.last) step.last = chunk.index
          if (chunk.kind === 'stop') {
            reports.stream({ event: 'changed', turn: e.turnId, change: { kind: 'model_response', step: e.index, usage: chunk.usage ?? null } })
            responseReported = true
            observedUsage = chunk.usage ?? null
          }
          if (chunk.kind === 'text' || chunk.kind === 'thinking') {
            const key = chunk.kind + ':' + chunk.index
            let block = blocks.get(key)
            if (block === undefined) {
              block = item(JSON.stringify([e.turnId, e.index, chunk.index, chunk.kind]),
                chunk.kind === 'text' ? 'message' : 'thinking', { text: '' })
              blocks.set(key, block)
              reports.stream({ event: 'began', turn: e.turnId, item: { ...block } })
            }
            // Thinking whose words are withheld streams empty pieces that
            // carry only Claude Code's progress estimate.
            if (chunk.text !== '') {
              block.text += chunk.text
              reports.stream({ event: 'grew', turn: e.turnId, item: block.id, part: 'text', text: chunk.text })
            }
          }
          yield chunk
        }
        const result = await stream.result
        if (!responseReported || result.usage != null && ['model', 'input_tokens', 'output_tokens', 'cache_read_input_tokens', 'cache_creation_input_tokens'].some((key) => observedUsage?.[key] !== result.usage[key])) {
          reports.stream({ event: 'changed', turn: e.turnId, change: { kind: 'model_response', step: e.index, usage: result.usage ?? null } })
        }
        return result
      } catch (error) {
        // A failed attempt's blocks are never stored: no later row is theirs.
        step.failed = true
        throw error
      } finally {
        // A block whose row was never stored (an interruption, a failed
        // attempt) ends with the step, as exactly the pieces observed.
        // Claude Code 2.1.296 exposes no attempt withdrawal; a future typed
        // reset/retract event belongs here; opaque engine refs must never be
        // interpreted as such an event.
        for (const block of blocks.values()) {
          if (block.ended) continue
          block.ended = true
          reports.stream({ event: 'finished', turn: e.turnId, item: shownOf(block) })
        }
        // The step's blocks outlive a reload of the plugin.
        checkpoint(reports).catch(() => {})
      }
    },

    async call(reports, e, next) {
      const { tool, tool_use_id: id, consent, agentId, ...input } = e
      const call = { id, tool, input, turn: agentId === undefined ? turnNow() : agentTurn, started_at: reserve().at }
      tools.set(id, call)
      await checkpoint(reports)
      await reports.event('tool_start', { turn: call.turn, item: id, tool, input, started_at: call.started_at })
      const began = Date.now()
      try {
        const result = await next(e)
        call.result = result
        call.ms = Date.now() - began
        if (result.deny === undefined && result.isError !== true) {
          const changed = steps.update(tool, input, result.result)
          if (changed !== null) await emit(reports, { event: 'steps', turn: call.turn, steps: changed, note: null })
        }
        await checkpoint(reports)
        if (call.stored !== undefined) await completeTool(reports, call, call.stored)
        return result
      } catch (error) {
        call.thrown = String(error)
        throw error
      }
    },

    async append(reports, e, next) {
      const turn = e.agentId === undefined ? turnNow() : agentTurn
      const result = await next(e)
      if (result.deny !== undefined || rows.has(result.uuid)) return result
      rows.add(result.uuid)
      // Only the conversation's own messages are certain to reach the file;
      // the node waits for the last of them when it reads that file.
      if (result.message.type === 'user' || result.message.type === 'assistant') reports.stored(result.uuid)
      const told = e.agentId === undefined && e.origin?.kind === 'task-notification'
        ? notified(result.message.content.filter(block => block.type === 'text').map(block => block.text).join('\n'))
        : null
      if (told !== null && agents.has(told.agent)) {
        await emit(reports, { event: 'changed', turn: turn ?? null, change: { kind: 'agent_ended', agent_id: told.agent, end: told.end } })
      } else if (e.agentId === undefined && e.door === 'prompt' && result.message.type === 'user'
        && !result.message.content.some(block => block.type === 'tool_result')) {
        const prompt = result.message.content.filter(block => block.type === 'text').map(block => block.text).join('\n')
        const text = e.origin?.kind === 'plugin' ? asSaid(prompt) : prompt
        // Words the World handed over are told by its own record, which knows
        // who said them, as the node's reading of the transcript leaves them
        // out (`claude::is_framed`).
        const framed = WORLD_FRAMES.some(frame => text.startsWith(frame) || text.includes('\n' + frame + '\n'))
        if (!framed && !['<local-command-caveat>', '<local-command-stdout>', '<local-command-stderr>', '<command-name>',
          '<command-message>', '[Request interrupted by user'].some(prefix => text.trimStart().startsWith(prefix))) {
          await emit(reports, { event: 'heard', delivery: null, turn: turn ?? null, from: null, text, notice: null,
            message: { id: result.uuid, client_id: null, attachments: [], content: result.message.content,
              hook_fragments: [], presentation: null, detail: { uuid: result.uuid, message: result.message } } })
        }
      }
      const rowTurn = e.agentId === undefined ? currentTurn() : agentTurn
      const stepOf = result.message.type === 'assistant' ? owner(rowTurn) : undefined
      for (const [index, block] of result.message.content.entries()) {
        // Where in its step's response this block stands, when the step
        // named that far in its stream.
        const position = stepOf === undefined ? null : stepOf.next++
        const placed = position !== null && position <= stepOf.last
        if (block.type === 'tool_use') {
          requests.set(block.id, { block, turn, at: reserve().at })
          await checkpoint(reports)
        } else if (block.type === 'tool_result') {
          let call = tools.get(block.tool_use_id)
          const stored = { uuid: result.uuid, block }
          if (call === undefined) {
            const request = requests.get(block.tool_use_id)
            // Unknown tools, entry-time cancellation and some refusals have
            // stored results without ever dispatching tool.call. No executed
            // arguments or duration were reported for those calls.
            call = {
              id: block.tool_use_id,
              tool: request?.block.name ?? e.origin.tool,
              input: null,
              turn: request?.turn ?? turn,
              started_at: request?.at ?? reserve().at,
            }
            await reports.event('tool_start', {
              turn: call.turn, item: call.id, tool: call.tool, input: null, started_at: call.started_at,
            })
            await completeTool(reports, call, stored)
          } else if (call.result !== undefined || call.thrown !== undefined) {
            await completeTool(reports, call, stored)
          } else {
            call.stored = stored
            await checkpoint(reports)
          }
        } else if (result.message.type === 'assistant' && (block.type === 'text' || block.type === 'thinking' || block.type === 'redacted_thinking')) {
          const kind = block.type === 'text' ? 'message' : 'thinking'
          const text = block.type === 'text' ? block.text : block.type === 'thinking' ? block.thinking : ''
          // The row is that of the earliest block of its kind a recent step
          // streamed and no row has claimed: blocks of one response come one
          // after another, each stored as it closes, and a step's rows come
          // before the next step's. One already ended with its step (an
          // interruption's partial block, stored after the stream) is not
          // shown again. The plugin's idea of the turn, where it has one,
          // keeps an earlier turn's leftovers out.
          // The block its step streamed at this place, when it streamed one
          // there; a row past what the stream named is matched as before, by
          // the earliest block of its kind no row has claimed.
          let claimed
          if (placed) {
            const there = stepOf.blocks.get((kind === 'message' ? 'text' : 'thinking') + ':' + position)
            claimed = there !== undefined && !there.stored ? { step: stepOf, shown: there } : undefined
          } else {
            claimed = claim(kind, currentTurn())
          }
          const streamed = claimed?.shown
          let shown
          if (streamed !== undefined) {
            // Kept now under the name Claude Code's own record gives it, the
            // name a history read from that record shows it by: the draft it
            // streamed as is that same item, once, however it is read later.
            streamed.stored = true
            streamed.ended = true
            shown = { ...shownOf(streamed), id: JSON.stringify([result.uuid, index]), draft: streamed.id,
              ...(typeof text === 'string' && { text }), detail: block }
            reports.stream({ event: 'finished', turn: claimed.step.turn, item: shown })
          } else {
            // A block with nothing streamed (thinking whose display sends no
            // pieces, a non-streamed response) is first seen as it is stored,
            // with its own stable row identity.
            shown = item(JSON.stringify([result.uuid, index]), kind, { text: text ?? '', detail: block })
            await emit(reports, { event: 'began', turn: turn ?? '', item: shown })
            await emit(reports, { event: 'finished', turn: turn ?? '', item: shown })
          }
          if (kind === 'thinking') reports.thought({ uuid: result.uuid, turn: claimed?.step.turn ?? turn ?? '', item: shown.id })
        } else if (result.message.type === 'assistant') {
          const other = item(JSON.stringify([result.uuid, index]), 'other', {
            host_kind: block.type, detail: block,
          })
          await emit(reports, { event: 'began', turn: turn ?? '', item: other })
          await emit(reports, { event: 'finished', turn: turn ?? '', item: other })
        }
      }
      // What the step's ledger learned from this row (blocks claimed, how
      // far its positions went) outlives a reload of the plugin: a row that
      // arrives after it claims the next block, not this one again. It goes
      // in its turn among the session's writes and is not waited for: a row
      // stored while its step still streams would otherwise wait for the
      // stream's batch to be handed over.
      if (stepOf !== undefined) checkpoint(reports).catch(() => {})
      // Prose already streamed is not reported again as it is stored: the
      // record of what was stored is Claude Code's own session file, which
      // the World reads for a session's history.
      return result
    },

    async compact(reports, e, next) {
      if (e.agentId !== undefined) return next(e)
      const turn = turnNow() ?? ''
      const identity = reserve()
      const compacted = { id: 'compact:' + identity.id, kind: 'compaction', started_at: identity.at }
      if (e.trigger === 'precompute') {
        compacted.kind = 'other'
        compacted.host_kind = 'compaction_precompute'
      }
      await emit(reports, { event: 'began', turn, item: compacted })
      try {
        const result = await next(e)
        const finished = {
          ...compacted,
          detail: { trigger: e.trigger, instructions: e.instructions, ...result },
        }
        if (e.trigger === 'precompute' || result.skip !== undefined) {
          finished.kind = 'other'
          finished.host_kind = e.trigger === 'precompute' ? 'compaction_precompute' : 'compaction_skipped'
        }
        await emit(reports, { event: 'finished', turn, item: finished })
        return result
      } catch (error) {
        await emit(reports, { event: 'finished', turn, item: {
          ...compacted, kind: 'other', host_kind: 'compaction_failed',
          detail: { trigger: e.trigger, error: String(error) },
        } })
        throw error
      }
    },

    async measure(reports, e, next) {
      await emit(reports, { event: 'changed', turn: null, change: { kind: 'usage', usage: e } })
      if (e.context.tokens !== undefined) {
        await emit(reports, { event: 'context', used: e.context.tokens, window: e.context.window })
      }
      return next(e)
    },

    async end(reports, turn) {
      for (const call of tools.values()) {
        if (call.turn !== turn) continue
        if (call.stored !== undefined) await completeTool(reports, call, call.stored)
        else {
          await reports.event('tool_stored', {
            turn, item: call.id, tool: call.tool, input: call.input, started_at: call.started_at,
            content: null, failed: call.result?.isError === true || call.thrown !== undefined,
            declined: call.result?.deny !== undefined, cut: true, ms: observedMilliseconds(call.ms),
            detail: { returned: call.result, error: call.thrown, missing_stored_result: true },
          })
          tools.delete(call.id)
        }
      }
      requests.clear()
      await checkpoint(reports)
    },
  }
}

// How a task that went on in the background ended, in the words Claude Code
// writes the model when it does (`<task-notification>`), read as
// `subagents.rs` reads them: its figures under `<usage>`. Null for anything
// else.
function notified(text) {
  const body = text.trim()
  if (!body.startsWith('<task-notification>')) return null
  const tag = (name, within = body) => {
    const open = within.indexOf('<' + name + '>')
    if (open < 0) return null
    const start = open + name.length + 2
    const close = within.indexOf('</' + name + '>', start)
    return close < 0 ? null : within.slice(start, close).trim()
  }
  const number = (usage, name) => {
    const open = usage.indexOf('<' + name + '>')
    if (open < 0) return null
    const value = Number(usage.slice(open + name.length + 2).split('<')[0].trim())
    return Number.isSafeInteger(value) && value >= 0 ? value : null
  }
  const agent = tag('task-id')
  if (!agent) return null
  const usage = tag('usage') ?? ''
  const status = tag('status')
  const how = status === 'completed' ? 'completed' : status === 'failed' ? 'failed'
    : status === 'stopped' || status === 'killed' ? 'interrupted' : status ?? ''
  return {
    agent,
    end: {
      how,
      reason: how === 'failed' ? tag('summary') : null,
      duration_ms: observedMilliseconds(number(usage, 'duration_ms')),
      tokens: number(usage, 'subagent_tokens') ?? number(usage, 'total_tokens'),
      tools: number(usage, 'tool_uses'),
      answer: tag('result') || null,
      task: null,
      told: true,
    },
  }
}
