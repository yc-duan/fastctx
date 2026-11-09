// A history, as the specimen holds it: a project's documents of dated
// paragraphs, one with a decision and a paragraph that no longer holds, one
// kept in three versions, one taken down; the history of a conversation
// outside any project; and what a conversation of the project wrote in its
// own before it joined.

import type { HistoryCatalog, HistoryDocument, HistoryRevision, HistoryScope } from "../world/history";

const day = 86_400_000;
/** The specimen's moment, which the World's specimen sets. */
let now = Date.now();

type By = HistoryRevision["author"]["by"];
const claude: By = { kind: "resident", address: { resident: "claude-9a01", machine: "laptop" } };
const codex: By = { kind: "resident", address: { resident: "codex-7f3a", machine: "lab" } };
const user: By = { kind: "user" };

interface Kept {
  description: string;
  /** Each version: how long before the specimen's moment, who, where, its body. */
  versions: { ago: number; by: By; on: string; model: string | null; body: string; retired?: true }[];
}

/** The day `ago` before the specimen's moment, as a paragraph's date. */
function dayOf(ago: number): string {
  const at = new Date(now - ago);
  return `${at.getFullYear()}-${String(at.getMonth() + 1).padStart(2, "0")}-${String(at.getDate()).padStart(2, "0")}`;
}

function project(): Record<string, Kept> {
  const d = (n: number) => dayOf(n * day);
  return {
    "q16-fixed-point": {
      description: "控制环里的定点数格式 Q16：乘法、累加怎样做才不丢精度。Q16、fixed point、定点、溢出、舍入、fixed.rs、step()",
      versions: [
        { ago: 9 * day, by: claude, on: "laptop", model: "claude-opus-5-5", body: `${d(9)}：控制环里的数一律用 Q16（16 位小数）。两个 Q16 相乘，先放大到 64 位，乘完再右移 16 位收窄。` },
        {
          ago: 4 * day,
          by: user,
          on: "desktop",
          model: null,
          body: `${d(9)}：控制环里的数一律用 Q16（16 位小数）。两个 Q16 相乘，先放大到 64 位，乘完再右移 16 位收窄。\n\n${d(4)}：Decided「积分项先在 64 位里累加，整段算完才收窄一次，哪一步都不许先舍入」。`,
        },
        {
          ago: 3 * 3_600_000,
          by: codex,
          on: "lab",
          model: "gpt-6.1-sol",
          body: `${d(9)}：控制环里的数一律用 Q16（16 位小数）。两个 Q16 相乘，先放大到 64 位，乘完再右移 16 位收窄。\n\n${d(4)}：Decided「积分项先在 64 位里累加，整段算完才收窄一次，哪一步都不许先舍入」。\n\n${d(2)}：Superseded (see ${d(0)}) \`limits.rs\` 里只做比较、不做累加的地方可以直接用 Q16，不必先放大。\n\n${d(0)}：\`limits.rs\` 的比较也要先放大到 64 位：\`clamp()\` 在两个都接近上限的数上溢出过一次，\`step_response\` 跳到 -0.98。改在 \`a41c9e2\`。\n\n${d(0)}：Open 采样率从 1 kHz 提到 4 kHz 之后，Q16 的分辨率还够不够，没量过。`,
        },
      ],
    },
    "step-response-stall": {
      description: "step_response 停在 0.9612 到不了 1 的排查。稳态误差、积分饱和、step()、0.9612、steady state、不收敛",
      versions: [
        {
          ago: 2 * day,
          by: claude,
          on: "laptop",
          model: "claude-opus-5-5",
          body: `${d(2)}：\`cargo test -p ctrl step_response\` 停在 0.9612，到不了 1。\`fixed.rs:58\` 的 \`step()\` 先在 Q16 里累加积分再乘 Ki，误差小于 2⁻¹⁶ 时整步被舍掉。\n\n${d(2)}：改成在 64 位里累加、乘完再收窄（见 q16-fixed-point），稳态值 0.9999。`,
        },
      ],
    },
    "joint-model-training": {
      description: "关节模型在 lab 上的训练：设置、损失曲线、为什么停在第 280 轮。训练、train.py、learning rate、验证损失、过拟合",
      versions: [
        {
          ago: 5 * day,
          by: codex,
          on: "lab",
          model: "gpt-6.1-sol",
          body: `${d(5)}：\`python train.py --epochs 400\`，学习率 3e-4，lab 上两张卡。第 280 轮之后验证损失不再降，停在 0.041；输出在 lab 的 \`/data/mark50-sim/runs/0412/\`。`,
        },
      ],
    },
    "webrtc-weak-network": {
      description: "电脑走 4G 时控制链路的延迟实测。弱网、4G、WebRTC、延迟、抖动、往返时间、RTT",
      versions: [{ ago: 12 * day, by: user, on: "desktop", model: null, body: `${d(12)}：走 4G 时往返 180 ms 上下，偶尔 600 ms。控制环要能扛 600 ms 的抖动。` }],
    },
    "can-timestamps": {
      description: "CAN 帧的时间戳在哪一层打。CAN、时间戳、timestamp、驱动层、帧",
      versions: [
        { ago: 6 * day, by: claude, on: "laptop", model: "claude-opus-5-5", body: `${d(6)}：Decided「时间戳在驱动层打，取收到帧的那一刻，应用层只读不改」。` },
        { ago: 1 * day, by: user, on: "desktop", model: null, body: `${d(6)}：Decided「时间戳在驱动层打，取收到帧的那一刻，应用层只读不改」。`, retired: true },
      ],
    },
  };
}

function conversation(resident: string): Record<string, Kept> {
  const d = (n: number) => dayOf(n * day);
  if (resident === "claude-2b1c") {
    return {
      "q16-overflow-first-seen": {
        description: "Q16 积分第一次溢出时的现场，进项目之前记下的。积分溢出、overflow、step 测试、ki",
        versions: [{ ago: 9 * day, by: claude, on: "laptop", model: "claude-opus-5-5", body: `${d(9)}：\`step_response\` 跑到第 4 秒，积分项越过 \`i32::MAX\`，输出从 0.98 跳成 -1。` }],
      },
    };
  }
  if (resident !== "codex-6b19") return {};
  return {
    "printer-setup": {
      description: "书房那台打印机在家里网络上的设置。打印机、printer、IPP、驱动、局域网地址",
      versions: [{ ago: 1 * day, by: claude, on: "laptop", model: "claude-opus-5-5", body: `${d(1)}：打印机走 IPP，地址是 \`ipp://192.0.2.40/ipp/print\`，系统自带的驱动就够，不装厂商的。` }],
    },
  };
}

function kept(scope: HistoryScope): Record<string, Kept> {
  return scope.kind === "project" ? project() : conversation(scope.resident);
}

function revision(name: string, one: Kept, index: number): HistoryRevision {
  const version = one.versions[index]!;
  const host = version.by.kind === "resident" ? { agent: version.by.address.resident.startsWith("codex") ? "codex" : "claude" } : null;
  return {
    name,
    description: one.description,
    version: index + 1,
    ...(version.retired ? { retired: true as const } : {}),
    author: { by: version.by, on: version.on, at: new Date(now - version.ago).toISOString(), model: version.model, host },
  };
}

export function specimenHistory(scope: HistoryScope, at: Date): HistoryCatalog {
  now = at.getTime();
  const documents = Object.entries(kept(scope)).map(([name, one]) => revision(name, one, one.versions.length - 1));
  documents.sort((a, b) => Date.parse(b.author.at) - Date.parse(a.author.at));
  return { documents };
}

export function specimenHistoryDocument(scope: HistoryScope, name: string, version: number | null, at: Date): HistoryDocument {
  now = at.getTime();
  const all = kept(scope);
  const one = all[name] ?? Object.values(all)[0] ?? Object.values(conversation("codex-6b19"))[0]!;
  const index = version === null ? one.versions.length - 1 : Math.min(Math.max(version - 1, 0), one.versions.length - 1);
  const chosen = revision(name, one, index);
  return {
    revision: chosen,
    text: `---\nname: ${name}\ndescription: ${one.description}\n---\n\n${one.versions[index]!.body}`,
    versions: one.versions.map((_, at) => revision(name, one, at)),
  };
}
