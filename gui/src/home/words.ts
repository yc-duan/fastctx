// What the World's front page says, in the user's words (`world-gui.md`,
// 用词): agents by who they are and where, projects and machines by name,
// times as people say them. Nothing here is a judgment; every line says a
// fact the World holds.

export interface HomeWords {
  world: string;
  /** An open page, as who started or wrote something. */
  aPage(name: string): string;
  /** The day, as a dateline: 十月四日 · 星期六. */
  dateline(at: Date): string;
  lastHere(ago: string): string;
  firstTime: string;
  machinesOnline(online: number, all: number): string;
  agentsWorking(n: number): string;
  waiting: string;
  waitingFor: string;
  now: string;
  nobodyWorking: string;
  idle(names: string): string;
  /** Between the names in a short list. */
  listSeparator: string;
  /** The World could not be asked, and why. */
  unreachable(why: string): string;
  /** The World here is older than the interface and refuses it. */
  oldWorld(node: string, client: string): string;
  /** Where the new one is put in place. */
  renewThere: string;
  /** The way to set this computer up, beside the World it could not ask. */
  setUp: string;
  /** Begins a conversation, where nobody is at work. */
  begin: string;
  away: string;
  nothingAway: string;
  projects: string;
  machines: string;
  unattended: string;
  /** "Claude · laptop": an agent by its program and machine. */
  someone(agent: string, machine: string): string;
  colleagueOf(asker: string): string;
  for(duration: string): string;
  ago(ms: number): string;
  lasted(ms: number): string;
  clock(at: Date): string;
  day(at: Date): string;
  online: string;
  offlineSince(ago: string): string;
  unknownLink: string;
  here(n: number): string;
  journals(n: number): string;
  jobsRunning(n: number): string;
  residents(n: number): string;
  turnEnded(who: string): string;
  turnFailed(who: string, reason: string): string;
  jobDone(job: string, machine: string): string;
  journalAdded(journal: string): string;
  asked(who: string, colleague: string): string;
  machineLeft(machine: string): string;
  machineCame(machine: string): string;
  arrived(who: string): string;
  entered(who: string, project: string): string;
  leftProject(who: string, project: string): string;
  movedTo(who: string, from: string, to: string): string;
  projectOpened(name: string): string;
  projectEdited(name: string, version: number): string;
  uncertain(who: string): string;
  undelivered(colleague: string, to: string): string;
  you: string;
  unasked(list: string): string;
  unaskedOffline(machine: string): string;
  unaskedWhy(machine: string, why: string): string;
  awayTrouble(why: string): string;
  startedBy(who: string): string;
  ownerLeft(ago: string): string;
  /** Said of a job or agent that works for other accounts than the reader's. */
  forAccounts(accounts: string): string;
  open: string;
}

const zhWeek = ["星期日", "星期一", "星期二", "星期三", "星期四", "星期五", "星期六"];
const zhDigits = ["〇", "一", "二", "三", "四", "五", "六", "七", "八", "九"];

/** 1 to 31 in Chinese numerals, as a date is written: 四、十、十二、二十一. */
function zhNumber(n: number): string {
  if (n < 10) return zhDigits[n];
  const tens = Math.floor(n / 10);
  const ones = n % 10;
  return `${tens === 1 ? "" : zhDigits[tens]}十${ones ? zhDigits[ones] : ""}`;
}

function zhSpan(ms: number): string {
  const minutes = Math.round(ms / 60_000);
  if (minutes < 1) return "不到一分钟";
  if (minutes < 60) return `${minutes} 分钟`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return minutes % 60 && hours < 6 ? `${hours} 小时 ${minutes % 60} 分` : `${hours} 小时`;
  return `${Math.floor(hours / 24)} 天`;
}

function enSpan(ms: number): string {
  const minutes = Math.round(ms / 60_000);
  if (minutes < 1) return "under a minute";
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return minutes % 60 && hours < 6 ? `${hours} h ${minutes % 60} min` : `${hours} h`;
  const days = Math.floor(hours / 24);
  return `${days} day${days === 1 ? "" : "s"}`;
}

const pad = (n: number) => String(n).padStart(2, "0");

export const zhHome: HomeWords = {
  world: "世界",
  aPage: (name) => `页面「${name}」`,
  dateline: (at) => `${zhNumber(at.getMonth() + 1)}月${zhNumber(at.getDate())}日 · ${zhWeek[at.getDay()]}`,
  lastHere: (ago) => `上次访问 ${ago}`,
  firstTime: "首次访问",
  machinesOnline: (online, all) => (online === all ? `${all} 台机器在线` : `${online}/${all} 台机器在线`),
  agentsWorking: (n) => (n ? `${n} 个智能体工作中` : "无智能体工作"),
  waiting: "等待回复",
  waitingFor: "等待你回复",
  now: "工作中",
  nobodyWorking: "暂无工作中的智能体",
  idle: (names) => `空闲：${names}`,
  listSeparator: "、",
  unreachable: () => "无法连接本机服务",
  oldWorld: (node, client) => `本机服务版本 ${node} 与界面 ${client} 不兼容`,
  renewThere: "前往设置更新",
  setUp: "设置这台电脑",
  begin: "新对话",
  away: "离开期间",
  nothingAway: "离开期间没有新动态",
  projects: "项目",
  machines: "机器",
  unattended: "无人值守的任务",
  someone: (agent, machine) => `${agent} · ${machine}`,
  colleagueOf: (asker) => `${asker} 的同事`,
  for: (duration) => duration,
  ago: (ms) => `${zhSpan(ms)}前`,
  lasted: zhSpan,
  clock: (at) => `${pad(at.getHours())}:${pad(at.getMinutes())}`,
  day: (at) => `${zhNumber(at.getMonth() + 1)}月${zhNumber(at.getDate())}日 · ${zhWeek[at.getDay()]}`,
  online: "在线",
  offlineSince: (ago) => `${ago}离线`,
  unknownLink: "状态未知",
  here: (n) => `${n} 位在线`,
  journals: (n) => `${n} 份手记`,
  jobsRunning: (n) => `${n} 个作业在跑`,
  residents: (n) => `${n} 个智能体`,
  turnEnded: (who) => `${who} 回复完成`,
  turnFailed: (who, reason) => `${who} 出错：${reason}`,
  jobDone: (job, machine) => `${job} 在 ${machine} 上完成`,
  journalAdded: (journal) => `手记「${journal}」新增一条`,
  asked: (who, colleague) => `${who} 请了同事 ${colleague}`,
  machineLeft: (machine) => `${machine} 离线`,
  machineCame: (machine) => `${machine} 上线`,
  arrived: (who) => `${who} 加入`,
  entered: (who, project) => `${who} 进入项目 ${project}`,
  leftProject: (who, project) => `${who} 离开项目 ${project}`,
  movedTo: (who, from, to) => `${who} 从 ${from} 移到 ${to}`,
  projectOpened: (name) => `新建项目「${name}」`,
  projectEdited: (name, version) => `「${name}」立项更新至第 ${version} 版`,
  uncertain: (who) => `发给 ${who} 的消息状态未知`,
  undelivered: (colleague, to) => `${colleague} 的回复未送达 ${to}`,
  you: "你",
  unasked: (list) => `未获取：${list}`,
  unaskedOffline: (machine) => `${machine}（离线）`,
  unaskedWhy: (machine, why) => `${machine}（${why}）`,
  awayTrouble: () => "无法获取离开期间的动态",
  startedBy: (who) => `由 ${who} 启动`,
  ownerLeft: (ago) => `启动者 ${ago}离开`,
  forAccounts: (accounts) => `属于 ${accounts}`,
  open: "打开",
};

const enWeek = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const enMonths = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

export const enHome: HomeWords = {
  world: "The World",
  aPage: (name) => `the page “${name}”`,
  dateline: (at) => `${enWeek[at.getDay()]}, ${enMonths[at.getMonth()]} ${at.getDate()}`,
  lastHere: (ago) => `Last visit ${ago}`,
  firstTime: "First visit",
  machinesOnline: (online, all) => (online === all ? `all ${all} machines online` : `${online} of ${all} machines online`),
  agentsWorking: (n) => (n ? `${n} agent${n === 1 ? "" : "s"} working` : "no agent working"),
  waiting: "Waiting for reply",
  waitingFor: "Waiting for your reply",
  now: "Working",
  nobodyWorking: "No agents working",
  idle: (names) => `Idle: ${names}`,
  listSeparator: ", ",
  unreachable: () => "Can't connect to the local service",
  oldWorld: (node, client) => `Local service ${node} isn't compatible with this app (${client})`,
  renewThere: "Update in Settings",
  setUp: "Set up this computer",
  begin: "New conversation",
  away: "While you were away",
  nothingAway: "Nothing new while you were away",
  projects: "Projects",
  machines: "Machines",
  unattended: "Unattended tasks",
  someone: (agent, machine) => `${agent} · ${machine}`,
  colleagueOf: (asker) => `colleague of ${asker}`,
  for: (duration) => duration,
  ago: (ms) => `${enSpan(ms)} ago`,
  lasted: enSpan,
  clock: (at) => `${pad(at.getHours())}:${pad(at.getMinutes())}`,
  day: (at) => `${enWeek[at.getDay()]}, ${enMonths[at.getMonth()]} ${at.getDate()}`,
  online: "online",
  offlineSince: (ago) => `offline since ${ago}`,
  unknownLink: "status unknown",
  here: (n) => `${n} here`,
  journals: (n) => `${n} journal${n === 1 ? "" : "s"}`,
  jobsRunning: (n) => `${n} job${n === 1 ? "" : "s"} running`,
  residents: (n) => `${n} agent${n === 1 ? "" : "s"}`,
  turnEnded: (who) => `${who} replied`,
  turnFailed: (who, reason) => `${who} hit an error: ${reason}`,
  jobDone: (job, machine) => `${job} ended on ${machine}`,
  journalAdded: (journal) => `A new entry in the journal “${journal}”`,
  asked: (who, colleague) => `${who} asked colleague ${colleague}`,
  machineLeft: (machine) => `${machine} went offline`,
  machineCame: (machine) => `${machine} came back`,
  arrived: (who) => `${who} arrived`,
  entered: (who, project) => `${who} entered ${project}`,
  leftProject: (who, project) => `${who} left ${project}`,
  movedTo: (who, from, to) => `${who} moved from ${from} to ${to}`,
  projectOpened: (name) => `Opened the project “${name}”`,
  projectEdited: (name, version) => `“${name}” updated to version ${version}`,
  uncertain: (who) => `Message to ${who}: delivery unknown`,
  undelivered: (colleague, to) => `${colleague}'s answer did not reach ${to}`,
  you: "You",
  unasked: (list) => `Not asked: ${list}`,
  unaskedOffline: (machine) => `${machine} (offline)`,
  unaskedWhy: (machine, why) => `${machine} (${why})`,
  awayTrouble: () => "Couldn't load activity while you were away",
  startedBy: (who) => `started by ${who}`,
  ownerLeft: (ago) => `starter left ${ago}`,
  forAccounts: (accounts) => `belongs to ${accounts}`,
  open: "Open",
};
