// The conversation's account's Claude subagents (`world-user-config.md`),
// read from the node and registered under the plugin's name, never from a
// login's own directory. A built-in type of the same name gives way to the
// account's. A read or a registration that fails keeps what was registered
// before and says so in Claude Code's debug log: the session goes on.
async function refreshAccountAgents($, context) {
  try {
    const result = await $.process.run([FASTCTX, 'claude-subagents', WORLD, context.sessionId])
    if (result.exitCode !== 0) throw new Error(result.stderr.replace(/^fastctx:\s*/, '').trim() || 'the node did not answer')
    const configuration = JSON.parse(result.stdout)
    const revision = JSON.stringify(configuration)
    if (context.accountAgentRevision === revision) return
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
    context.accountAgentRevision = revision
    await $.ui.invalidate('prompt.context')
  } catch (error) {
    worldFailure($, 'account subagents', error)
  }
}

// Whether `event`'s agent type is offered: a built-in one the account has its
// own of is not, nor one of the account's taken out since it was registered.
function offerAccountAgent(context, event) {
  if (event.source === 'built-in' && event.provider.plugin === 'engine' && event.provider.tier === 'core'
      && context.accountAgents?.has(event.agent)) return false
  if (context.registeredAgents?.has(event.agent)) {
    return [...(context.accountAgents?.values() ?? [])].some((entry) => entry.full === event.agent)
  }
  return null
}

// What the model last heard of the account's subagents, kept beside the
// session: a resumed process's first message still says what an earlier one
// was told, and only a change since is told again. A compaction's first
// message carries the revision of the read it was built from.
async function agentsHeard($, context, revision = context.accountAgentRevision) {
  context.heardAccountAgents = revision
  try {
    await $.fs.write(context.dir + '/agents-heard.json', JSON.stringify({ revision: revision ?? null }))
  } catch (error) {
    worldFailure($, 'account subagents', error)
  }
}

// At a process's start: what an earlier process of this session told the
// model. Without a receipt, no revision is known to have reached it.
async function agentsHeardBefore($, context) {
  try {
    context.heardAccountAgents = JSON.parse(await $.fs.read(context.dir + '/agents-heard.json')).revision ?? undefined
  } catch (error) {
    context.heardAccountAgents = undefined
  }
}

// A plugin's own submit skips its prompt hooks, and Claude Code lists a type
// again only when one comes or goes: a change since the model last heard is
// told in a note ahead of the World's words.
async function accountAgentsBeforeSubmit($, context) {
  await refreshAccountAgents($, context)
  if (context.contextFresh === true || context.accountAgentRevision === undefined || context.heardAccountAgents === context.accountAgentRevision) return
  const rows = [...(context.accountAgents?.values() ?? [])].map((entry) => '- ' + entry.full + ': ' + JSON.parse(entry.encoded).description)
  const listing = rows.length ? rows.join('\n') : '(none)'
  const text = "The account's Claude subagents now listed below replace its previous subagent list:\n" + listing
    + '\n\n' + (context.subagentInstructions || "The account's subagent instructions are empty.")
  try {
    const result = await $.session.append({ message: { type: 'user', content: [{ type: 'text', text }] } })
    if (result.deny !== undefined) throw new Error(result.deny)
    await agentsHeard($, context)
  } catch (error) {
    worldFailure($, 'account subagents', error)
  }
}
