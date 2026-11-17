// What the features pages say (`world-gui.md`, 功能): a name and one line
// per feature, what is said under its film, the settings' labels.

import type { GlyphName } from "../conversation/glyphs";
import type { CollaborationWords } from "../film/collaboration";
import type { HistoryFilmWords } from "../film/history";
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
  /** History (`world-history.md`): a switch, and no settings. `unread`
   *  when the switch cannot be read; `hubOld` where the hub cannot keep it. */
  history: { name: string; line: string; film: HistoryFilmWords; gives: [GlyphName, string][]; after: string; offWhat: string; unread: string; hubOld: string };
  /** While the settings are being read. */
  reading: string;
  retry: string;
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
  atOnce: string;
  /** Under the pointer on the limit's name. */
  atOnceSaid: string;
  noLimit: string;
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
  /** On a computer in no network, where the switch would be: it asks
   *  colleagues only in one (`world-colleagues.md`). */
  alone: string;
  newNetwork: string;
  joinNetwork: string;
  unavailable: string;
  failed(why: string): string;
  noLogins: string;
  noCodex: string;
}

/** The sentinels' page: its film and its settings (`sentinel-form.ts`). */
export interface SentinelWords {
  enableSentinels: string;
  enableGoals: string;
  name: string;
  line: string;
  /** The page of every watcher, from here. */
  all: string;
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
  /** Trying what is saved, from the machine they live on. */
  tryIt: string;
  trying: string;
  tryWhat: string;
  trySaveFirst: string;
  searchName: string;
  worked: string;
  failed: string;
  seconds(ms: number): string;
  tryFailed: string;
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
  replay: "重播",
  closeFilm: "关闭",
  conversations: {
    name: "工作台",
    line: "Codex、Claude Code 的图形界面，支持其他电脑与手机",
    film: {
      captions: [
        "在其他界面中，智能体经 run 完成的工作只是一段文字",
        "工作台清楚呈现每一步：运行了什么、修改了哪些行、在哪台机器",
        "其他电脑上的智能体，在此打开，如同在本机",
        "离开电脑时，可在手机上继续查看与回复",
      ],
      thought: "Thought for 4s",
      ran: "运行",
      edited: "修改",
      approve: "待批准",
    },
    after: "首页、新对话、项目与对话已在侧栏",
    offWhat: "侧栏中的首页、新对话、项目与对话将收起；已有对话及其任务照常运行，数据在设置的「本机数据」中管理。",
  },
  collaboration: {
    name: "模型协作",
    line: "智能体将需求交给 GPT 完成",
    film: { captions: ["与智能体共同完成一件作品", "其中所需的一部分，交给 GPT", "GPT 在自己的机器上完成", "交回后并入整体"] },
    gives: [
      ["page", "附带现成的协作说明，指导智能体如何向\u00a0GPT\u00a0提出需求，可编辑"],
      ["colleague", "委派、过程中的消息与结果都在对话中，无需手动操作"],
    ],
    after: "所选调用方的对话中已加入请 GPT 的工具",
    offWhat: "智能体的对话中不再提供请 GPT 的工具；此处设置保留，重新开启后恢复。",
  },
  history: {
    name: "笔记",
    line: "项目与对话留给后来者的记录",
    film: { captions: ["对话将经过、决定与否决的事项记入笔记", "次日接手的模型，无论来自哪一家", "检索即可找到那一行，继续推进", "每台电脑上各有一份纯文本"] },
    gives: [
      ["journal", "每个项目一本，每段不属于项目的对话一本；由模型撰写，也可手动编辑"],
      ["search", "纯文本存于每台电脑，任何厂商的模型均可用\u00a0rg\u00a0读取"],
    ],
    after: "对话角落与项目页中已显示笔记；模型自下一轮起获取笔记列表",
    offWhat: "对话中的模型不再读取笔记，界面入口收起；已写的笔记全部保留，重新开启后恢复。",
    unread: "无法读取笔记开关",
    hubOld: "本网络的中枢暂不支持此开关，更新中枢后可用",
  },
  reading: "正在读取设置",
  retry: "重试",
  sentinels: {
    enableSentinels: "开启观测",
    enableGoals: "开启目标",
    name: "观测",
    line: "会话等待期间，由观测代为监视",
    all: "全部观测",
    film: { captions: ["会话需要等待时，留下一项观测", "会话休息，观测代为巡检该机器", "条件成立后，观测唤醒会话继续工作"] },
    after: "此账号下的会话均可留下观测",
    offWhat: "会话不能再创建观测，进行中的观测暂停；此处的设置与密钥保留，重新开启后继续。",
    home: "所在机器",
    homeStatus: { forbidden: "此账号无权使用该机器", offline: "该机器离线", too_old: "该机器的 Fastctx 版本过旧", unavailable: "该机器暂不可用" },
    model: "模型",
    protocols: { open_ai: "OpenAI 兼容", anthropic: "Anthropic 兼容" },
    endpoint: "地址",
    modelName: "模型名",
    key: "密钥",
    keySet: "已设置",
    keyMissing: "未设置",
    keyReplace: "更换",
    keyGoes: (machine) => `只交给 ${machine}`,
    search: "搜索（可选）",
    searchEndpoint: "Tavily 地址",
    save: "保存",
    notSaved: "未能保存",
    changedElsewhere: "已在别处修改，现已载入最新版本",
    tryIt: "测试",
    trying: "测试中",
    tryWhat: "由所在机器使用已保存的设置与密钥，向模型和搜索各发送一次请求",
    trySaveFirst: "请先保存",
    searchName: "搜索",
    worked: "可用",
    failed: "不可用",
    seconds: (ms) => `${(ms / 1000).toFixed(1)} 秒`,
    tryFailed: "测试失败",
  },
  callers: "调用方",
  callersSaid: "这些智能体的对话中将提供请 GPT 的工具",
  callees: "被调用方",
  addCallee: "添加被调用方",
  name: "名称",
  namePattern: "小写字母、数字或连字符",
  useWhen: "使用时机",
  login: "登录",
  model: "模型",
  modelDefault: "默认",
  effort: "推理强度",
  screen: "操作屏幕",
  screenSaid: "可操作该电脑的屏幕与浏览器",
  atOnce: "同时最多",
  atOnceSaid: "超过此数时，新的请求等待其中一项完成后开始",
  noLimit: "不限",
  removeCallee: "移除",
  prompt: "协作说明",
  promptReset: "恢复默认",
  rules: "给 GPT 的规则",
  rulesHint: "每次请求时附带，例如禁止事项、服务器的承载上限",
  now: "此刻",
  nobodyNow: "暂无进行中的协作",
  workingFor: (asker) => `为 ${asker} 工作`,
  save: "保存",
  saved: "已保存",
  openCodex: "已打开的 Codex 对话需重新连接后生效",
  noAccount: "登录账号后可用",
  alone: "加入网络后可用",
  newNetwork: "新建网络",
  joinNetwork: "加入网络",
  unavailable: "无法读取模型协作设置",
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
    line: "An agent asks a GPT for what it wants, and the GPT makes it",
    film: { captions: ["You and an agent build something together", "What it wants, it asks a GPT for", "The GPT makes it on its own machine", "It comes back and joins the whole"] },
    gives: [
      ["page", "A ready collaboration text teaches the agent how to wish for what it wants; you can edit it"],
      ["colleague", "Asking, what it says on the way and what comes back all happen in the conversation, with no steps for you"],
    ],
    after: "The callers you choose get the tools to ask a GPT in their conversations",
    offWhat: "Conversations no longer get the tools to ask a GPT; these settings stay for when you turn it on again.",
  },
  history: {
    name: "Notes",
    line: "What projects and conversations leave for whoever comes next",
    film: {
      captions: [
        "A conversation writes down what happened, what was decided and what was let go",
        "The next day a model wakes to the same work, from whichever vendor",
        "One search finds the line, and it carries on",
        "On every computer, a plain text file",
      ],
    },
    gives: [
      ["journal", "One for each project and one for each conversation outside a project; models write them, and so can you"],
      ["search", "Plain text on each of your computers, read with rg by any vendor's model"],
    ],
    after: "Conversations and projects show their notes; models get their list from their next turn",
    offWhat: "Models in conversations no longer get the notes, and their ways in fold away; every note written stays, as it is, for when you turn them on again.",
    unread: "Could not read the notes switch",
    hubOld: "This network's hub cannot keep this switch yet; update the hub to use it",
  },
  reading: "Reading the settings",
  retry: "Retry",
  sentinels: {
    enableSentinels: "Enable watchers",
    enableGoals: "Enable goals",
    name: "Watchers",
    line: "While a conversation waits, a watcher keeps an eye out",
    all: "All watchers",
    film: { captions: ["A conversation that must wait leaves a watcher", "It rests while the watcher checks that machine", "When the condition holds, the watcher wakes it to go on"] },
    after: "Conversations working for this account can leave watchers",
    offWhat: "Conversations can no longer leave watchers, and those under way pause; the settings and keys here are kept for next time.",
    home: "Lives on",
    homeStatus: { forbidden: "This account cannot use that machine", offline: "That machine is offline", too_old: "Fastctx there is too old", unavailable: "That machine cannot be used now" },
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
    tryIt: "Try it",
    trying: "Trying",
    tryWhat: "The machine they live on asks the model and the search once, with the saved settings and keys",
    trySaveFirst: "Save first",
    searchName: "Search",
    worked: "Works",
    failed: "Failed",
    seconds: (ms) => `${(ms / 1000).toFixed(1)} s`,
    tryFailed: "Could not try",
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
  atOnce: "At once",
  atOnceSaid: "Past this, a new colleague waits until one at work is done",
  noLimit: "No limit",
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
  alone: "Join a network to use it",
  newNetwork: "New network",
  joinNetwork: "Join a network",
  unavailable: "Could not read the collaboration settings",
  failed: () => "Could not save",
  noLogins: "No agents available",
  noCodex: "No Codex logins",
};
