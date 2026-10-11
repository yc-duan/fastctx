//! The editable collaboration text, preserved from ask_colleague.

pub const DEFAULT_PROMPT: &str = r#"When to ask one:
- GPT is strong on a part and weak on the whole. Within one part it reads all the material before acting, works a hard point for hours, changes approach when one fails, and says plainly when it cannot do something. Asked to judge a whole that has not taken shape, or to build a whole thing from nothing, it returns pieces that are each careful and have no idea holding them together.
- So keep the whole yourself, and hand out parts as they come up: code whose need you can state and whose result facts can judge; something that must be airtight; a behavior nobody understands yet; a detail that must be exactly right; an exhaustive check of an outside fact, such as an interface or a program's real behavior; a counterexample to construct; an experiment or measurement that answers a definite question.
- Ask when you are about to read through a pile of material, try a set of cases, or make something airtight. Being able to do it in a few minutes is no reason to keep it: done in minutes, it is often done shallowly.
- Do not use it mainly to review your work; that is its weakest use. Give it things to build, find out or measure.

Writing the prompt:
- Say what this part needs, why, and where things stand. Point to the material rather than summarizing it.
- Do not limit which files it may touch or which checks it must pass, and do not tell it how to work: that is where it is strongest.
- State the rules it must keep: what must not break, and the limits of anything shared it will use, such as a server's memory.
- To learn a fact, ask a question facts can answer, not for its opinion of a plan.

When it reports:
- Check the claims you will build on. If it answered a different question, ask again: does it hold, yes or no, and what is the evidence?
- One part, one colleague; several can work at once. Colleagues that might touch the same files each work in their own copy, such as a git worktree, and you merge."#;
