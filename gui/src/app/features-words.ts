// What the features pages say (`world-gui.md`, 功能): a name and one line
// per feature, the storyboard's captions, the settings' labels.

export interface FeaturesWords {
  title: string;
  on: string;
  off: string;
  coming: string;
  turnOn: string;
  turnOff: string;
  replay: string;
  collaboration: { name: string; line: string; frames: [string, string, string, string] };
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
  collaboration: {
    name: "模型协作",
    line: "对话里的智能体请一位 GPT 来钻一处难点",
    frames: ["你和智能体在一段对话里", "遇到一处难点，它请来一位 GPT", "GPT 在自己的机器上钻下去", "做完交回，对话里看得到"],
  },
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
  unavailable: "本机服务暂不支持",
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
  collaboration: {
    name: "Model collaboration",
    line: "An agent asks a GPT in to dig into one hard part",
    frames: ["You and an agent, in a conversation", "At a hard part, it asks a GPT in", "The GPT digs in on its own machine", "It hands back; the conversation shows it"],
  },
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
  unavailable: "Not supported by the local service yet",
  failed: () => "Couldn't save",
  noLogins: "No agents available",
  noCodex: "No Codex logins",
};
