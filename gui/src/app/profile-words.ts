// What the agents' sections and the account's own pages say
// (`world-user-config.md`; `world-gui.md`, 智能体页): the sections under
// 「智能体」 in the sidebar, the tabs of an agent's page, the prompt, the
// MCP servers and an agent's preferences, and the servers' switches in a
// conversation's 「机器与文件夹」.

export type AgentSection = "claude" | "codex" | "prompt" | "skills" | "mcp";

/** What an agent's page shows: its logins, or the account's preferences
 *  for it. */
export type AgentTab = "logins" | "prefs";

export interface ProfileWords {
  sections: Record<AgentSection, string>;
  tabs: Record<AgentTab, string>;
  /** The account a page is of, where the device is logged in to more than
   *  one. */
  account: string;
  /** The hub keeps no profiles: it runs an older Fastctx. */
  olderHub: string;
  reading: string;
  readFailed: string;
  saving: string;
  saved: string;
  saveFailed: string;
  /** Another device wrote it since it was read here. */
  changedElsewhere: string;
  /** Takes what was written elsewhere in place of what is here. */
  loadTheirs: string;
  /** When what is saved reaches the sessions, said on hover. */
  takesEffect: string;
  prompt: {
    placeholder: string;
    save: string;
    /** Said on hover over saving it. */
    reaches: string;
    /** How much of the most it holds, near the end. */
    size(used: string, most: string): string;
    tooLong: string;
  };
  mcp: {
    add: string;
    /** Adding one on a machine, from its group. */
    addOn(machine: string): string;
    import: string;
    /** The group of servers by address. */
    remote: string;
    here: string;
    command: string;
    missing: string;
    older: string;
    failed: string;
    /** Said on hover over a command server on a Windows machine. */
    sessionZero: string;
    /** Said on hover over a server by address, and where one is added. */
    remoteReaches: string;
    test: string;
    edit: string;
    /** Editing a server a command starts: filled in anew, as its machine
     *  keeps the command to itself. */
    refill: string;
    remove: string;
    more: string;
    testing: string;
    tools(count: number): string;
    noTools: string;
    didNotStart: string;
    log: string;
    removeTitle(name: string): string;
    removeCommand(machine: string): string;
    removeRemote: string;
    removeGo: string;
    removing: string;
    removeFailed: string;
  };
  add: {
    title: string;
    editTitle(name: string): string;
    paste: string;
    form: string;
    pastePlaceholder: string;
    notJson: string;
    noServers: string;
    shape: string;
    name: string;
    nameRule: string;
    reserved: string;
    taken: string;
    kind: string;
    kindCommand: string;
    kindRemote: string;
    command: string;
    args: string;
    env: string;
    envPlaceholder: string;
    cwd: string;
    url: string;
    urlRule: string;
    headers: string;
    headersPlaceholder: string;
    sse: string;
    on: string;
    /** Said on hover beside where a server a command starts runs. */
    onTip: string;
    noMachine: string;
    go(count: number): string;
    save: string;
    adding: string;
    failed: string;
  };
  import: {
    title: string;
    from: string;
    reading: string;
    none: string;
    had: string;
    claudeDesktop: string;
    claudeCode: string;
    cursor: string;
    go(count: number): string;
    importing: string;
    failed: string;
  };
  prefs: {
    standard: string;
    /** Said on hover over 「默认」: what is taken when nothing is set. */
    standardTip: string;
    on: string;
    off: string;
    modelSection: string;
    contextSection: string;
    otherSection: string;
    model: string;
    noModels: string;
    effort: string;
    window: string;
    windowTip(size: string): string;
    compact: string;
    compactTip(size: string): string;
    /** The step past the last share: no compacting by itself. */
    never: string;
    neverTip: string;
    language: string;
    languagePlaceholder: string;
    outputStyle: string;
    styles: { explanatory: string; learning: string };
    summaries: string;
    git: string;
    attribution: string;
    unsigned: string;
    reasoningSummary: string;
    summaryKinds: { auto: string; concise: string; detailed: string; none: string };
    verbosity: string;
    verbosityKinds: { low: string; medium: string; high: string };
    hideReasoning: string;
    rawReasoning: string;
  };
  subagents: {
    /** The page and its row among Claude Code's preferences, the list,
     *  and the tag beside it while it is the defaults. */
    agents: string;
    /** The subagents named on that row. */
    names(names: string[]): string;
    defaults: string;
    /** Said on hover over that tag. */
    defaultsTip: string;
    add: string;
    /** The account took every one of them out. */
    none: string;
    /** A model left to the conversation's. */
    inherit: string;
    /** How the main agent should use them, and its paper's words. */
    instructions: string;
    instructionsPlaceholder: string;
    save: string;
    reset: string;
    resetTitle: string;
    resetWhat: string;
    /** The hub keeps no subagents: it runs an earlier Fastctx. */
    olderHub: string;
    /** When a change reaches the conversations, said on hover. */
    takesEffect: string;
    newTitle: string;
    editTitle(name: string): string;
    name: string;
    nameRule: string;
    taken: string;
    description: string;
    descriptionNeeded: string;
    prompt: string;
    model: string;
    effort: string;
    more: string;
    tools: string;
    disallowed: string;
    toolsPlaceholder: string;
    maxTurns: string;
    maxTurnsRule: string;
    background: string;
    claudeMd: string;
    remove: string;
    removeTitle(name: string): string;
    removeWhat: string;
  };
  focus: {
    /** The group of the account's servers in 「机器与文件夹」. */
    title: string;
    /** Said on hover over it. */
    when: string;
  };
  /** The account's skills (`world-user-config.md`, 技能). */
  skills: {
    add: string;
    /** The account keeps none yet. */
    none: string;
    newTitle: string;
    editTitle(name: string): string;
    name: string;
    nameRule: string;
    taken: string;
    /** When it applies, in a sentence. */
    when: string;
    whenNeeded: string;
    instructions: string;
    instructionsNeeded: string;
    /** Fills the sheet from a SKILL.md file. */
    fromFile: string;
    fileUnread: string;
    save: string;
    remove: string;
    removeTitle(name: string): string;
    removeWhat: string;
    /** When a change reaches the conversations, said on hover. */
    takesEffect: string;
  };
}

export const zhProfile: ProfileWords = {
  sections: { claude: "Claude Code", codex: "Codex", prompt: "提示词", skills: "技能", mcp: "MCP" },
  tabs: { logins: "登录", prefs: "偏好" },
  account: "账号",
  olderHub: "这个网络的中枢服务器要先升级",
  reading: "正在读取",
  readFailed: "没读到",
  saving: "正在保存",
  saved: "已保存",
  saveFailed: "保存失败",
  changedElsewhere: "别处刚改过",
  loadTheirs: "载入那一版",
  takesEffect: "新开的对话马上带上；开着的对话下次被唤醒时换上",
  prompt: {
    placeholder: "写给替你干活的每一个智能体",
    save: "保存",
    reaches: "替你干活的每个智能体都会读到；正在干活的，下一次调用世界的工具时换上",
    size: (used, most) => `${used} / ${most}`,
    tooLong: "太长了",
  },
  mcp: {
    add: "添加",
    addOn: (machine) => `在 ${machine} 上添加`,
    import: "导入",
    remote: "网址",
    here: "本机",
    command: "命令",
    missing: "那台机器上没有",
    older: "那台机器要先升级",
    failed: "没问到",
    sessionZero: "这台 Windows 上它在系统的会话 0 里起：连本机端口的可用，要操作桌面窗口的不行",
    remoteReaches: "地址与请求头会交给替你干活的机器",
    test: "试一下",
    edit: "编辑",
    refill: "重新填写",
    remove: "移除",
    more: "更多",
    testing: "正在试",
    tools: (count) => `${count} 个工具`,
    noTools: "没有工具",
    didNotStart: "没起来",
    log: "日志",
    removeTitle: (name) => `移除 ${name}？`,
    removeCommand: (machine) => `它的命令与环境变量从 ${machine} 上删掉；对话下次被唤醒时不再带它。`,
    removeRemote: "对话下次被唤醒时不再带它。",
    removeGo: "移除",
    removing: "正在移除",
    removeFailed: "没能移除",
  },
  add: {
    title: "添加 MCP",
    editTitle: (name) => `编辑 ${name}`,
    paste: "粘贴",
    form: "填写",
    pastePlaceholder: '{ "mcpServers": { … } }',
    notJson: "不是 JSON",
    noServers: "里面没有 MCP",
    shape: "认不出这段 JSON",
    name: "名称",
    nameRule: "只用英文字母、数字、_ 和 -，至多 32 个",
    reserved: "fastctx 留给世界自己的工具",
    taken: "已经有这个名字",
    kind: "类型",
    kindCommand: "命令",
    kindRemote: "网址",
    command: "命令",
    args: "参数（可选）",
    env: "环境变量（可选）",
    envPlaceholder: "KEY=value，一行一个",
    cwd: "工作目录（可选）",
    url: "地址",
    urlRule: "以 https:// 或 http:// 开头",
    headers: "请求头（可选）",
    headersPlaceholder: "Name: value，一行一个",
    sse: "旧式 SSE",
    on: "运行在",
    onTip: "这条命令在哪台电脑上起。Unity、Blender 这类要连本机软件的，选开着那个软件的电脑；别的机器上的对话经 World 用它。",
    noMachine: "没有能放命令的机器在线",
    go: (count) => (count > 1 ? `添加 ${count} 个` : "添加"),
    save: "保存",
    adding: "正在添加",
    failed: "没能添加",
  },
  import: {
    title: "导入",
    from: "从哪台导入",
    reading: "正在读",
    none: "这台上没找到",
    had: "已有",
    claudeDesktop: "Claude Desktop",
    claudeCode: "Claude Code",
    cursor: "Cursor",
    go: (count) => (count ? `导入 ${count} 个` : "导入"),
    importing: "正在导入",
    failed: "没能导入",
  },
  prefs: {
    standard: "默认",
    standardTip: "用这个登录自己的设置",
    on: "开",
    off: "关",
    modelSection: "模型",
    contextSection: "上下文",
    otherSection: "其他",
    model: "默认模型",
    noModels: "还没有哪台电脑说过它有哪些模型",
    effort: "推理强度",
    window: "上下文长度",
    windowTip: (size) => `对话长到 ${size} token 为止`,
    compact: "压缩点",
    compactTip: (size) => `用到 ${size} token 时压缩`,
    never: "不压缩",
    neverTip: "不自动压缩，满了再说",
    language: "回答的语言",
    languagePlaceholder: "默认",
    outputStyle: "输出风格",
    styles: { explanatory: "解释型", learning: "学习型" },
    summaries: "思考摘要",
    git: "Git 内置指引",
    attribution: "提交与 PR 署名",
    unsigned: "不署名",
    reasoningSummary: "推理摘要",
    summaryKinds: { auto: "自动", concise: "简短", detailed: "详细", none: "不要" },
    verbosity: "回答详略",
    verbosityKinds: { low: "简", medium: "中", high: "详" },
    hideReasoning: "隐藏推理过程",
    rawReasoning: "原始推理内容",
  },
  subagents: {
    agents: "子代理",
    names: (names) => names.join("、"),
    defaults: "默认",
    defaultsTip: "Fastctx 自带的一套，随版本更新；改动任何一处，就成了你自己的",
    add: "添加",
    none: "还没有子代理",
    inherit: "跟随对话",
    instructions: "须知",
    instructionsPlaceholder: "写给主代理：什么时候派子代理，怎样分工",
    save: "保存",
    reset: "恢复默认",
    resetTitle: "恢复默认？",
    resetWhat: "子代理和须知换回 Fastctx 自带的那一套，你改过的不再保留。",
    olderHub: "这个网络的中枢服务器要先升级，才能设置子代理",
    takesEffect: "从下一轮起生效；已经在跑的子代理照旧",
    newTitle: "新的子代理",
    editTitle: (name) => `编辑 ${name}`,
    name: "名字",
    nameRule: "只用英文字母、数字、下划线和连字符，最多 64 个，不以连字符开头",
    taken: "已经有叫这个名字的了",
    description: "用途",
    descriptionNeeded: "写一句什么时候派它",
    prompt: "提示词",
    model: "模型",
    effort: "强度",
    more: "更多",
    tools: "只用这些工具（可选）",
    disallowed: "不用这些工具（可选）",
    toolsPlaceholder: "Read, Grep, Bash",
    maxTurns: "最多轮数（可选）",
    maxTurnsRule: "填一个正整数",
    background: "在后台跑",
    claudeMd: "读 CLAUDE.md",
    remove: "删除",
    removeTitle: (name) => `删除 ${name}？`,
    removeWhat: "之后的对话不再派它，已经在跑的照旧跑完。",
  },
  focus: { title: "MCP", when: "开关在这段对话下次被唤醒时生效" },
  skills: {
    add: "新建",
    none: "还没有技能",
    newTitle: "新技能",
    editTitle: (name) => `编辑「${name}」`,
    name: "名称",
    nameRule: "小写字母开头，可含数字和连字符，至多 64 个字符",
    taken: "已有同名的技能",
    when: "什么时候用",
    whenNeeded: "写一句什么时候用它，一行之内",
    instructions: "内容",
    instructionsNeeded: "写下它的内容",
    fromFile: "从文件读入",
    fileUnread: "这个文件读不出来",
    save: "保存",
    remove: "删除",
    removeTitle: (name) => `删除「${name}」？`,
    removeWhat: "替这个账号干活的对话不再看到它。",
    takesEffect: "替这个账号干活的对话随即看到改动",
  },
};

export const enProfile: ProfileWords = {
  sections: { claude: "Claude Code", codex: "Codex", prompt: "Prompt", skills: "Skills", mcp: "MCP" },
  tabs: { logins: "Logins", prefs: "Preferences" },
  account: "Account",
  olderHub: "This network's server needs updating first",
  reading: "Reading",
  readFailed: "Couldn't read",
  saving: "Saving",
  saved: "Saved",
  saveFailed: "Couldn't save",
  changedElsewhere: "Changed elsewhere",
  loadTheirs: "Load that version",
  takesEffect: "New conversations take it at once; open ones at their next wake",
  prompt: {
    placeholder: "To every agent that works for you",
    save: "Save",
    reaches: "Every agent working for you reads it; one at work takes it at its next call to the World's tools",
    size: (used, most) => `${used} / ${most}`,
    tooLong: "Too long",
  },
  mcp: {
    add: "Add",
    addOn: (machine) => `Add on ${machine}`,
    import: "Import",
    remote: "By address",
    here: "This computer",
    command: "Command",
    missing: "Not on that machine",
    older: "That machine needs updating",
    failed: "No answer",
    sessionZero: "On this Windows machine it starts in the system's session 0: servers that reach a local port work, those that drive desktop windows do not",
    remoteReaches: "The address and headers go to the machines that work for you",
    test: "Try it",
    edit: "Edit",
    refill: "Re-enter",
    remove: "Remove",
    more: "More",
    testing: "Trying",
    tools: (count) => (count === 1 ? "1 tool" : `${count} tools`),
    noTools: "No tools",
    didNotStart: "Didn't start",
    log: "Log",
    removeTitle: (name) => `Remove ${name}?`,
    removeCommand: (machine) => `Its command and environment are deleted from ${machine}; conversations go without it from their next wake.`,
    removeRemote: "Conversations go without it from their next wake.",
    removeGo: "Remove",
    removing: "Removing",
    removeFailed: "Couldn't remove",
  },
  add: {
    title: "Add an MCP server",
    editTitle: (name) => `Edit ${name}`,
    paste: "Paste",
    form: "Fill in",
    pastePlaceholder: '{ "mcpServers": { … } }',
    notJson: "Not JSON",
    noServers: "No servers in it",
    shape: "Not an mcpServers object",
    name: "Name",
    nameRule: "Letters, digits, _ and -, at most 32",
    reserved: "fastctx is the World's own tools",
    taken: "That name is taken",
    kind: "Kind",
    kindCommand: "Command",
    kindRemote: "Address",
    command: "Command",
    args: "Arguments (optional)",
    env: "Environment (optional)",
    envPlaceholder: "KEY=value, one per line",
    cwd: "Working folder (optional)",
    url: "Address",
    urlRule: "Starts with https:// or http://",
    headers: "Headers (optional)",
    headersPlaceholder: "Name: value, one per line",
    sse: "Older SSE",
    on: "Runs on",
    onTip: "The computer this command starts on. One that drives local software, such as Unity or Blender, runs where that software is open; conversations on other machines reach it through the World.",
    noMachine: "No machine to run it on is online",
    go: (count) => (count > 1 ? `Add ${count}` : "Add"),
    save: "Save",
    adding: "Adding",
    failed: "Couldn't add",
  },
  import: {
    title: "Import",
    from: "Import from",
    reading: "Reading",
    none: "None found here",
    had: "Added",
    claudeDesktop: "Claude Desktop",
    claudeCode: "Claude Code",
    cursor: "Cursor",
    go: (count) => (count ? `Import ${count}` : "Import"),
    importing: "Importing",
    failed: "Couldn't import",
  },
  prefs: {
    standard: "Default",
    standardTip: "Use this login's own setting",
    on: "On",
    off: "Off",
    modelSection: "Model",
    contextSection: "Context",
    otherSection: "Other",
    model: "Default model",
    noModels: "No computer has listed its models yet",
    effort: "Reasoning effort",
    window: "Context length",
    windowTip: (size) => `A conversation grows to ${size} tokens`,
    compact: "Compaction point",
    compactTip: (size) => `Compacts at ${size} tokens`,
    never: "Never",
    neverTip: "Doesn't compact by itself",
    language: "Language",
    languagePlaceholder: "Default",
    outputStyle: "Output style",
    styles: { explanatory: "Explanatory", learning: "Learning" },
    summaries: "Thinking summaries",
    git: "Git instructions",
    attribution: "Attribution",
    unsigned: "None",
    reasoningSummary: "Reasoning summary",
    summaryKinds: { auto: "Auto", concise: "Concise", detailed: "Detailed", none: "None" },
    verbosity: "Verbosity",
    verbosityKinds: { low: "Low", medium: "Medium", high: "High" },
    hideReasoning: "Hide reasoning",
    rawReasoning: "Raw reasoning",
  },
  subagents: {
    agents: "Subagents",
    names: (names) => names.join(", "),
    defaults: "Default",
    defaultsTip: "The set Fastctx comes with, updated with it; change any of it and it becomes your own",
    add: "Add",
    none: "No subagents yet",
    inherit: "As the conversation",
    instructions: "Instructions",
    instructionsPlaceholder: "For the main agent: when to hand work to subagents, and how to split it",
    save: "Save",
    reset: "Restore defaults",
    resetTitle: "Restore the defaults?",
    resetWhat: "Your subagents and instructions are replaced by the set Fastctx comes with.",
    olderHub: "This network's hub server needs an update before subagents can be set",
    takesEffect: "Takes effect from the next turn; subagents already running carry on",
    newTitle: "New subagent",
    editTitle: (name) => `Edit ${name}`,
    name: "Name",
    nameRule: "Letters, digits, underscores and hyphens, up to 64, not starting with a hyphen",
    taken: "There's already one by that name",
    description: "Use",
    descriptionNeeded: "Say when to hand it work",
    prompt: "Prompt",
    model: "Model",
    effort: "Effort",
    more: "More",
    tools: "Only these tools (optional)",
    disallowed: "Not these tools (optional)",
    toolsPlaceholder: "Read, Grep, Bash",
    maxTurns: "Most turns (optional)",
    maxTurnsRule: "A whole number above zero",
    background: "Runs in the background",
    claudeMd: "Reads CLAUDE.md",
    remove: "Delete",
    removeTitle: (name) => `Delete ${name}?`,
    removeWhat: "Conversations stop handing it work; one already running finishes.",
  },
  focus: { title: "MCP", when: "Switches take effect at this conversation's next wake" },
  skills: {
    add: "New",
    none: "No skills yet",
    newTitle: "New skill",
    editTitle: (name) => `Edit “${name}”`,
    name: "Name",
    nameRule: "Starts with a lowercase letter; digits and hyphens allowed; at most 64 characters",
    taken: "There's a skill with this name already",
    when: "When to use",
    whenNeeded: "Say in one line when to use it",
    instructions: "Instructions",
    instructionsNeeded: "Write its instructions",
    fromFile: "Read from a file",
    fileUnread: "This file couldn't be read",
    save: "Save",
    remove: "Delete",
    removeTitle: (name) => `Delete “${name}”?`,
    removeWhat: "Conversations working for this account no longer see it.",
    takesEffect: "Conversations working for this account see the change at once",
  },
};
