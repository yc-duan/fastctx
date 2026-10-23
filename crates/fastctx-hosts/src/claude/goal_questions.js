// Goal questions use the existing asynchronous message card and user-reply envelope.
const GOAL_QUESTION_REPLY = 'The question has been sent to the owner of this goal. Continue with the best-supported judgment; their answer will arrive separately.'
const GOAL_QUESTION_FAILED = 'The World could not publish this question'

async function goalQuestion($, context, input, next) {
  if (input.tool !== 'AskUserQuestion') return next(input)
  const turn = context.turn
  await readGoal($, context)
  if (context.goal?.active !== true || context.goalStopping === true) return next(input)
  const questions = input.questions
  if (!Array.isArray(questions) || questions.length === 0) return next(input)
  const item = {
    id: JSON.stringify(['fastctx_goal_question', context.sessionId, input.tool_use_id]),
    kind: 'message',
    text: questions.map((question) => question.question).join('\n'),
    detail: {
      delivery: 'async',
      questions: questions.map((question) => ({
        title: question.question,
        options: Array.isArray(question.options) ? question.options.map((option) => option.label) : null,
        multiSelect: question.multiSelect === true,
      })),
      fastctx_goal_question: { tool_use_id: input.tool_use_id, agent_id: input.agentId ?? null, questions },
    },
  }
  try {
    await event($, context, 'session', { happened: { event: 'finished', turn: turn ?? '', item } })
  } catch (error) {
    worldFailure($, 'goal question', error)
    return { deny: GOAL_QUESTION_FAILED + ': ' + String(error) }
  }
  return { result: { questions, answers: {} }, context: [GOAL_QUESTION_REPLY] }
}
