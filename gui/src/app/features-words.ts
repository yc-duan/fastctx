// What the features pages say (`world-gui.md`, 功能): a name and one line
// per feature, the storyboard's captions, the settings' labels.

import type { GlyphName } from "../conversation/glyphs";

export interface FeaturesWords {
  title: string;
  on: string;
  off: string;
  coming: string;
  turnOn: string;
  turnOff: string;
  replay: string;
  conversations: { name: string; line: string; frames: [string, string, string, string]; after: string };
  collaboration: { name: string; line: string; frames: [string, string, string, string]; gives: [GlyphName, string][] };
  /** While the settings are being read. */
  reading: string;
  retry: string;
  memory: { name: string; line: string };
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
  /** "for Claude · desktop". */
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

export const zhFeatures: FeaturesWords = {
  title: "功能",
  on: "已开启",
  off: "未开启",
  coming: "开发中",
  turnOn: "开启",
  turnOff: "关闭",
  replay: "再看一遍",
  conversations: {
    name: "对话",
    line: "在界面里和智能体说话，不论它在哪台机器",
    frames: ["手机在你手里，智能体在家里那台电脑上", "你说一句，看它一个字一个字地写", "它要你拍板时，找到你的手机", "你走开了，回来一眼看到发生了什么"],
    after: "首页、新对话、项目与对话已在侧栏",
  },
  collaboration: {
    name: "模型协作",
    line: "智能体把一处要做透的局部交给 GPT",
    frames: ["你和智能体一起做一样东西", "一处要做透的局部，它交给 GPT", "GPT 在自己那台机器上把这一处做透", "做完交回，接进你们手里的整体"],
    gives: [
      ["page", "附一份现成的协作说明，教智能体怎样用自然的话把局部交出去，你可以改"],
      ["colleague", "交出去、交回来都在对话里，不用你操心步骤"],
    ],
  },
  reading: "正在读取设置",
  retry: "重试",
  memory: { name: "项目记忆", line: "手记、定论与地图" },
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
  replay: "Watch again",
  conversations: {
    name: "Conversations",
    line: "Talk with your agents in the app, wherever they run",
    frames: ["Your phone in your hand, the agent on the computer at home", "You say a line and watch it write back, word by word", "When it needs your call, it finds your phone", "You step away; one look on return shows what happened"],
    after: "Home, new conversations, projects and conversations are in the sidebar",
  },
  collaboration: {
    name: "Model collaboration",
    line: "An agent hands a part to be done thoroughly to a GPT",
    frames: ["You and an agent build something together", "A part that must be done thoroughly, it hands to a GPT", "The GPT does that part thoroughly on its own machine", "It hands back, and the part joins the whole"],
    gives: [
      ["page", "A ready collaboration text teaches the agent to hand a part over in plain words; you can edit it"],
      ["colleague", "Handing over and handing back happen in the conversation, with no steps for you"],
    ],
  },
  reading: "Reading the settings",
  retry: "Retry",
  memory: { name: "Project memory", line: "Journals, decisions and the map" },
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
