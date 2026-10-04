// Task tools report different projections of the host's task store. Keep
// fields a later projection omits, and apply updates only when confirmed.
export function taskSteps() {
  const tasks = new Map()
  const status = (value) => ({ pending: 'pending', in_progress: 'doing', completed: 'done' })[value]
  const step = (task) => {
    const state = status(task.status)
    if (state === undefined) throw new Error('unknown task status: ' + task.status)
    return {
      text: task.content ?? task.subject,
      status: state,
      id: task.id,
      active_form: task.activeForm,
      detail: task,
    }
  }
  function update(name, input, result) {
    if (name === 'TodoWrite') return result.newTodos.map(step)
    if (name === 'TaskCreate') {
      const task = result.task
      tasks.set(task.id, { ...input, ...task, status: 'pending', blocks: [], blockedBy: [] })
    } else if (name === 'TaskList') {
      const present = new Set(result.tasks.map((task) => task.id))
      for (const id of tasks.keys()) if (!present.has(id)) tasks.delete(id)
      for (const task of result.tasks) {
        const known = { ...tasks.get(task.id), ...task }
        if (task.owner === undefined) delete known.owner
        tasks.set(task.id, known)
      }
    } else if (name === 'TaskGet') {
      if (result.task === null) return null
      tasks.set(result.task.id, { ...tasks.get(result.task.id), ...result.task })
    } else if (name === 'TaskUpdate') {
      if (result.success !== true) return null
      if (result.statusChange?.to === 'deleted') tasks.delete(result.taskId)
      else {
        const task = tasks.get(result.taskId)
        // A plugin enabled mid-session may not have seen this task yet.
        // Its tool record still retains this update; TaskList/Get supplies
        // the missing task identity and state without inventing them.
        if (!task) return null
        for (const field of result.updatedFields) {
          if (field === 'metadata') {
            task.metadata = { ...task.metadata, ...input.metadata }
            for (const [key, value] of Object.entries(task.metadata)) if (value === null) delete task.metadata[key]
          } else if (field === 'blocks' || field === 'blockedBy') {
            const added = input[field === 'blocks' ? 'addBlocks' : 'addBlockedBy']
            if (added !== undefined) task[field] = [...new Set([...(task[field] ?? []), ...added])]
          } else if (input[field] !== undefined) task[field] = input[field]
        }
        if (result.statusChange) task.status = result.statusChange.to
      }
    } else return null
    return [...tasks.values()].map(step)
  }
  return {
    update,
    snapshot: () => [...tasks.entries()],
    restore: (entries) => { for (const [id, task] of entries) tasks.set(id, task) },
  }
}
