// What a specimen does when it is asked to act: answers that change
// nothing anywhere, so each part of the interface can be looked at in a
// plain browser.

import { MemoryLines } from "../longview";
import type { Removing } from "../app/removal";
import type { PhoneSeam } from "../app/phone";
import type { RenewalStatus } from "../world/renewal";
import type { RemovalItem, RemovalPlan, Setup } from "../world/bridge";
import type { TokenPeek } from "../world/accounts";
import type { FileRange } from "../world/file-read";
import type { TextRead } from "../app/reader";
import type { Offers } from "../world/control";
import type { Attachment, UploadTarget } from "../world/attachments";
import type { AttachSource } from "../conversation/attach";

/** What a specimen conversation offers, so its menus can be looked at. */
export const specimenOffers: Offers = {
  controls: ["command", "compact", "model", "rename", "fork", "rewind", "answer"],
  commands: [
    { name: "review", description: "Review the changes so far", args: null, arguments: "optional", source: "codex" },
    { name: "init", description: "Write an AGENTS.md for this place", args: null, arguments: "none", source: "codex" },
    { name: "goal", description: "Set, show or clear the goal", args: "[get | clear | set <objective>]", arguments: "optional", source: "codex" },
    { name: "fast", description: "Faster responses", args: null, arguments: "none", source: "codex" },
  ],
  models: [
    { id: "gpt-6.1-sol", name: "GPT-6.1 Sol", description: "The default", efforts: ["low", "medium", "high", "xhigh"], default_effort: "medium", default: true, service_tiers: [{ id: "priority", name: "Fast", description: "Faster responses" }, { id: "ultrafast", name: "Ultrafast", description: "Fastest responses, at a higher cost" }], default_service_tier: null },
    { id: "gpt-6-astra", name: "GPT-6 Astra", description: "Deeper, slower", efforts: ["medium", "high", "xhigh"], default_effort: "high", default: false, service_tiers: [{ id: "priority", name: "Fast", description: "Faster responses" }], default_service_tier: null },
  ],
  model: "gpt-6.1-sol",
  effort: "medium",
  service_tier: null,
  usage: {
    context: null,
    limits: [
      { name: "codex primary 300m", used_percent: 34, resets_at: "2026-10-04T18:40:00Z" },
      { name: "codex secondary 10080m", used_percent: 61, resets_at: "2026-10-08T03:00:00Z" },
    ],
    cost_usd: null,
    raw: { tokenUsage: { last: { inputTokens: 41_822, cachedInputTokens: 38_400, outputTokens: 1_204, reasoningOutputTokens: 512, totalTokens: 43_026 }, modelContextWindow: 258_400 } },
  },
};

/** What a specimen Claude conversation offers: the models its mod reports
 *  from Claude Code's own `/config`, and the five efforts. */
export const specimenClaudeOffers: Offers = {
  controls: ["command", "compact", "model", "rename", "fork", "rewind", "answer"],
  commands: [
    { name: "review", description: "Review a pull request", args: null, arguments: "optional", source: "claude" },
    { name: "init", description: "Write a CLAUDE.md for this place", args: null, arguments: "none", source: "claude" },
  ],
  models: ["opus", "fable", "sonnet", "haiku"].map((id, index) => ({
    id,
    name: id[0]!.toUpperCase() + id.slice(1),
    description: "",
    efforts: ["low", "medium", "high", "xhigh", "max"],
    default_effort: id === "fable" ? "high" : "medium",
    default: index === 0,
    service_tiers: [],
    default_service_tier: null,
  })),
  model: "opus",
  effort: "medium",
  service_tier: null,
  usage: {
    context: null,
    limits: [
      { name: "five_hour", used_percent: 42, resets_at: "2026-10-04T18:40:00Z" },
      { name: "seven_day", used_percent: 18, resets_at: "2026-10-08T03:00:00Z" },
    ],
    cost_usd: null,
    raw: {
      context: {
        tokens: 61_280,
        window: 200_000,
        breakdown: {
          categories: [
            { name: "System prompt", tokens: 3_120, kind: "used" },
            { name: "System tools", tokens: 11_480, kind: "used" },
            { name: "MCP tools", tokens: 2_310, kind: "used" },
            { name: "Memory files", tokens: 1_870, kind: "used" },
            { name: "Skills", tokens: 940, kind: "used" },
            { name: "Messages", tokens: 41_560, kind: "used" },
            { name: "MCP tools (deferred)", tokens: 8_200, kind: "deferred" },
            { name: "Free space", tokens: 105_720, kind: "free" },
            { name: "Autocompact buffer", tokens: 33_000, kind: "buffer" },
          ],
          apiUsage: { input_tokens: 1_312, cache_read_input_tokens: 58_204, cache_creation_input_tokens: 1_764, output_tokens: 862 },
        },
      },
    },
  },
};

/** Files a specimen's machine has in its project's places, found by the
 *  letters typed in order, as a fuzzy finder does. */
export async function specimenFiles(query: string) {
  const files = [
    "D:/work/mark50/src/ctrl/pid.rs",
    "D:/work/mark50/src/ctrl/fixed.rs",
    "D:/work/mark50/src/ctrl/loop.rs",
    "D:/work/mark50/src/telemetry/frame.rs",
    "D:/work/mark50/specs/控制环.md",
    "D:/work/mark50/specs/遥测.md",
    "D:/work/mark50/tests/pid_saturation.rs",
    "D:/work/mark50/Cargo.toml",
  ];
  const wanted = query.toLowerCase();
  const fits = (path: string) => {
    let at = 0;
    for (const letter of path.toLowerCase()) if (letter === wanted[at]) at += 1;
    return at === wanted.length;
  };
  await new Promise((resolve) => setTimeout(resolve, 120));
  return files.filter(fits).map((path) => ({ path }));
}

/** A specimen computer, upgraded from an old fastctx: it changes nothing
 *  anywhere, only how the settings show it. */
/** What a code lets this computer into, as a specimen's server says: one
 *  with USED in it meets a Fastctx used before, one with PLAIN logs in to
 *  an account, any other makes this computer a managed machine. */
export async function specimenPeek(token: string): Promise<TokenPeek | null> {
  await new Promise((resolve) => setTimeout(resolve, 500));
  if (!token.startsWith("FX5")) throw new Error("not a token: a code starts with FX5");
  const plain = token.includes("PLAIN");
  return {
    hub: "home",
    account: "me",
    managed: !plain,
    ...(plain ? {} : { label: "机房 A" }),
    expires: new Date(Date.now() + 6 * 86_400_000).toISOString(),
    ...(token.includes("USED") ? { refused: { why: "conversations" as const } } : {}),
  };
}

export function specimenComputer() {
  let kept = false;
  let days = 7;
  let proxy: string | null = null;
  const after = <T>(value: T) => new Promise<T>((resolve) => setTimeout(() => resolve(value), 900));
  return {
    proxy: {
      get: async () => proxy,
      set: async (url: string | null) => {
        if (url && !/^(https?:\/\/)?[^\s/:@]+(:\d+)?$/.test(url)) throw new Error(`${url} is no HTTP proxy: write it as http://host:port`);
        proxy = await after(url);
        return proxy;
      },
    },
    retention: {
      read: async () => days,
      write: async (n: number) => {
        days = n;
      },
    },
    setup: async (): Promise<Setup> => ({
      installed: "C:/Users/me/.fastctx/bin/fastctx.exe",
      carried: null,
      same: false,
      service: kept,
      codex: "in",
      claude: "out",
      legacy: {
        kind: "done",
        record: "C:/Users/me/.fastctx/upgrade-clean.json",
        archives: ["C:/Users/me/.fastctx/legacy-archive-2026-10-04"],
        archived: 14,
        tidied: 6,
        waiting: 1,
      },
    }),
    keep: () => {
      kept = true;
      return after({ kind: "installed" as const });
    },
    removing: specimenRemoving(),
    // An older World than the interface, as a computer that has not been
    // brought up to date shows it.
    renewing: {
      state: async (): Promise<RenewalStatus> => ({
        node: null,
        mismatch: { node_version: "1.0.0", node_protocol: 5, client_version: "1.0.0", client_protocol: 6 },
        pending: null,
        phase: null,
        error: null,
      }),
      renew: () => after(undefined).then(() => Promise.reject(new Error("this is a specimen"))),
    },
  };
}

/** A specimen machine's files: a decision, read whole, and a training log,
 *  read from its end. */
export async function specimenRead(machine: string, path: string, range: FileRange): Promise<TextRead> {
  await new Promise((resolve) => setTimeout(resolve, 150));
  const modified = "2026-10-04T13:20:00Z";
  if (range.kind === "tail") {
    const lines = Array.from({ length: 400 }, (_, n) => `epoch ${n + 1}/400  loss ${(0.9 * Math.exp(-n / 120) + 0.04).toFixed(4)}  overshoot ${(0.31 * Math.exp(-n / 90) + 0.02).toFixed(3)}  step ${(n + 1) * 250}`);
    const text = `${lines.join("\n")}\n`;
    const size = 5_812_224;
    return { info: { machine, path, file: { size, modified }, offset: size - text.length, length: text.length }, text };
  }
  const text = `# 控制环

关节控制环的结构、定点数的取舍，以及在真机上验收的条件。

## 结构

每个关节一个 PID 环，1 kHz；积分用 Q16.16，进 \`narrow()\` 之前先饱和，不回绕。

| 量 | 格式 | 范围 |
|---|---|---|
| 误差 | Q16.16 | ±32767 |
| 积分 | Q32.32 | 饱和到 ±2³¹ |

## 验收

在 lab 上跑 400 轮仿真，阶跃响应的超调不超过 5%，稳态误差在 $\\pm 0.2^\\circ$ 以内。
`;
  return { info: { machine, path, file: { size: text.length, modified }, offset: 0, length: text.length }, text };
}

/** A specimen carries nothing anywhere: a file for a new conversation
 *  arrives after a moment, in a place made up for it, so the sheet can be
 *  looked at. */
export function specimenUpload(
  machine: string,
  target: UploadTarget,
  source: AttachSource,
  progress: (sent: number) => void,
): Promise<Attachment> {
  const name = source.kind === "bytes" ? source.file.name : (source.path.split(/[\/]/).pop() ?? source.path);
  const size = source.kind === "bytes" ? source.file.size : 0;
  const resident = target.kind === "new" ? `${target.agent}-specimen` : target.resident;
  return new Promise((resolve) => {
    let sent = 0;
    const step = () => {
      sent = Math.min(size, sent + Math.max(1, size / 8));
      progress(sent);
      if (sent < size) {
        setTimeout(step, 120);
        return;
      }
      resolve({
        id: `attachment-${Math.random().toString(16).slice(2, 10)}` as Attachment["id"],
        owner: { resident, machine } as Attachment["owner"],
        town: target.kind === "resident" ? null : target.town,
        path: `/home/me/.fastctx/world/attachments/${name}`,
        name,
        media_type: source.kind === "bytes" ? source.file.type || "application/octet-stream" : "application/octet-stream",
        size,
        sha256: "0".repeat(64),
      });
    };
    step();
  });
}

/** An Honor phone that has not been set up for the background yet: calls
 *  allowed, battery optimisation still on. */
export const specimenPhone: PhoneSeam = {
  status: async () => ({
    notificationsAllowed: true,
    alertChannelAllowed: true,
    batteryOptimizationExempt: false,
    powerSave: false,
    manufacturer: "HONOR",
    model: "ANN-AN00",
    deviceName: "HONOR 80",
    background: { vendor: "honor", backgroundRestricted: false },
  }),
  allow: async () => {},
  open: () => Promise.reject(new Error("this is a specimen")),
};

/** What removing fastctx from a specimen computer would take: one that is
 *  also the hub of a World, with an npm fastctx on its PATH; its steps
 *  tick off one by one, one of them failing. */
function specimenRemoving(): Removing {
  const item = (id: string, kind: string, where: string | null, admin = false): RemovalItem => ({ id, kind, what: kind, where, admin });
  const plan = (everything: boolean): RemovalPlan => ({
    everything,
    steps: [
      item("service", "service", "fastctx-node-me", true),
      item("task", "task", "fastctx-node-me-check", true),
      item("codex", "codex", "C:/Users/me/.codex"),
      item("claude", "claude", "C:/Users/me/.claude"),
      ...(everything
        ? [item("hub-service", "hub-service", "fastctx-hub-lab", true), item("firewall", "firewall", "fastctx-hub-lab", true), item("hub-data", "hub-data", "C:/Users/me/.fastctx/hub"), item("logout", "logout", "lab · me"), item("data", "data", "C:/Users/me/.fastctx")]
        : []),
      item("path", "path", "C:/Users/me/.fastctx/bin"),
      item("program", "program", "C:/Users/me/.fastctx/bin/fastctx.exe"),
      item("app", "app", "C:/Users/me/AppData/Local/Fastctx"),
    ],
    keeps: everything ? [] : [item("data", "data", "C:/Users/me/.fastctx/world"), item("hub-data", "hub-data", "C:/Users/me/.fastctx/hub")],
    hub: { name: "lab", dir: "C:/Users/me/.fastctx/hub" },
    other: { path: "C:/Users/me/AppData/Roaming/npm/fastctx.cmd", version: "0.2.6" },
  });
  return {
    plan: (everything) => new Promise((resolve) => setTimeout(() => resolve(plan(everything)), 500)),
    run: async (everything, onEvent) => {
      for (const step of plan(everything).steps) {
        await new Promise((resolve) => setTimeout(resolve, 420));
        onEvent(step.id === "claude" ? { step: step.id, result: "failed", why: "settings.json changed while it was being read" } : { step: step.id, result: step.id === "task" ? "skipped" : "done", why: null });
      }
      onEvent({ left: [{ kind: "claude", what: "In Claude Code", where: "C:/Users/me/.claude/settings.json", why: "it changed while it was being read" }] });
    },
    finish: () => Promise.reject(new Error("this is a specimen")),
  };
}

/** A picture for a specimen's sent file: a step response sketched on
 *  squared paper, drawn here at `max` pixels on its longer side. */
export function specimenPicture(name: string, max: number): string {
  const canvas = document.createElement("canvas");
  const wide = name.includes("scope");
  canvas.width = max;
  canvas.height = Math.round(max * (wide ? 0.5 : 0.66));
  const g = canvas.getContext("2d")!;
  const { width, height } = canvas;
  g.fillStyle = "#f4efe6";
  g.fillRect(0, 0, width, height);
  g.strokeStyle = "rgba(60, 60, 60, 0.12)";
  g.lineWidth = Math.max(1, max / 520);
  const step = width / 16;
  for (let x = 0; x <= width; x += step) {
    g.beginPath();
    g.moveTo(x, 0);
    g.lineTo(x, height);
    g.stroke();
  }
  for (let y = 0; y <= height; y += step) {
    g.beginPath();
    g.moveTo(0, y);
    g.lineTo(width, y);
    g.stroke();
  }
  const plot = (colour: string, wobble: number, damping: number) => {
    g.strokeStyle = colour;
    g.lineWidth = Math.max(2, max / 180);
    g.beginPath();
    for (let i = 0; i <= 200; i += 1) {
      const t = i / 200;
      const y = 1 - Math.exp(-t * damping) * Math.cos(t * wobble);
      const px = width * 0.06 + t * width * 0.88;
      const py = height * 0.85 - y * height * 0.6;
      if (i === 0) g.moveTo(px, py);
      else g.lineTo(px, py);
    }
    g.stroke();
  };
  plot("#4a4a48", wide ? 40 : 18, wide ? 3 : 5);
  plot("#d97757", 6, 7);
  return canvas.toDataURL("image/png");
}

/** A specimen job's output, growing a line a second while it is read. */
export function specimenOutput(command: string): MemoryLines & { close(): Promise<void> } {
  const lines = [`$ ${command}`, "   Compiling mark50 v0.4.2 (/home/me/mark50)", "    Finished `test` profile [unoptimized + debuginfo] target(s) in 41.07s", "     Running unittests src/lib.rs"];
  const source = new MemoryLines(lines, false);
  let n = 0;
  const timer = window.setInterval(() => {
    n += 1;
    lines.push(`test ctrl::fixed::step_response_${String(n).padStart(3, "0")} ... ok`);
    source.replace([...lines], false, lines.length - 1);
  }, 1000);
  return Object.assign(source, {
    close: async () => {
      clearInterval(timer);
    },
  });
}
