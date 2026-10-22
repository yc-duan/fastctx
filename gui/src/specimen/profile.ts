// The account's own configuration as the specimen has it: a prompt, a few
// preferences, MCP servers on this computer, the laptop and the server and
// two reached at an address, kept in memory so adding, editing and removing
// can be looked at.

import type { CommandMcp, Found, Listed, McpCall, McpReply, Profile, ProfileView, Server } from "../world/profile";
import type { ProfileCalls } from "../app/profile-account";

const pause = (ms: number) => new Promise((done) => setTimeout(done, ms));

/** Each machine's own record of the command servers it keeps. */
const kept = new Map<string, Map<string, CommandMcp>>([
  [
    "desktop",
    new Map([
      ["unity", { command: "uvx", args: ["--offline", "--from", "mcpforunityserver==10.1.0", "mcp-for-unity", "--transport", "stdio"], env: { PYTHONUTF8: "1" } }],
      ["blender", { command: "uvx", args: ["mcp-for-blender"], env: { PYTHONUTF8: "1" } }],
    ]),
  ],
  ["laptop", new Map([["figma", { command: "npx", args: ["-y", "figma-developer-mcp", "--stdio"], env: { FIGMA_API_KEY: "••••••" } }]])],
  ["server", new Map()],
]);

let profile: Profile = {
  version: 3,
  prompt: "一律用中文和我交流，代码注释用英文。\n动手之前先读相关的设计文档；拿不准的地方先问我，不要猜。\n改界面时三套主题、电脑与手机都要截图看过。",
  claude: { language: "中文", model: "claude-opus-5-5", autoCompactWindow: 600_000, env: { CLAUDE_AUTOCOMPACT_PCT_OVERRIDE: "80" } },
  codex: { model_reasoning_summary: "concise", model_reasoning_effort: "high" },
  servers: {
    unity: { kind: "command", machine: "desktop" },
    blender: { kind: "command", machine: "desktop" },
    figma: { kind: "command", machine: "laptop" },
    postgres: { kind: "command", machine: "server" },
    linear: { kind: "remote", url: "https://mcp.linear.app/mcp" },
    context7: { kind: "remote", url: "https://mcp.context7.com/mcp", headers: { CONTEXT7_API_KEY: "••••••" } },
  },
};

// `?noServers`: an account that keeps none yet, its page as it starts.
if (new URLSearchParams(location.search).has("noServers")) {
  profile = { ...profile, servers: {} };
  for (const servers of kept.values()) servers.clear();
}

const found: Found[] = [
  { source: { app: "claude_desktop" }, name: "unityMCP", server: { kind: "command" } },
  { source: { app: "claude_desktop" }, name: "blender", server: { kind: "command" } },
  { source: { app: "claude_code", home: "C:/Users/me/.claude" }, name: "playwright", server: { kind: "command" } },
  { source: { app: "cursor" }, name: "github", server: { kind: "remote", url: "https://api.githubcopilot.com/mcp/" } },
];

function listed(): Listed[] {
  return Object.entries(profile.servers ?? {}).map(([name, entry]): Listed => {
    if (entry.kind === "command") {
      const server = kept.get(entry.machine)?.get(name);
      return { name, server: { kind: "command", machine: entry.machine, state: server ? { state: "kept" } : { state: "missing" } } };
    }
    return entry.kind === "remote" ? { name, server: entry } : { name, server: { kind: "unknown" } };
  });
}

function view(): ProfileView {
  return { hub: "home", account: "me", profile: structuredClone(profile), kept: true };
}

function keep(name: string, server: Server, machine: string | undefined): McpReply {
  const servers = { ...(profile.servers ?? {}) };
  if (server.kind === "command") {
    const on = machine ?? "desktop";
    const { kind: _, ...command } = server;
    if (!kept.has(on)) kept.set(on, new Map());
    kept.get(on)!.set(name, command);
    servers[name] = { kind: "command", machine: on };
  } else servers[name] = server;
  profile = { ...profile, servers, version: profile.version + 1 };
  return { reply: "kept", name, version: profile.version };
}

async function answer(call: McpCall): Promise<McpReply> {
  await pause(call.do === "test" ? 1400 : call.do === "found" ? 700 : 250);
  switch (call.do) {
    case "list":
      return { reply: "listed", servers: listed() };
    case "keep":
      return keep(call.name, call.server, call.machine);
    case "remove": {
      const servers = { ...(profile.servers ?? {}) };
      const entry = servers[call.name];
      if (entry?.kind === "command") kept.get(entry.machine)?.delete(call.name);
      delete servers[call.name];
      profile = { ...profile, servers, version: profile.version + 1 };
      return { reply: "removed", version: profile.version };
    }
    case "test":
      if (call.name === "unity") return { reply: "tested", tools: ["manage_scene", "manage_gameobject", "manage_asset", "manage_script", "read_console", "execute_menu_item", "manage_editor", "manage_shader", "find_in_scene", "set_component_property", "create_prefab", "run_tests", "screenshot", "play_mode"] };
      if (call.name === "blender") return { reply: "tested", tools: [], problem: "it closed its output before answering initialize", log: "Traceback (most recent call last):\n  File \"mcp_for_blender/server.py\", line 41, in connect\nConnectionRefusedError: [WinError 10061] No connection could be made because the target machine actively refused it (127.0.0.1:9876)" };
      return { reply: "tested", tools: ["get_figma_data", "download_figma_images"] };
    case "import":
      return keep(call.as_name, { kind: "command", command: "uvx", args: [call.name] }, call.machine);
    case "found":
      return { reply: "found", machine: call.machine ?? "desktop", servers: found };
  }
}

export function specimenProfileCalls(): ProfileCalls {
  return {
    read: async () => {
      await pause(200);
      return view();
    },
    write: async (_hub, _account, written) => {
      await pause(300);
      if (written.version !== profile.version) throw new Error(`the profile changed: version ${written.version} was read, current version is ${profile.version}`);
      profile = { ...structuredClone(written), servers: profile.servers ?? {}, version: profile.version + 1 };
      return profile.version;
    },
    mcp: (_hub, _account, call) => answer(call),
  };
}
