// A project's memory, as the specimen holds it: two decisions, one kept in
// three versions, and three journals, written by agents and by the user.

import type { MemoryCatalog, MemoryDocument, MemoryRevision } from "../world/memory";

const day = 86_400_000;
/** The specimen's moment, which the World's specimen sets. */
let now = Date.now();

function revision(kind: "journal" | "spec", name: string, description: string, version: number, ago: number, by: MemoryRevision["author"]["by"], on: string, model: string | null): MemoryRevision {
  const host = by.kind === "resident" ? { agent: by.address.resident.startsWith("codex") ? "codex" : "claude" } : null;
  return { kind, name, description, version, author: { by, on, at: String(ago), model, host } };
}

/** The revisions with their moments reckoned back from the specimen's. */
function dated(one: MemoryRevision): MemoryRevision {
  return { ...one, author: { ...one.author, at: new Date(now - Number(one.author.at)).toISOString() } };
}

const claude = { kind: "resident" as const, address: { resident: "claude-9a01", machine: "laptop" } };
const codex = { kind: "resident" as const, address: { resident: "codex-7f3a", machine: "laptop" } };

const format = [
  revision("spec", "定点数格式", "控制环里的数一律用 Q16，乘法先放大再收窄，哪一步都不许先舍入", 1, 9 * day, claude, "laptop", "claude-opus-4-1"),
  revision("spec", "定点数格式", "控制环里的数一律用 Q16，乘法先放大再收窄，哪一步都不许先舍入", 2, 4 * day, { kind: "user" }, "desktop", null),
  revision("spec", "定点数格式", "控制环里的数一律用 Q16，乘法先放大再收窄，哪一步都不许先舍入", 3, 3 * 3_600_000, codex, "lab", "gpt-6.1-sol"),
];

const journals = [
  revision("journal", "q16-溢出排查", "step_response 卡在 0.9612 的来龙去脉：积分先舍入是罪魁", 1, 2 * day, claude, "laptop", "claude-opus-4-1"),
  revision("journal", "关节模型训练", "400 轮训练在 lab 上的设置、损失曲线和停下来的原因", 1, 5 * day, codex, "lab", "gpt-6.1-sol"),
  revision("journal", "弱网下的-webrtc", "电脑走 4G 时控制延迟的实测", 1, 12 * day, { kind: "user" }, "desktop", null),
];

const bus = revision("spec", "can-帧时间戳", "时间戳以收到帧的那一刻为准，驱动层打，应用层不再改", 1, 6 * day, claude, "laptop", "claude-opus-4-1");

export function specimenCatalog(at: Date): MemoryCatalog {
  now = at.getTime();
  return { specs: [format[format.length - 1]!, bus].map(dated), journals: journals.map(dated) };
}

const texts: Record<string, string[]> = {
  定点数格式: [
    "## 格式\n\n控制环里的数一律用 **Q16**（16 位小数）。\n\n## 乘法\n\n两个 Q16 相乘，先放大到 64 位，乘完再右移 16 位收窄。",
    "## 格式\n\n控制环里的数一律用 **Q16**（16 位小数）。\n\n## 乘法\n\n两个 Q16 相乘，先放大到 64 位，乘完再右移 16 位收窄。\n\n## 累加\n\n积分项先在 64 位里累加，整段算完才收窄一次。",
    "## 格式\n\n控制环里的数一律用 **Q16**（16 位小数）。\n\n## 乘法\n\n两个 Q16 相乘，先放大到 64 位，乘完再右移 16 位收窄。\n\n## 累加\n\n积分项先在 64 位里累加，整段算完才收窄一次。哪一步都不许先舍入：`fixed.rs:58` 的 `step()` 就是因为先舍入，误差小于 2⁻¹⁶ 时整步被吞掉。\n\n## 例外\n\n- 只做比较不做累加的地方（`limits.rs`）可以直接用 Q16。",
  ],
  "can-帧时间戳": ["## 规矩\n\n- 时间戳在驱动层打，取收到帧的那一刻。\n- 应用层只读，不改。"],
  "q16-溢出排查": ["## 现象\n\n`step_response` 停在 0.9612，到不了 1。\n\n## 查到的\n\n`fixed.rs:58` 的 `step()` 先在 Q16 里累加积分再乘 Ki，误差小于 2⁻¹⁶ 时整步被舍掉。\n\n## 怎样改的\n\n改成在 64 位里累加，乘完再收窄；改后稳态值 0.9999。"],
  关节模型训练: ["## 设置\n\n400 轮，学习率 3e-4，lab 上两张卡。\n\n## 结果\n\n第 280 轮之后验证损失不再降，停在 0.041。"],
  "弱网下的-webrtc": ["走 4G 时往返 180 ms 上下，偶尔 600 ms。控制环要能扛 600 ms 的抖动。"],
};

export function specimenDocument(name: string, version: number | null, at: Date): MemoryDocument {
  now = at.getTime();
  const versions = name === "定点数格式" ? format : [...journals, bus].filter((one) => one.name === name);
  const chosen = versions.find((one) => one.version === version) ?? versions[versions.length - 1] ?? journals[0]!;
  const body = texts[chosen.name]?.[chosen.version - 1] ?? "";
  return { revision: dated(chosen), text: `---\nname: ${chosen.name}\ndescription: ${chosen.description}\n---\n\n${body}`, versions: versions.map(dated) };
}
