// What the panel's specimens show: a goal in each of its states, and a
// conversation's colleagues with the host's agents they and it sent.

import type { CollabNode, GoalShown } from "./types";

export function goalSamples(english: boolean, now: number): Record<string, GoalShown | null> {
  const minute = 60_000;

  const objective = english
  ? "Make the blog's comments work end to end: the back end, the form under each post, and a test that posts one and reads it back."
  : "让博客的评论从头到尾能用：后端、每篇文章下面的表单，以及一个发一条再读回来的测试。";

  const goals: Record<string, GoalShown | null> = {
  none: null,
  active: { objective, version: 2, status: "active", worked: 74 * minute, since: now - 60 * minute, sentinel: null },
  paused: { objective, version: 2, status: "paused", worked: 134 * minute, since: now - 8 * minute, paused: { by: "user", until: null } },
  quota: {
    objective,
    version: 2,
    status: "paused",
    worked: 134 * minute,
    since: now - 20 * minute,
    paused: { by: "quota", until: now + 95 * minute },
  },
  blocked: {
    objective,
    version: 2,
    status: "blocked",
    worked: 211 * minute,
    since: now - 30 * minute,
    statement: english
      ? "Posting needs the database the comments live in. `psql` on **lab** says the role `blog` does not exist, and nothing on the machines holds its password.\n\nWhat would clear it: the role made, or its password put in `/srv/blog/.env`."
      : "发评论要用评论所在的数据库。**lab** 上 `psql` 说角色 `blog` 不存在，各台机器上都找不到它的密码。\n\n怎样能过去：建好这个角色，或者把它的密码放进 `/srv/blog/.env`。",
    sentinel: {
      at: now - 26 * minute,
      sound: true,
      reason: english
        ? "Saw the same error on lab myself; no .env on any machine holds the role's password."
        : "我在 lab 上亲眼看到了同样的报错；哪台机器的 .env 里都没有这个角色的密码。",
    },
  },
  complete: {
    objective,
    version: 3,
    status: "complete",
    worked: 247 * minute,
    since: now - 12 * minute,
    statement: english
      ? "- **Back end**: `POST /api/comments` and `GET /api/comments?post=` in `server/comments.ts`.\n- **Form**: under every post, in `web/Comments.tsx`.\n- **Test**: `tests/comments.spec.ts` posts one and reads it back; it passed on lab (`j-t4k2m9`)."
      : "- **后端**：`server/comments.ts` 里的 `POST /api/comments` 与 `GET /api/comments?post=`。\n- **表单**：每篇文章下面，在 `web/Comments.tsx`。\n- **测试**：`tests/comments.spec.ts` 发一条再读回来，在 lab 上跑过了（`j-t4k2m9`）。",
  },
};
// The sentinel's advice on a stop it did not find sound.
goals.advised = {
  ...goals.active!,
  sentinel: {
    at: now - 3 * minute,
    sound: false,
    looked: english ? "web/Comments.tsx and tests/ on lab" : "lab 上的 web/Comments.tsx 与 tests/",
    saw: english ? "The form is there; tests/ has no comments test yet." : "表单在了；tests/ 里还没有评论的测试。",
    thinks: english ? "The test that posts one and reads it back may still be missing." : "那个发一条再读回来的测试可能还没写。",
  },
};
  return goals;
}

export function teamSample(english: boolean): CollabNode[] {
  const minute = 60_000;
  return [
  {
    id: "codex-b10g",
    kind: "colleague",
    agent: "codex",
    name: "GPT-6.1 Sol",
    task: english ? "The comments back end, with its tests" : "评论的后端，连同它的测试",
    state: "working",
    goal: "active",
    took: 62 * minute,
    steps: 48,
    machine: "lab",
    children: [
      { id: "hopper", kind: "subagent", agent: "codex", name: "Hopper", task: english ? "Read how posts are stored" : "读文章是怎样存的", state: "done", took: 4 * minute, steps: 17, children: [] },
      { id: "lovelace", kind: "subagent", agent: "codex", name: "Lovelace", task: english ? "Try the migration on a copy" : "在副本上试迁移", state: "working", took: 9 * minute, steps: 12, children: [] },
    ],
  },
  {
    id: "codex-f0rm",
    kind: "colleague",
    agent: "codex",
    name: "GPT-6.1 Luna",
    task: english ? "The form under each post" : "每篇文章下面的表单",
    state: "done",
    goal: "complete",
    took: 38 * minute,
    steps: 31,
    machine: "laptop",
    children: [],
  },
  { id: "explore-1", kind: "subagent", agent: "claude", name: "Explore", task: english ? "Where the blog renders a post" : "博客在哪里渲染一篇文章", state: "done", took: 2 * minute, steps: 9, children: [] },
  { id: "general-2", kind: "subagent", agent: "claude", name: "general-purpose", task: english ? "Why the old comments plugin was dropped" : "旧的评论插件为什么被拿掉", state: "failed", took: 6 * minute, steps: 22, children: [] },
];
}
