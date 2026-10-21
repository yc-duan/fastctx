// Definitions belong to the conversation's account, never to a login directory.
async function refreshAccountAgents($, context) {
  const result = await $.process.run([FASTCTX, 'claude-subagents', WORLD, context.sessionId])
  if (result.exitCode !== 0) throw new Error(result.stderr.trim() || 'the account subagents could not be read')
  const configuration = JSON.parse(result.stdout)
  const revision = JSON.stringify(configuration)
  const active = new Map()
  context.registeredAgents ??= new Set()
  for (const spec of configuration?.agents ?? []) {
    const encoded = JSON.stringify(spec)
    const previous = context.accountAgents?.get(spec.name)
    let full = previous?.full
    if (previous?.encoded !== encoded) full = (await $.agent.register(spec)).agent
    active.set(spec.name, { full, encoded })
    context.registeredAgents.add(full)
  }
  context.accountAgents = active
  context.subagentInstructions = configuration?.instructions ?? ''
  context.accountAgentConfiguration = configuration
  if (context.accountAgentRevision !== revision) await $.ui.invalidate('prompt.context')
  context.accountAgentRevision = revision
}

function offerAccountAgent(context, event) {
  if (event.source === 'built-in' && event.provider.plugin === 'engine' && event.provider.tier === 'core'
      && context.accountAgents?.has(event.agent)) return false
  if (context.registeredAgents?.has(event.agent)) {
    return [...(context.accountAgents?.values() ?? [])].some((entry) => entry.full === event.agent)
  }
  return null
}

// A plugin's own submit skips its prompt hooks; append a model-only note first.
async function accountAgentsBeforeSubmit($, context) {
  await refreshAccountAgents($, context)
  if (context.heardAccountAgents === context.accountAgentRevision) return
  if (context.accountAgentConfiguration === null && context.heardAccountAgents === undefined) return
  const rows = [...context.accountAgents.entries()].map(([name, entry]) => {
    const spec = JSON.parse(entry.encoded)
    return '- ' + entry.full + ': ' + spec.description
  })
  const listing = rows.length ? rows.join('\n') : '(none)'
  const text = "The account's Claude subagents now listed below replace its previous subagent list:\n" + listing
    + '\n\n' + (context.subagentInstructions || "The account's subagent instructions are empty.")
  const result = await $.session.append({ message: { type: 'user', content: [{ type: 'text', text }] } })
  if (result.deny !== undefined) throw new Error(result.deny)
  context.heardAccountAgents = context.accountAgentRevision
}
