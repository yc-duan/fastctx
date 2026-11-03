// What the panel's specimens show: a goal in each of its states, and a
// conversation's colleagues with the host's agents they and it sent.

import type { CollabNode, GoalShown, PastGoal } from "./types";

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
// A goal that is never done, under way, and paused when it twice left
// nothing to wake it.
const watch = english ? "Every night, look over CI and fix it when it is red." : "每晚看一遍 CI，红了就修。";
goals.standing = { objective: watch, version: 1, status: "active", worked: 26 * minute, since: now - 26 * minute, standing: true };
goals.unwoken = { objective: watch, version: 2, status: "paused", worked: 41 * minute, since: now - 5 * minute, standing: true, paused: { by: "unwoken", until: null } };
for (const goal of Object.values(goals)) if (goal) goal.id ??= "goal-sample-now";
  return goals;
}

/** The goals the specimen conversation had before: one done that sent a
 *  colleague, one put down, and one that stood until it was cleared. */
export function pastSamples(english: boolean, now: number): PastGoal[] {
  const hour = 3_600_000;
  const day = 24 * hour;
  return [
    {
      id: "goal-sample-astro",
      objective: english ? "Move the blog from Hugo to Astro, with every old link still opening." : "把博客从 Hugo 迁到 Astro，旧的链接都还能打开。",
      done: true,
      standing: false,
      created: now - 3 * day,
      closed: now - 2 * day - 5 * hour,
      statement: english
        ? "- **Site**: builds with Astro on lab (`j-a7s2q1`).\n- **Links**: all 412 old addresses open, checked by `scripts/links.ts`."
        : "- **站点**：在 lab 上用 Astro 编出来了（`j-a7s2q1`）。\n- **链接**：412 个旧地址都能打开，`scripts/links.ts` 一条条查过。",
    },
    {
      id: "goal-sample-lazy",
      objective: english ? "Lazy-load the pictures in posts." : "给文章里的图片加懒加载。",
      done: false,
      standing: false,
      created: now - 2 * day,
      closed: now - day - 20 * hour,
      statement: null,
    },
    {
      id: "goal-sample-404",
      objective: english ? "Every morning, look over the site's 404s and fix what can be fixed." : "每天早上看一遍站点的 404，修掉能修的。",
      done: false,
      standing: true,
      created: now - day - 18 * hour,
      closed: now - 3 * hour,
      statement: null,
    },
  ];
}

export function teamSample(english: boolean): CollabNode[] {
  const minute = 60_000;
  const now = Date.now();
  const ago = (minutes: number) => now - minutes * minute;
  return [
    {
      id: "codex-b10g",
      kind: "colleague",
      agent: "codex",
      name: "GPT-6.1 Sol",
      task: english ? "The comments back end, with its tests" : "评论的后端，连同它的测试",
      state: "working",
      goal: "active",
      branchOf: "goal-sample-now",
      took: 62 * minute,
      began: ago(62),
      ended: null,
      steps: 48,
      machine: "lab",
      children: [
        { id: "hopper", kind: "subagent", agent: "codex", name: "Hopper", task: english ? "Read how posts are stored" : "读文章是怎样存的", state: "done", took: 4 * minute, began: ago(58), ended: ago(54), tokens: 41_200, model: "gpt-6.1-sol", steps: 17, children: [] },
        { id: "turing", kind: "subagent", agent: "codex", name: "Turing", task: english ? "Load the comments of every post at once" : "一次读出全部文章的评论", state: "failed", took: 6 * minute, began: ago(31), ended: ago(25), tokens: 18_400, model: "gpt-6.1-sol", steps: 9, why: english ? "stream disconnected before completion: model overloaded" : "stream disconnected before completion: model overloaded", children: [] },
        { id: "lovelace", kind: "subagent", agent: "codex", name: "Lovelace", task: english ? "Try the migration on a copy" : "在副本上试迁移", state: "working", took: 9 * minute, began: ago(9), ended: null, doing: english ? "Run cargo test -p migrate" : "运行 cargo test -p migrate", model: "gpt-6.1-sol", steps: 12, children: [] },
      ],
    },
    {
      id: "codex-d3pl",
      kind: "colleague",
      agent: "codex",
      name: "GPT-6 Luna",
      task: english ? "Check the migration before it goes live" : "上线前核对迁移",
      state: "waiting",
      goal: "active",
      branchOf: "goal-sample-now",
      took: 12 * minute,
      began: ago(12),
      ended: null,
      steps: 14,
      machine: "laptop",
      children: [],
    },
    {
      id: "codex-p4ge",
      kind: "colleague",
      agent: "codex",
      name: "GPT-6.1 Sol",
      task: english ? "Page the comments" : "评论分页",
      state: "stopped",
      goal: "paused",
      branchOf: "goal-sample-now",
      took: 5 * minute,
      began: ago(46),
      ended: ago(41),
      steps: 6,
      machine: "lab",
      children: [],
    },
    {
      id: "codex-f0rm",
      kind: "colleague",
      agent: "codex",
      name: "GPT-6.1 Luna",
      task: english ? "The form under each post" : "每篇文章下面的表单",
      state: "done",
      goal: "complete",
      branchOf: "goal-sample-now",
      took: 38 * minute,
      began: ago(70),
      ended: ago(32),
      steps: 31,
      machine: "laptop",
      children: [],
    },
    {
      id: "codex-l1nk",
      kind: "colleague",
      agent: "codex",
      name: "GPT-6.1 Luna",
      task: english ? "Open every old link" : "旧链接一条条打开看",
      state: "done",
      goal: "complete",
      branchOf: "goal-sample-astro",
      took: 21 * minute,
      began: ago(185),
      ended: ago(164),
      steps: 26,
      machine: "lab",
      children: [],
    },
    { id: "explore-1", kind: "subagent", agent: "claude", name: "Explore", task: english ? "Where the blog renders a post" : "博客在哪里渲染一篇文章", state: "done", took: 2 * minute, began: ago(16), ended: ago(14), steps: 9, children: [] },
    // `?team=many`: a colleague that fanned its work out wide, to see the
    // rows scroll under a clock that stays.
    ...(new URLSearchParams(location.search).get("team") === "many"
      ? [
          {
            id: "codex-w1de",
            kind: "colleague" as const,
            agent: "codex" as const,
            name: "GPT-6.1 Sol",
            task: english ? "Read every module for the old comment calls" : "逐个模块找旧评论接口的调用",
            state: "working" as const,
            goal: "active" as const,
            took: 40 * minute,
            began: ago(40),
            ended: null,
            steps: 120,
            machine: "lab",
            children: Array.from({ length: 48 }, (_, index) => {
              const begun = 38 - (index % 12) * 3;
              const ended = index % 5 === 0 ? null : begun - 1 - (index % 4);
              return {
                id: `wide-${index}`,
                kind: "subagent" as const,
                agent: "codex" as const,
                name: `Scout ${index + 1}`,
                task: english ? `Module ${index + 1}: where it calls the comments` : `第 ${index + 1} 个模块：哪里调了评论`,
                state: (ended === null ? "working" : index % 7 === 3 ? "failed" : "done") as "working" | "failed" | "done",
                took: 0,
                began: ago(begun),
                ended: ended === null ? null : ago(ended),
                ...(ended === null ? { doing: english ? `Read src/module_${index + 1}.rs` : `读 src/module_${index + 1}.rs` } : {}),
                steps: 4 + (index % 9),
                children: [],
              };
            }),
          },
        ]
      : []),
    { id: "general-2", kind: "subagent", agent: "claude", name: "general-purpose", task: english ? "Why the old comments plugin was dropped" : "旧的评论插件为什么被拿掉", state: "failed", took: 6 * minute, began: ago(22), ended: ago(16), steps: 22, children: [] },
  ];
}
