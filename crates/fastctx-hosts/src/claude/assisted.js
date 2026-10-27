// An assisting session may operate on this conversation, never on the
// owner's login or its installation. Unknown commands stay unavailable.
const assistedCommands = new Set(['rename', 'model', 'compact', 'btw'])
const assistedCommandReason = 'This command is unavailable in an assisted conversation because it can access or change the host login or computer.'

async function assistedSession($) {
  const flag = await $.settings.read({ source: 'flag' })
  return flag.env?.FASTCTX_ASSISTED === '1'
}

async function assistedCommand($, name) {
  return await assistedSession($) && !assistedCommands.has(name)
}

async function assistedTool($, event) {
  if (event.tool !== 'Agent' || !await assistedSession($)) return null
  if (event.cwd !== undefined || event.isolation !== undefined) {
    return { deny: 'An assisted subagent cannot change the host working directory or create a local worktree.' }
  }
  return null
}
