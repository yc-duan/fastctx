// What the window before a change to an agent says (`world-layers.md`,
// 接入之前讲清楚做了什么): a card each kind of change, in concise written
// words, and what the change does not do.

import type { Change } from "../world/logins";

/** A card's two lines: what changes, and what that means for the user. */
export interface ChangeWords {
  title: string;
  line: string;
}

export interface DiscloseWords {
  /** The window's title, for connecting the agent or taking it out. */
  connect(agent: string): string;
  remove(agent: string): string;
  /** Under the title: which machine, which login. */
  where(machine: string, who: string): string;
  /** What connecting is, behind the question mark. */
  what: string;
  about(agent: string): string;
  /** Each kind of change, putting the World's in and taking it out. */
  into: Record<Change, ChangeWords>;
  out: Record<Change, ChangeWords>;
  /** On the limits card: a limit the user set, raised. */
  raised(current: number, requested: number): string;
  /** What connecting never does, on a card of its own. */
  never: ChangeWords;
  /** A change that finds nothing to change. */
  same: string;
  /** Under the cards: sessions already open, and the World's lines the
   *  user changed, put back. */
  sessions(agent: "codex" | "claude", into: boolean): string;
  takeBack: string;
  /** Beside the button: how it is undone. */
  undo: string;
  undone: string;
  /** The button. */
  apply: string;
  take: string;
  /** A file's changed lines, folded: how many. */
  lines(added: number, removed: number): string;
  /** More lines than the card shows. */
  more(n: number): string;
  /** The files moved on since the window was opened. */
  moved: string;
}

export const zhDisclose: DiscloseWords = {
  connect: (agent) => `接入 ${agent}`,
  remove: (agent) => `移除 ${agent}`,
  where: (machine, who) => `${machine}，${who}`,
  what: "关于接入",
  about: (agent) => `你仍在自己的 ${agent} 中工作，它经由 Fastctx 通达你的每一台机器；所做之事，尽在「活动」中。`,
  into: {
    tools: { title: "新增工具 run", line: "在你的各台机器上执行命令、读写文件，并在机器之间传送文件。" },
    shell: { title: "停用自带的 shell", line: "命令改由 run 执行；Fastctx 未运行时，无法执行命令。" },
    guidance: { title: "写入使用说明", line: "说明何时使用 run；原有内容不变。" },
    limits: { title: "放宽输出上限", line: "单次工具输出不再截断，run 的结果完整送达。" },
    plugin: { title: "安装 Fastctx 插件", line: "经 Claude Code 自身的命令安装；Fastctx 发起的对话借此收发消息、接受唤醒。" },
    consent: { title: "免除逐项审批", line: "仅限 Fastctx 发起的对话；你自行开启的会话仍逐项请求批准。" },
    legacy: { title: "接管旧版配置", line: "早期版本写入的配置，替换为当前版本。" },
  },
  out: {
    tools: { title: "移除工具 run", line: "此后不能再经 Fastctx 访问其他机器。" },
    shell: { title: "恢复自带的 shell", line: "重新以自带的 shell 执行命令。" },
    guidance: { title: "移除使用说明", line: "原有内容不变。" },
    limits: { title: "还原输出上限", line: "恢复接入前的设置；此后你改动的值予以保留。" },
    plugin: { title: "卸载 Fastctx 插件", line: "经 Claude Code 自身的命令卸载。" },
    consent: { title: "撤回审批豁免", line: "Fastctx 发起的对话同样需逐项批准。" },
    legacy: { title: "清理旧版配置", line: "早期版本写入的配置一并移除。" },
  },
  raised: (current, requested) => `你设定的上限 ${current.toLocaleString("zh-CN")} 将调至 ${requested.toLocaleString("zh-CN")}；此上限由所有工具共用。`,
  never: { title: "边界", line: "Fastctx 不唤醒它，不代它向厂商发出请求，也不读取它的对话。" },
  same: "无需改动。",
  sessions: (agent, into) =>
    into
      ? agent === "claude"
        ? "已开启的 Claude Code 会话即刻停用自带的 Bash，重新打开后方可使用 run。"
        : "已开启的 Codex 会话需重新打开，方可使用 run。"
      : "已开启的会话需重新打开，方可恢复自带的工具。",
  takeBack: "你改动过的 Fastctx 配置将恢复为当前版本的写法，文件其余部分不变。",
  undo: "可随时撤回，届时仅移除上述改动。",
  undone: "仅移除 Fastctx 写入的内容，原有内容不变。",
  apply: "接入",
  take: "移除",
  lines: (added, removed) => [added ? `新增 ${added} 行` : "", removed ? `删除 ${removed} 行` : ""].filter(Boolean).join("，"),
  more: (n) => `另有 ${n.toLocaleString("zh-CN")} 行未显示`,
  moved: "文件已有改动，以下为重新计算的结果。",
};

export const enDisclose: DiscloseWords = {
  connect: (agent) => `Connect ${agent}`,
  remove: (agent) => `Remove ${agent}`,
  where: (machine, who) => `${machine}, ${who}`,
  what: "About connecting",
  about: (agent) => `You keep working in your own ${agent}; through Fastctx it reaches every one of your machines, and all it does is recorded in Activity.`,
  into: {
    tools: { title: "Adds the run tool", line: "Runs commands and reads and writes files on each of your machines, and moves files between them." },
    shell: { title: "Turns off its own shell", line: "Commands run through run instead; while Fastctx is not running, none can run." },
    guidance: { title: "Adds usage instructions", line: "They say when to use run; your existing text stays unchanged." },
    limits: { title: "Raises the output limit", line: "A single tool answer is no longer cut short, so run's results arrive in full." },
    plugin: { title: "Installs the Fastctx plugin", line: "Installed with Claude Code's own commands; conversations Fastctx starts exchange messages and are woken through it." },
    consent: { title: "Exempts step-by-step approval", line: "Only for conversations Fastctx starts; sessions you open still ask for approval at each step." },
    legacy: { title: "Takes over earlier settings", line: "Settings an earlier version wrote are replaced with this version's." },
  },
  out: {
    tools: { title: "Removes the run tool", line: "It can no longer reach your other machines through Fastctx." },
    shell: { title: "Restores its own shell", line: "Commands run in its own shell again." },
    guidance: { title: "Removes the usage instructions", line: "Your existing text stays unchanged." },
    limits: { title: "Restores the output limit", line: "Settings return to how they were before connecting; values you changed since are kept." },
    plugin: { title: "Uninstalls the Fastctx plugin", line: "Uninstalled with Claude Code's own commands." },
    consent: { title: "Withdraws the approval exemption", line: "Conversations Fastctx starts ask for approval at each step too." },
    legacy: { title: "Clears earlier settings", line: "Settings an earlier version wrote are removed as well." },
  },
  raised: (current, requested) => `Your limit of ${current.toLocaleString("en")} is raised to ${requested.toLocaleString("en")}; every tool shares this limit.`,
  never: { title: "Boundaries", line: "Fastctx does not wake it, sends no request to its vendor on its behalf, and does not read its conversations." },
  same: "No changes needed.",
  sessions: (agent, into) =>
    into
      ? agent === "claude"
        ? "Claude Code sessions already open lose their own Bash at once; reopen them to use run."
        : "Codex sessions already open must be reopened to use run."
      : "Sessions already open must be reopened to restore their own tools.",
  takeBack: "Fastctx settings you changed return to how this version writes them; the rest of each file stays unchanged.",
  undo: "This can be undone at any time; only the changes above are removed.",
  undone: "Only what Fastctx wrote is removed; your existing content stays unchanged.",
  apply: "Connect",
  take: "Remove",
  lines: (added, removed) => [added ? `${added} added` : "", removed ? `${removed} removed` : ""].filter(Boolean).join(", "),
  more: (n) => `${n.toLocaleString("en")} more lines not shown`,
  moved: "The files have changed; below is the change worked out again.",
};
