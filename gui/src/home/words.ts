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
  lastHere: (ago) => `你上次来是 ${ago}`,
  firstTime: "这是你第一次打开",
  machinesOnline: (online, all) => (online === all ? `${all} 台机器都在线` : `${all} 台机器里 ${online} 台在线`),
  agentsWorking: (n) => (n ? `${n} 个智能体在干活` : "没有智能体在干活"),
  waiting: "等你回答",
  waitingFor: "在等你回答",
  now: "此刻",
  nobodyWorking: "此刻没有谁在干活。",
  idle: (names) => `空闲：${names}`,
  listSeparator: "、",
  unreachable: (why) => `问不到这台机器上的世界：${why}`,
  oldWorld: (node, client) => `这台电脑上的世界还是旧版 ${node}，和这个界面（${client}）说不上话`,
  renewThere: "到设置里换成新版",
  setUp: "设置这台电脑",
  begin: "开一段新对话",
  away: "你不在时",
  nothingAway: "你不在时，世界里没有新的事。",
  projects: "项目",
  machines: "机器",
  unattended: "没人看着、还在跑的",
  someone: (agent, machine) => `${agent} · ${machine}`,
  colleagueOf: (asker) => `受 ${asker} 之托`,
  for: (duration) => `已 ${duration}`,
  ago: (ms) => `${zhSpan(ms)}前`,
  lasted: zhSpan,
  clock: (at) => `${pad(at.getHours())}:${pad(at.getMinutes())}`,
  day: (at) => `${zhNumber(at.getMonth() + 1)}月${zhNumber(at.getDate())}日 · ${zhWeek[at.getDay()]}`,
  online: "在线",
  offlineSince: (ago) => `${ago}离线`,
  unknownLink: "不知道在不在线",
  here: (n) => `${n} 位在场`,
  journals: (n) => `${n} 份手记`,
  jobsRunning: (n) => `${n} 个作业在跑`,
  residents: (n) => `${n} 个智能体`,
  turnEnded: (who) => `${who} 说完了`,
  turnFailed: (who, reason) => `${who} 这一轮失败了：${reason}`,
  jobDone: (job, machine) => `${job} 在 ${machine} 上跑完了`,
  journalAdded: (journal) => `手记「${journal}」多了一条`,
  asked: (who, colleague) => `${who} 请了同事 ${colleague}`,
  machineLeft: (machine) => `${machine} 离线了`,
  machineCame: (machine) => `${machine} 回来了`,
  arrived: (who) => `${who} 来了`,
  entered: (who, project) => `${who} 进了项目 ${project}`,
  leftProject: (who, project) => `${who} 离开了项目 ${project}`,
  movedTo: (who, from, to) => `${who} 从项目 ${from} 换到了 ${to}`,
  projectOpened: (name) => `开了项目「${name}」`,
  projectEdited: (name, version) => `「${name}」的立项写到了第 ${version} 版`,
  uncertain: (who) => `给 ${who} 的话，进没进去不知道`,
  undelivered: (colleague, to) => `${colleague} 的答案没能送到 ${to}`,
  you: "你",
  unasked: (list) => `没问到：${list}`,
  unaskedOffline: (machine) => `${machine}（离线）`,
  unaskedWhy: (machine, why) => `${machine}（${why}）`,
  awayTrouble: (why) => `问不到你不在时的事：${why}`,
  startedBy: (who) => `${who} 起的`,
  ownerLeft: (ago) => `它的主人 ${ago}走了`,
  forAccounts: (accounts) => `替 ${accounts} 跑的`,
  open: "打开",
};

const enWeek = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const enMonths = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

export const enHome: HomeWords = {
  world: "The World",
  aPage: (name) => `the page “${name}”`,
  dateline: (at) => `${enWeek[at.getDay()]}, ${enMonths[at.getMonth()]} ${at.getDate()}`,
  lastHere: (ago) => `You were last here ${ago}`,
  firstTime: "This is your first time here",
  machinesOnline: (online, all) => (online === all ? `all ${all} machines online` : `${online} of ${all} machines online`),
  agentsWorking: (n) => (n ? `${n} agent${n === 1 ? "" : "s"} working` : "no agent working"),
  waiting: "Waiting for you",
  waitingFor: "is waiting for your answer",
  now: "Now",
  nobodyWorking: "Nobody is working right now.",
  idle: (names) => `Idle: ${names}`,
  listSeparator: ", ",
  unreachable: (why) => `The World on this machine could not be asked: ${why}`,
  oldWorld: (node, client) => `This computer still runs the older World ${node}, which cannot talk with this interface (${client})`,
  renewThere: "Put the new one in place in Settings",
  setUp: "Set up this computer",
  begin: "Begin a conversation",
  away: "While you were away",
  nothingAway: "Nothing new happened in the World while you were away.",
  projects: "Projects",
  machines: "Machines",
  unattended: "Running with nobody watching",
  someone: (agent, machine) => `${agent} · ${machine}`,
  colleagueOf: (asker) => `for ${asker}`,
  for: (duration) => `for ${duration}`,
  ago: (ms) => `${enSpan(ms)} ago`,
  lasted: enSpan,
  clock: (at) => `${pad(at.getHours())}:${pad(at.getMinutes())}`,
  day: (at) => `${enWeek[at.getDay()]}, ${enMonths[at.getMonth()]} ${at.getDate()}`,
  online: "online",
  offlineSince: (ago) => `offline since ${ago}`,
  unknownLink: "not known whether online",
  here: (n) => `${n} here`,
  journals: (n) => `${n} journal${n === 1 ? "" : "s"}`,
  jobsRunning: (n) => `${n} job${n === 1 ? "" : "s"} running`,
  residents: (n) => `${n} agent${n === 1 ? "" : "s"}`,
  turnEnded: (who) => `${who} finished`,
  turnFailed: (who, reason) => `${who}'s turn failed: ${reason}`,
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
  projectEdited: (name, version) => `“${name}” is at version ${version} of its record`,
  uncertain: (who) => `Words for ${who}: whether they got in is not known`,
  undelivered: (colleague, to) => `${colleague}'s answer did not reach ${to}`,
  you: "You",
  unasked: (list) => `Not asked: ${list}`,
  unaskedOffline: (machine) => `${machine} (offline)`,
  unaskedWhy: (machine, why) => `${machine} (${why})`,
  awayTrouble: (why) => `What happened while you were away could not be asked: ${why}`,
  startedBy: (who) => `started by ${who}`,
  ownerLeft: (ago) => `its owner left ${ago}`,
  forAccounts: (accounts) => `runs for ${accounts}`,
  open: "Open",
};
