// What the features pages say (`world-gui.md`, 功能): a name and one line
// per feature, what is said under its film, the settings' labels.

import type { GlyphName } from "../conversation/glyphs";
import type { CollaborationWords } from "../film/collaboration";
import type { MemoryFilmWords } from "../film/memory";
import type { SentinelFilmWords } from "../film/sentinel";
import type { WorkbenchWords } from "../film/workbench";

export interface FeaturesWords {
  title: string;
  on: string;
  off: string;
  coming: string;
  turnOn: string;
  turnOff: string;
  /** The light way off at the foot of a feature's page: "关闭工作台". */
  turnOffNamed(name: string): string;
  /** The question asked before it is. */
  offTitle(name: string): string;
  /** The page shown once a feature is turned on. */
  enabled(name: string): string;
  back: string;
  replay: string;
  /** Closes the film watched over the page. */
  closeFilm: string;
  /** The workbench: talking with the agents in the interface. */
  /** `after` is what the interface gained with it; `offWhat`, what folds
   *  away and what carries on once it is off. */
  conversations: { name: string; line: string; film: WorkbenchWords; after: string; offWhat: string };
  collaboration: { name: string; line: string; film: CollaborationWords; gives: [GlyphName, string][]; after: string; offWhat: string };
  /** While the settings are being read. */
  reading: string;
  retry: string;
  memory: { name: string; line: string; film: MemoryFilmWords; after: string; offWhat: string };
  sentinels: SentinelWords;
  callers: string;
  callersSaid: string;
  callees: string;
  addCallee: string;
  name: string;
  namePattern: string;
  useWhen: string;
  login: string;
  model: string;
  modelDefault: string;
  effort: string;
  screen: string;
  screenSaid: string;
  removeCallee: string;
  prompt: string;
  promptReset: string;
  rules: string;
  rulesHint: string;
  now: string;
  nobodyNow: string;
  /** "for Claude desktop". */
  workingFor(asker: string): string;
  save: string;
  saved: string;
  openCodex: string;
  noAccount: string;
  unavailable: string;
  failed(why: string): string;
  noLogins: string;
  noCodex: string;
}

/** The sentinels' page: its film and its settings (`sentinel-form.ts`). */
export interface SentinelWords {
  name: string;
  line: string;
  film: SentinelFilmWords;
  after: string;
  offWhat: string;
  home: string;
  homeStatus: { forbidden: string; offline: string; too_old: string; unavailable: string };
  model: string;
  protocols: { open_ai: string; anthropic: string };
  endpoint: string;
  modelName: string;
  key: string;
  keySet: string;
  keyMissing: string;
  keyReplace: string;
  /** Said when a key's field is pointed at: where the key goes. */
  keyGoes(machine: string): string;
  search: string;
  searchEndpoint: string;
  save: string;
  notSaved: string;
  changedElsewhere: string;
}

export const zhFeatures: FeaturesWords = {
  title: "功能",
  on: "已开启",
  off: "未开启",
  coming: "开发中",
  turnOn: "开启",
  turnOff: "关闭",
  turnOffNamed: (name) => `关闭${name}`,
  offTitle: (name) => `关闭${name}？`,
  enabled: (name) => `${name}已开启`,
  back: "返回",
  replay: "再看一遍",
  closeFilm: "关闭",
  conversations: {
    name: "工作台",
    line: "Codex、Claude Code 的图形界面，别的电脑与手机上也能用",
    film: {
      captions: [
        "别的界面里，智能体用 run 干的活只是一团文字",
        "工作台把每一步画清楚：跑了什么、改了哪几行、在哪台机器",
        "别的电脑上的智能体，在这里打开，像在本机一样",
        "走开了，手机上接着看、接着回它",
      ],
      thought: "Thought for 4s",
      ran: "运行",
      edited: "修改",
      approve: "等你批准",
    },
    after: "首页、新对话、项目与对话已在侧栏",
    offWhat: "侧栏里的首页、新对话、项目与对话收起来；已有的对话和它们起的活照常，数据在设置的「本机数据」里管。",
  },
  collaboration: {
    name: "模型协作",
    line: "智能体把一处要做透的局部交给 GPT",
    film: { captions: ["你和智能体一起做一样东西", "一处要做透的局部，它交给 GPT", "GPT 在自己那台机器上把这一处做透", "做完交回，接进你们手里的整体"] },
    gives: [
      ["page", "附一份现成的协作说明，教智能体怎样用自然的话把局部交出去，你可以改"],
      ["colleague", "交出去、交回来都在对话里，不用你操心步骤"],
    ],
    after: "选中的调用方，对话里有了请 GPT 的工具",
    offWhat: "智能体的对话里不再有请 GPT 的工具；这里的设置留着，再开时照旧。",
  },
  reading: "正在读取设置",
  retry: "重试",
  sentinels: {
    name: "哨兵",
    line: "要等一件事时，留个哨兵替会话盯着",
    film: { captions: ["会话要等一件事，留下一个哨兵", "会话歇着，哨兵替它盯着那台机器", "那件事一成，哨兵叫醒会话接着干"] },
    after: "替这个账号干活的会话都能留哨兵了",
    offWhat: "会话不再能留哨兵，在盯的都撤掉；这里的设置与钥匙留着，再开时照旧。",
    home: "住在哪台",
    homeStatus: { forbidden: "这个账号用不了这台", offline: "这台不在线", too_old: "这台的 Fastctx 太旧", unavailable: "这台现在用不了" },
    model: "模型",
    protocols: { open_ai: "OpenAI 兼容", anthropic: "Anthropic 兼容" },
    endpoint: "地址",
    modelName: "模型名",
    key: "钥匙",
    keySet: "已设好",
    keyMissing: "还没设",
    keyReplace: "换一把",
    keyGoes: (machine) => `只交给 ${machine}`,
    search: "搜索（可选）",
    searchEndpoint: "Tavily 地址",
    save: "保存",
    notSaved: "没存上",
    changedElsewhere: "别处刚改过，已载入那一版",
  },
  memory: {
    name: "项目记忆",
    line: "项目里一路写下的手记与定下的定论",
    film: { captions: ["对话里要紧的，写成一篇手记", "过些天，另一段对话一开头就读到它", "定下的规矩是定论，每改一次多一版"] },
    after: "项目页里有了手记与定论，项目里的对话一开头就读到它们",
    offWhat: "项目页里收起手记与定论，对话不再拿到它们；已经写下的都留着，再开时照旧。",
  },
  callers: "调用方",
  callersSaid: "这些智能体的对话里会有请 GPT 的工具",
  callees: "被调用方",
  addCallee: "添加被调用方",
  name: "名字",
  namePattern: "小写字母、数字或连字符",
  useWhen: "什么时候用它",
  login: "登录",
  model: "模型",
  modelDefault: "默认",
  effort: "推理强度",
  screen: "操作屏幕",
  screenSaid: "能操作那台电脑的屏幕与浏览器",
  removeCallee: "移除",
  prompt: "协作说明",
  promptReset: "恢复默认",
  rules: "交给 GPT 的规矩",
  rulesHint: "每次请来时附上，比如红线、某台服务器能承受多少",
  now: "此刻",
  nobodyNow: "没有在干活的",
  workingFor: (asker) => `替 ${asker}`,
  save: "保存",
  saved: "已保存",
  openCodex: "已开着的 Codex 对话要重新连上才看到改动",
  noAccount: "登录账号后可用",
  unavailable: "读不到模型协作的设置",
  failed: () => "保存失败",
  noLogins: "暂无可用的智能体",
  noCodex: "暂无 Codex 登录",
};

export const enFeatures: FeaturesWords = {
  title: "Features",
  on: "On",
  off: "Off",
  coming: "In development",
  turnOn: "Turn on",
  turnOff: "Turn off",
  turnOffNamed: (name) => `Turn off ${name}`,
  offTitle: (name) => `Turn off ${name}?`,
  enabled: (name) => `${name} is on`,
  back: "Back",
  replay: "Watch again",
  closeFilm: "Close",
  conversations: {
    name: "Workbench",
    line: "A clear interface for Codex and Claude Code, from any computer or your phone",
    film: {
      captions: [
        "Elsewhere, an agent's work through run is a wall of text",
        "The workbench draws each step: what ran, which lines changed, on which machine",
        "An agent on another computer opens here, as if on this one",
        "Step away, and keep watching and answering it on your phone",
      ],
      thought: "Thought for 4s",
      ran: "Ran",
      edited: "Edited",
      approve: "Needs your OK",
    },
    after: "Home, new conversations, projects and conversations are in the sidebar",
    offWhat: "Home, new conversations, projects and conversations leave the sidebar; existing conversations and what they started carry on, and their data stays under Data in Settings.",
  },
  collaboration: {
    name: "Model collaboration",
    line: "An agent hands a part to be done thoroughly to a GPT",
    film: { captions: ["You and an agent build something together", "A part that must be done thoroughly, it hands to a GPT", "The GPT does that part thoroughly on its own machine", "It hands back, and the part joins the whole"] },
    gives: [
      ["page", "A ready collaboration text teaches the agent to hand a part over in plain words; you can edit it"],
      ["colleague", "Handing over and handing back happen in the conversation, with no steps for you"],
    ],
    after: "The callers you choose get the tools to ask a GPT in their conversations",
    offWhat: "Conversations no longer get the tools to ask a GPT; these settings stay for when you turn it on again.",
  },
  reading: "Reading the settings",
  retry: "Retry",
  sentinels: {
    name: "Sentinels",
    line: "When a conversation must wait, a sentinel watches for it",
    film: { captions: ["A conversation that has to wait leaves a sentinel", "It rests while the sentinel watches that machine", "When the thing happens, the sentinel wakes it to go on"] },
    after: "Conversations working for this account can leave sentinels",
    offWhat: "Conversations can no longer leave sentinels, and those watching are taken down; the settings and keys here stay for next time.",
    home: "Lives on",
    homeStatus: { forbidden: "This account can't use that machine", offline: "That machine is offline", too_old: "Fastctx there is too old", unavailable: "That machine can't be used now" },
    model: "Model",
    protocols: { open_ai: "OpenAI compatible", anthropic: "Anthropic compatible" },
    endpoint: "Address",
    modelName: "Model name",
    key: "Key",
    keySet: "Set",
    keyMissing: "Not set",
    keyReplace: "Replace it",
    keyGoes: (machine) => `Given to ${machine} only`,
    search: "Search (optional)",
    searchEndpoint: "Tavily address",
    save: "Save",
    notSaved: "Not saved",
    changedElsewhere: "Changed elsewhere; that version is loaded",
  },
  memory: {
    name: "Project memory",
    line: "A project's journal and its decisions",
    film: { captions: ["What mattered in a conversation is written as a journal", "Days later, another conversation reads it from its first line", "What was settled is a decision, one more version each change"] },
    after: "Projects show their journal and decisions, and conversations in them start with both",
    offWhat: "Projects fold their journal and decisions away and conversations no longer get them; all that was written stays, as it was, for next time.",
  },
  callers: "Callers",
  callersSaid: "Conversations with these agents get the tools to ask a GPT",
  callees: "Callees",
  addCallee: "Add callee",
  name: "Name",
  namePattern: "Lowercase letters, digits or hyphens",
  useWhen: "When to use it",
  login: "Login",
  model: "Model",
  modelDefault: "Default",
  effort: "Reasoning",
  screen: "Use the screen",
  screenSaid: "May operate that computer's screen and browser",
  removeCallee: "Remove",
  prompt: "Collaboration text",
  promptReset: "Restore default",
  rules: "Rules for GPT",
  rulesHint: "Given every time one is asked in: red lines, what a server can take",
  now: "Now",
  nobodyNow: "Nobody at work",
  workingFor: (asker) => `for ${asker}`,
  save: "Save",
  saved: "Saved",
  openCodex: "Codex conversations already open see changes once reconnected",
  noAccount: "Log in to an account to use it",
  unavailable: "Couldn't read the collaboration settings",
  failed: () => "Couldn't save",
  noLogins: "No agents available",
  noCodex: "No Codex logins",
};
