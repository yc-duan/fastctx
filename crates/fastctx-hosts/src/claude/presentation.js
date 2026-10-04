import { taskSteps } from './steps.js'

// The session's observable presentation. Transport and inbox delivery live
// in world.js; this module never submits a prompt or changes a host result.
export function presentation(event, reserve, currentTurn, save) {
  const tools = new Map()
  const requests = new Map()
  const rows = new Set()
  let latestStep = null
  const steps = taskSteps()
  const emit = ($, happened) => event($, 'session', { happened })
  const item = (id, kind, fields = {}) => ({ id, kind, started_at: reserve().at, ...fields })
  const checkpoint = ($) => save($, { tools: [...tools.entries()], requests: [...requests.entries()], tasks: steps.snapshot() })

  async function completeTool($, call, stored) {
    const result = call.result
    const { content, ...header } = stored.block
    await event($, 'tool_stored', {
      turn: call.turn,
      item: call.id,
      tool: call.tool,
      input: call.input,
      started_at: call.started_at,
      content: content ?? null,
      failed: stored.block.is_error === true,
      declined: result?.deny !== undefined,
      ms: call.ms,
      detail: { returned: result, stored: { uuid: stored.uuid, block: header }, requested: requests.get(call.id)?.block },
    })
    tools.delete(call.id)
    requests.delete(call.id)
    await checkpoint($)
  }

  return {
    restore(state) {
      for (const [id, call] of state.tools) tools.set(id, call)
      for (const [id, request] of state.requests) requests.set(id, request)
      steps.restore(state.tasks)
    },
    async *step($, e, next) {
      const stream = next(e)
      if (e.agentId !== undefined) {
        for await (const chunk of stream) yield chunk
        return await stream.result
      }
      const blocks = new Map()
      latestStep = { turn: e.turnId, blocks }
      try {
        for await (const chunk of stream) {
          if (chunk.kind === 'text' || chunk.kind === 'thinking') {
            const key = chunk.kind + ':' + chunk.index
            let block = blocks.get(key)
            if (block === undefined) {
              block = item(JSON.stringify([e.turnId, e.index, chunk.index, chunk.kind]),
                chunk.kind === 'text' ? 'message' : 'thinking', { text: '' })
              blocks.set(key, block)
              await emit($, { event: 'began', turn: e.turnId, item: { ...block } })
            }
            block.text += chunk.text
            await emit($, { event: 'grew', turn: e.turnId, item: block.id, part: 'text', text: chunk.text })
          }
          yield chunk
        }
        return await stream.result
      } finally {
        // Claude Code 2.1.289 exposes neither attempt withdrawal nor a
        // binding from stream blocks to stored row UUIDs (2026-10-04).
        // Keep exactly the pieces observed for this turn/step/block, also
        // on interruption. A future typed reset/retract event belongs here;
        // opaque engine refs must never be interpreted as such an event.
        for (const block of blocks.values()) {
          await emit($, { event: 'finished', turn: e.turnId, item: block })
        }
      }
    },

    async call($, e, next) {
      if (e.agentId !== undefined) return next(e)
      const { tool, tool_use_id: id, consent, agentId, ...input } = e
      const call = { id, tool, input, turn: currentTurn(), started_at: reserve().at }
      tools.set(id, call)
      await checkpoint($)
      await event($, 'tool_start', { turn: call.turn, item: id, tool, input, started_at: call.started_at })
      const began = Date.now()
      try {
        const result = await next(e)
        call.result = result
        call.ms = Date.now() - began
        if (result.deny === undefined && result.isError !== true) {
          const changed = steps.update(tool, input, result.result)
          if (changed !== null) await emit($, { event: 'steps', turn: call.turn, steps: changed, note: null })
        }
        await checkpoint($)
        if (call.stored !== undefined) await completeTool($, call, call.stored)
        return result
      } catch (error) {
        call.thrown = String(error)
        throw error
      }
    },

    async append($, e, next) {
      const turn = currentTurn()
      const result = await next(e)
      if (e.agentId !== undefined || result.deny !== undefined || rows.has(result.uuid)) return result
      rows.add(result.uuid)
      const streamedKinds = new Set()
      if (latestStep?.turn === turn) {
        for (const block of latestStep.blocks.values()) streamedKinds.add(block.kind)
      }
      let storedProse = false
      for (const [index, block] of result.message.content.entries()) {
        if (block.type === 'tool_use') {
          requests.set(block.id, { block, turn, at: reserve().at })
          await checkpoint($)
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
            await event($, 'tool_start', {
              turn: call.turn, item: call.id, tool: call.tool, input: null, started_at: call.started_at,
            })
            await completeTool($, call, stored)
          } else if (call.result !== undefined || call.thrown !== undefined) {
            await completeTool($, call, stored)
          } else {
            call.stored = stored
            await checkpoint($)
          }
        } else if (result.message.type === 'assistant' && (block.type === 'text' || block.type === 'thinking')) {
          const kind = block.type === 'text' ? 'message' : 'thinking'
          if (!streamedKinds.has(kind)) {
            // The host can return a non-streamed assistant row: its stream
            // projects only an opaque engine ref. This stored block is the
            // first observable prose, with its own stable row identity.
            const prose = item(JSON.stringify([result.uuid, index]), kind, {
              text: block.type === 'text' ? block.text : block.thinking, detail: block,
            })
            await emit($, { event: 'began', turn: turn ?? '', item: prose })
            await emit($, { event: 'finished', turn: turn ?? '', item: prose })
          } else storedProse = true
        } else if (result.message.type === 'assistant') {
          const other = item(JSON.stringify([result.uuid, index]), 'other', {
            host_kind: block.type, detail: block,
          })
          await emit($, { event: 'began', turn: turn ?? '', item: other })
          await emit($, { event: 'finished', turn: turn ?? '', item: other })
        }
      }
      if (storedProse) {
        // A stored row cannot be bound to the streamed blocks. Retain its
        // original content separately, without replacing or duplicating the
        // streamed prose items. This also preserves a non-streamed fallback
        // after a partially streamed attempt, and signed thinking metadata.
        const record = item('stored:' + result.uuid, 'other', {
          host_kind: 'claude_response_record',
          detail: { uuid: result.uuid, message: result.message, door: e.door, origin: e.origin },
        })
        await emit($, { event: 'began', turn: turn ?? '', item: record })
        await emit($, { event: 'finished', turn: turn ?? '', item: record })
      }
      return result
    },

    async compact($, e, next) {
      if (e.agentId !== undefined) return next(e)
      const turn = currentTurn() ?? ''
      const identity = reserve()
      const compacted = { id: 'compact:' + identity.id, kind: 'compaction', started_at: identity.at }
      if (e.trigger === 'precompute') {
        compacted.kind = 'other'
        compacted.host_kind = 'compaction_precompute'
      }
      await emit($, { event: 'began', turn, item: compacted })
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
        await emit($, { event: 'finished', turn, item: finished })
        return result
      } catch (error) {
        await emit($, { event: 'finished', turn, item: {
          ...compacted, kind: 'other', host_kind: 'compaction_failed',
          detail: { trigger: e.trigger, error: String(error) },
        } })
        throw error
      }
    },

    async measure($, e, next) {
      if (e.context.tokens !== undefined) {
        await emit($, { event: 'context', used: e.context.tokens, window: e.context.window })
      }
      return next(e)
    },

    async end($, turn) {
      for (const call of tools.values()) {
        if (call.turn !== turn) continue
        if (call.stored !== undefined) await completeTool($, call, call.stored)
        else {
          await event($, 'tool_stored', {
            turn, item: call.id, tool: call.tool, input: call.input, started_at: call.started_at,
            content: null, failed: call.result?.isError === true || call.thrown !== undefined,
            declined: call.result?.deny !== undefined, cut: true, ms: call.ms,
            detail: { returned: call.result, error: call.thrown, missing_stored_result: true },
          })
          tools.delete(call.id)
        }
      }
      requests.clear()
      await checkpoint($)
    },
  }
}
