// What a plain browser shows on 本机数据: a computer with a few months of
// conversations, some removed, a few kept here for conversations that live
// on other machines.

import type { ConversationSpace, DataCall, DataReply, Space } from "../world/data";

const MB = 1024 * 1024;
let cacheBytes = 240 * MB;
const hoursAgo = (hours: number) => new Date(Date.now() - hours * 3_600_000).toISOString();

const conversations: ConversationSpace[] = [
  {
    name: "codex-7f3a",
    group: "listed",
    agent: "codex",
    title: "定点化 Q16 的控制环",
    town: "mark50",
    last_activity: hoursAgo(2),
    bytes: 412 * MB,
    parts: { records: 96 * MB, command_output: 288 * MB, host: { kept: "kept", bytes: 28 * MB } },
  },
  {
    name: "claude-2b1c",
    group: "listed",
    agent: "claude",
    title: "整理实验室服务器的备份脚本",
    last_activity: hoursAgo(26),
    bytes: 58 * MB,
    parts: { records: 21 * MB, command_output: 25 * MB, host: { kept: "kept", bytes: 12 * MB, days: 30 } },
  },
  {
    name: "codex-a51c",
    group: "listed",
    agent: "codex",
    title: "把 CI 的缓存挪到 E 盘",
    last_activity: hoursAgo(0.3),
    bytes: 9.4 * MB,
    parts: { records: 3.1 * MB, command_output: 4.6 * MB, host: { kept: "kept", bytes: 1.7 * MB } },
    held: "它还在工作；先停下它",
  },
  {
    name: "claude-91de",
    group: "removed",
    agent: "claude",
    title: "读一遍电机驱动的数据手册",
    town: "mark50",
    last_activity: hoursAgo(24 * 19),
    bytes: 133 * MB,
    parts: { records: 128 * MB, command_output: 5 * MB, host: { kept: "gone_by_claude", days: 30 } },
  },
  {
    name: "codex-0c44",
    group: "removed",
    agent: "codex",
    last_activity: hoursAgo(24 * 41),
    bytes: 3.2 * MB,
    parts: { records: 2.2 * MB, command_output: 0, host: { kept: "missing" } },
  },
  {
    name: "codex-5e2a",
    group: "elsewhere",
    agent: "codex",
    title: "编译 Linux 的服务包",
    lives_on: "lab",
    last_activity: hoursAgo(5),
    bytes: 71 * MB,
    parts: { records: 6 * MB, command_output: 65 * MB, host: { kept: "none" } },
  },
];

let outputDays = 7;

function space(): Space {
  const listed = conversations.reduce((sum, one) => sum + one.parts.records, 0);
  const output = conversations.reduce((sum, one) => sum + one.parts.command_output, 0);
  const hosts = conversations.reduce((sum, one) => sum + (one.parts.host.kept === "kept" ? one.parts.host.bytes : 0), 0);
  return {
    machine: "desktop",
    at: new Date().toISOString(),
    root: "C:\\Users\\me\\.fastctx",
    root_bytes: listed + output + cacheBytes + 96 * MB,
    kinds: { conversations: listed, host_records: hosts, command_output: output, cache: cacheBytes, program: 96 * MB, other: 4 * MB },
    conversations: conversations.map((one) => ({ ...one })),
    output_days: outputDays,
    claude_logins: [{ set: false, days: 30 }, { home: "C:\\Users\\me\\.claude-work", set: true, days: 90 }],
  };
}

export async function specimenData(call: DataCall): Promise<DataReply> {
  await new Promise((done) => setTimeout(done, 300));
  switch (call.do) {
    case "clear_cache": {
      const bytes = cacheBytes;
      cacheBytes = 0;
      cached = 0;
      return { data: "cache_cleared", bytes };
    }
    case "list":
      return { data: "listed", ...space() };
    case "output_days":
      outputDays = call.days ?? 7;
      return { data: "output_days", days: outputDays };
    case "delete":
      return {
        data: "deleted",
        deletions: call.names.map((name) => {
          const at = conversations.findIndex((one) => one.name === name);
          if (at < 0) return { name, outcome: { outcome: "failed", why: "没有这段对话" } };
          const [gone] = conversations.splice(at, 1);
          return { name, outcome: { outcome: "deleted", bytes: gone!.bytes } };
        }),
      };
  }
}

let cached = 31 * MB;

export const specimenCache = {
  size: async () => cached,
  clear: async () => {
    cached = 0;
    cacheBytes = 0;
  },
};
