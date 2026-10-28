// Model collaboration as the specimen's account has it: Claude on this
// computer may hand work to two GPTs, one on this computer and one on the
// laptop.

import type { CollabView } from "../world/collab";

export function specimenCollab(): CollabView {
  return {
    collab: {
      version: 3,
      on: true,
      callers: [{ machine: "desktop", agent: "claude", home: null }],
      callees: [
        {
          name: "sol",
          use_when: "Long refactors and anything that needs careful reading of a large codebase.",
          login: { machine: "desktop", agent: "codex", home: null },
          model: "gpt-6.1-sol",
          effort: "high",
          screen: false,
        },
        {
          name: "luna",
          use_when: "Quick checks, small scripts, running the test suite and reading its failures.",
          login: { machine: "laptop", agent: "codex", home: null },
          model: "gpt-6-luna",
          effort: "low",
          screen: true,
        },
      ],
      prompt: null,
      rules: "Work in the branch you are given. Say what you changed and how you checked it.",
    },
    default_prompt: "You can hand a well-defined part of your work to a colleague: another model, working in the same World.",
  };
}
