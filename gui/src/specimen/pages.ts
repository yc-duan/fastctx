// mark50's pages, to look at them without a World: a tuning board Codex
// wrote on lab, in three versions, and a short table Claude wrote. What the
// user agrees to is kept for the window, and the board's acts are refused
// until they agree, as the core would refuse them.

import html from "./page-tuning.html?raw";
import { answers } from "../page/specimen-answers";
import { holds, type PageAct, type PageConsent, type PageInvocation, type PageSummary, type PageVersion, type PageView } from "../world/pages";
import type { PageWorld } from "../pages/view";

const codex = { kind: "resident" as const, address: { resident: "codex-0d1e", machine: "lab" } };
const claude = { kind: "resident" as const, address: { resident: "claude-2b1c", machine: "laptop" } };
/** The specimen World's moment (`world.ts`), which its times count back from. */
const NOW = Date.parse("2026-10-04T13:40:00Z");
const ago = (minutes: number) => new Date(NOW - minutes * 60_000).toISOString();

const acts: PageAct[] = [
  { kind: "run", machine: "lab", script: "nvidia-smi --query-gpu=utilization.gpu --format=csv" },
  { kind: "say", to: { resident: "codex-0d1e", machine: "lab" } },
];

const tuning: PageVersion[] = [
  { version: 1, base: null, title: "mark50 调参看板", description: "第 1–6 组的损失曲线", acts: [], withdrawn: false, restored_from: null, by: codex, on: "lab", at: ago(60 * 26) },
  { version: 2, base: 1, title: "mark50 调参看板", description: "加上各组结果表与训练日志", acts: [], withdrawn: false, restored_from: null, by: codex, on: "lab", at: ago(60 * 5) },
  { version: 3, base: 2, title: "mark50 调参看板", description: "第 7 组之后的曲线、显卡占用，能让 Codex 重跑一组", acts, withdrawn: false, restored_from: null, by: codex, on: "lab", at: ago(42) },
];

const table: PageVersion[] = [
  { version: 1, base: null, title: "Q16 与 Q32 的误差对照", description: "同一组输入下两种定点格式的最大误差", acts: [], withdrawn: false, restored_from: null, by: claude, on: "laptop", at: ago(60 * 3) },
];

const kept: Record<string, { versions: PageVersion[]; html: (version: number) => string | null }> = {
  tuning: { versions: tuning, html: () => html },
  "q-formats": {
    versions: table,
    html: () =>
      `<!doctype html><html lang="zh-CN"><body><h1>Q16 与 Q32 的误差对照</h1><w-card title="最大误差" note="同一组 4096 个输入"><w-table columns="input:输入,q16:Q16,q32:Q32"><script type="application/json">[{"input":"阶跃","q16":0.0021,"q32":0.0000004},{"input":"斜坡","q16":0.0187,"q32":0.0000031},{"input":"正弦","q16":0.0094,"q32":0.0000012}]</script></w-table></w-card></body></html>`,
  },
};

let consent: PageConsent | null = null;

export function specimenPages(): PageSummary[] {
  return Object.entries(kept).map(([page, { versions }]) => ({ page, current: versions[versions.length - 1]! }));
}

export function specimenPageWorld(page: string): PageWorld {
  const held = kept[page];
  const latest = () => held!.versions[held!.versions.length - 1]!;
  const after = () => new Promise((resolve) => setTimeout(resolve, 300));
  return {
    async read(version) {
      if (!held) throw new Error(`mark50 has no page ${page}`);
      const publication = version === null ? latest() : held.versions.find((v) => v.version === version);
      if (!publication) throw new Error(`${page} has no version ${version}`);
      return { town: "mark50", page, publication, html: publication.withdrawn ? null : held.html(publication.version), consent: page === "tuning" ? consent : null };
    },
    async history() {
      return held?.versions ?? [];
    },
    async consent(version, base, given) {
      await after();
      if ((consent?.version ?? null) !== base) throw new Error("the consent changed since it was read; read it again");
      consent = { version: (consent?.version ?? 0) + 1, base, page_version: version, acts: given, stamp: { by: { kind: "user" }, on: "desktop", at: ago(0) } };
      return consent;
    },
    async withdraw(base) {
      await after();
      const current = latest();
      if (current.version !== base) throw new Error(`version ${current.version} is current now; read it again`);
      held!.versions.push({ ...current, version: base + 1, base, acts: [], withdrawn: true, restored_from: null, by: { kind: "user" }, on: "desktop", at: ago(0) });
    },
    async restore(base, version) {
      await after();
      const from = held!.versions.find((v) => v.version === version)!;
      held!.versions.push({ ...from, version: base + 1, base, withdrawn: false, restored_from: version, by: { kind: "user" }, on: "desktop", at: ago(0) });
    },
  };
}

/** A page's tools in the specimen: the World as the specimen has it for
 *  `world.now`, made-up answers for the rest, and acts refused unless the
 *  user agreed to them and they were declared. */
export function specimenPageTools(view: PageView, now: (tool: string, args: Record<string, unknown>) => Promise<unknown>): (tool: string, args: Record<string, unknown>) => Promise<unknown> {
  const gate = (action: PageInvocation, act: PageAct) => {
    if (!holds(view.publication.acts, act)) throw new Error(`version ${view.publication.version} of this page did not declare that`);
    if (!holds(consent?.acts ?? [], act)) throw new Error("you have not agreed to this page doing that");
    return action;
  };
  return async (tool, args) => {
    if (tool === "world.now" || tool === "open") return now(tool, args);
    if (tool === "run") gate({ kind: "run", machine: String(args.machine), script: String(args.command) }, { kind: "run", machine: String(args.machine), script: String(args.command) });
    if (tool === "say") gate({ kind: "say", to: { resident: String(args.resident), machine: String(args.machine) }, text: String(args.text) }, { kind: "say", to: { resident: String(args.resident), machine: String(args.machine) } });
    const answer = answers[tool];
    if (!answer) throw new Error(`Fastctx has no tool ${tool} for a page`);
    await new Promise((resolve) => setTimeout(resolve, tool === "run" || tool === "say" ? 900 : 120));
    return answer(args);
  };
}
