// What the page of the user's own things says (`world-gui.md`, 账号、机器、
// 共享与协助: 「我」).

import { enCodes, zhCodes, type CodeWords } from "./code-rows";
import type { MemberKind } from "../world/accounts";
import type { PasswordProblem } from "./password";
import { enPasswordFloor, enPasswordRule, enStrength, zhPasswordFloor, zhPasswordRule, zhStrength } from "./account-words";
import { enProblems, failure, zhProblems } from "./problem";

export interface MeWords {
  title: string;
  /** The page's tabs: what of the user's others use, their devices, and
   *  the codes that let devices in. */
  tabs: { given: string; devices: string; codes: string };
  level: Record<"super" | "member" | "visitor", string>;
  /** On a member's page: who the administrator is, and what they may do. */
  adminIs(admin: string): string;
  superNote(admin: string): string;
  /** On a visitor's page (`world-network.md`, 一个中枢，几个账号): what being
   *  one means behind a pointer's rest, and why nothing of theirs is given;
   *  the card that asks them to become a member. */
  visitorNote: string;
  visitorGives: string;
  promotionTitle(admin: string): string;
  promotionWhat: string;
  accept: string;
  decline: string;
  accepted: string;
  declined: string;
  /** The codes for managed machines the account made that still let them
   *  in (`world-network.md`, 受管机器). */
  codes: string;
  /** The codes' rows, as the sheet that adds a device has them too. */
  codeRows: CodeWords;
  /** Managed machines that came in by one code, told together. */
  arrivedByCode(name: string, count: number, at: string): string;
  someCode: string;
  notMeCodeTitle(name: string, count: number): string;
  notMeCodeWhat(count: number): string;
  notMeCodeGo: string;
  /** Between the items of a list. */
  separator: string;
  changePassword: string;
  passwordTitle: string;
  passwordWhat: string;
  password: string;
  passwordAgain: string;
  passwordsDiffer: string;
  passwordRule: Record<PasswordProblem, string>;
  passwordFloor: string;
  strength: [string, string, string];
  passwordGo: string;
  passwordDone: string;
  /** Who came into the account lately. */
  arrived(kind: string, name: string, at: string): string;
  me: string;
  notMe: string;
  notMeWhat(name: string): string;
  kinds: Record<MemberKind, string>;
  forms: Record<"desktop" | "laptop" | "server", string>;
  removeTitle(name: string): string;
  removeGo: string;
  /** What of the user's others may use: machines shared, logins assisting. */
  given: string;
  sharedTo: string;
  assisting: string;
  /** What an account may spend of a login in a week and a day, short. */
  weeklyShort(amount: string): string;
  dailyShort(amount: string): string;
  /** Until when something given lasts. */
  untilShort(at: string): string;
  takeBack(what: string): string;
  tookBack(what: string): string;
  sharedLabel(machine: string, account: string): string;
  assistLabel(login: string, account: string): string;
  /** What an account was given, under its name. */
  givenSub(machines: number, logins: number): string;
  /** A group of machines given together, in a sentence. */
  groupOf(group: string, count: number): string;
  /** Others' machines whose agents may reach the user's. */
  reaching: string;
  reachingWhat: string;
  reachingLine(machine: string, owner: string | null): string;
  reachingSince(at: string): string;
  stopReaching: string;
  stopReachingTitle(machine: string): string;
  stopReachingWhat(machine: string): string;
  /** The phones, and any web AI, logged in to the account. */
  devices(web: boolean): string;
  thisOne: string;
  since(at: string): string;
  onlineNow: string;
  lastSeen(at: string): string;
  neverSeen: string;
  removeDevice: string;
  removeDeviceWhat(name: string): string;
  /** A computer leaves the network; a phone logs out. */
  leave: string;
  leaveTitle(network: string): string;
  leaveWhat(network: string, registered: boolean): string;
  leaveGo: string;
  logout: string;
  logoutTitle(network: string): string;
  logoutWhat(network: string): string;
  logoutGo: string;
  none: string;
  logIn: string;
  undo: string;
  working: string;
  close: string;
  failed(why: string): string;
}

export const zhMe: MeWords = {
  title: "我",
  tabs: { given: "共享", devices: "设备", codes: "入网码" },
  level: { super: "管理员", member: "成员", visitor: "访客" },
  adminIs: (admin) => `管理员 ${admin}`,
  superNote: (admin) => `管理员 ${admin} 可以访问你的所有机器`,
  visitorNote: "你的机器只归你自己：管理员只看得到有哪几台",
  visitorGives: "访客的机器只归自己：不共享给别人，智能体也不协助别人。",
  promotionTitle: (admin) => `${admin} 想把你改成成员`,
  promotionWhat: "改成成员之后，管理员用得了你的每一台机器。",
  accept: "同意",
  decline: "不同意",
  accepted: "你现在是成员",
  declined: "你照旧是访客",
  codes: "入网码",
  codeRows: zhCodes,
  arrivedByCode: (name, count, at) => `${at}　「${name}」进来了 ${count} 台受管机器`,
  someCode: "一个入网码",
  notMeCodeTitle: (name, count) => `作废「${name}」并移除这 ${count} 台？`,
  notMeCodeWhat: (count) => `码泄露了，别人就能把陌生的机器塞成你的受管机器。作废之后新的进不来，这 ${count} 台随之移除。`,
  notMeCodeGo: "作废并移除",
  separator: "、",
  changePassword: "修改密码",
  passwordTitle: "修改密码",
  passwordWhat: "已登录的设备不受影响",
  password: "新密码",
  passwordAgain: "确认新密码",
  passwordsDiffer: "两次输入的密码不一致",
  passwordRule: zhPasswordRule,
  passwordFloor: zhPasswordFloor,
  strength: zhStrength,
  passwordGo: "保存",
  passwordDone: "密码已修改",
  arrived: (kind, name, at) => `${at}　新${kind} ${name}`,
  me: "是我",
  notMe: "不是我",
  notMeWhat: (name) => `移除 ${name}，它将无法再使用这个账号。`,
  kinds: { computer: "电脑", phone: "手机", web: "网页 AI" },
  forms: { desktop: "台式机", laptop: "笔记本", server: "服务器" },
  removeTitle: (name) => `移除 ${name}？`,
  removeGo: "移除",
  given: "谁能用你的东西",
  sharedTo: "共享给",
  assisting: "协助",
  weeklyShort: (amount) => `${amount}/周`,
  dailyShort: (amount) => `${amount}/天`,
  untilShort: (at) => `到 ${at}`,
  takeBack: (what) => `收回${what}`,
  tookBack: (what) => `已收回${what}`,
  sharedLabel: (machine, account) => ` ${account} 对 ${machine} 的使用`,
  assistLabel: (login, account) => ` ${login} 对 ${account} 的协助`,
  givenSub: (machines, logins) => [machines ? `${machines} 台机器` : "", logins ? `${logins} 个登录协助` : ""].filter(Boolean).join("，"),
  groupOf: (group, count) => `${group}（${count} 台）`,
  reaching: "能访问你的机器的智能体",
  reachingWhat: "你用过别人机器上的智能体之后，它们可以访问你的机器",
  reachingLine: (machine, owner) => (owner ? `${owner} 的智能体` : machine),
  reachingSince: (at) => `${at} 起`,
  stopReaching: "撤销",
  stopReachingTitle: (machine) => `撤销 ${machine} 的访问？`,
  stopReachingWhat: (machine) => `${machine} 上的智能体将无法再访问你的机器；下次用到它时会重新获得访问。`,
  devices: (web) => (web ? "手机与网页 AI" : "手机"),
  thisOne: "本机",
  since: (at) => `${at} 登录`,
  onlineNow: "在线",
  lastSeen: (at) => `${at} 在线`,
  neverSeen: "从未连接",
  removeDevice: "移除",
  removeDeviceWhat: (name) => `${name} 将退出这个账号。`,
  leave: "离开这个网络",
  leaveTitle: (network) => `离开 ${network}？`,
  leaveWhat: (network, registered) =>
    registered
      ? `这台电脑将退出 ${network} 里的账号，不再是这个网络里的一台机器，共享与协助随之结束。电脑上的数据不受影响，要回来重新加入就是。`
      : `这台电脑将退出 ${network} 里的账号。`,
  leaveGo: "离开",
  logout: "退出登录",
  logoutTitle: (network) => `退出 ${network}？`,
  logoutWhat: (network) => `这台设备将退出 ${network} 里的账号。`,
  logoutGo: "退出",
  none: "未加入网络",
  logIn: "加入",
  undo: "撤销",
  working: "处理中…",
  close: "关闭",
  failed: failure(zhProblems, "操作失败"),
};

export const enMe: MeWords = {
  title: "Me",
  tabs: { given: "Sharing", devices: "Devices", codes: "Codes" },
  level: { super: "administrator", member: "member", visitor: "visitor" },
  adminIs: (admin) => `administrator ${admin}`,
  superNote: (admin) => `Administrator ${admin} can access all your machines`,
  visitorNote: "Your machines are yours alone: the administrator only sees which there are",
  visitorGives: "A visitor's machines are their own: shared with no one, and their agents assist no one.",
  promotionTitle: (admin) => `${admin} would make you a member`,
  promotionWhat: "As a member, the administrator can use every machine of yours.",
  accept: "Agree",
  decline: "Decline",
  accepted: "You are a member now",
  declined: "You stay a visitor",
  codes: "Codes",
  codeRows: enCodes,
  arrivedByCode: (name, count, at) => `${at}: ${count} managed machine${count === 1 ? "" : "s"} came in by "${name}"`,
  someCode: "a code",
  notMeCodeTitle: (name, count) => `Revoke "${name}" and remove these ${count}?`,
  notMeCodeWhat: (count) => `A leaked code lets anyone make a strange machine yours. Once revoked no new one comes in, and these ${count} are removed.`,
  notMeCodeGo: "Revoke and remove",
  separator: ", ",
  changePassword: "Change password",
  passwordTitle: "Change password",
  passwordWhat: "Signed-in devices stay signed in",
  password: "New password",
  passwordAgain: "Confirm new password",
  passwordsDiffer: "Passwords don't match",
  passwordRule: enPasswordRule,
  passwordFloor: enPasswordFloor,
  strength: enStrength,
  passwordGo: "Save",
  passwordDone: "Password changed",
  arrived: (kind, name, at) => `${at}: new ${kind} ${name}`,
  me: "It's me",
  notMe: "Not me",
  notMeWhat: (name) => `Remove ${name}? It will no longer be able to use this account.`,
  kinds: { computer: "computer", phone: "phone", web: "web AI" },
  forms: { desktop: "desktop", laptop: "laptop", server: "server" },
  removeTitle: (name) => `Remove ${name}?`,
  removeGo: "Remove",
  given: "Who can use your things",
  sharedTo: "shared with",
  assisting: "assisting",
  weeklyShort: (amount) => `${amount}/wk`,
  dailyShort: (amount) => `${amount}/day`,
  untilShort: (at) => `until ${at}`,
  takeBack: (what) => `Take back${what}`,
  tookBack: (what) => `Took back${what}`,
  sharedLabel: (machine, account) => ` ${machine} from ${account}`,
  assistLabel: (login, account) => ` ${login}'s help for ${account}`,
  givenSub: (machines, logins) =>
    [machines ? `${machines} ${machines === 1 ? "machine" : "machines"}` : "", logins ? `${logins} ${logins === 1 ? "login" : "logins"} assisting` : ""].filter(Boolean).join(", "),
  groupOf: (group, count) => `${group} (${count} machines)`,
  reaching: "Agents with access to your machines",
  reachingWhat: "Once you use an agent on someone else's machine, its agents can access your machines",
  reachingLine: (machine, owner) => (owner ? `${owner}'s agents` : machine),
  reachingSince: (at) => `since ${at}`,
  stopReaching: "Revoke",
  stopReachingTitle: (machine) => `Revoke access for ${machine}?`,
  stopReachingWhat: (machine) => `Agents on ${machine} will lose access to your machines until you use one of them again.`,
  devices: (web) => (web ? "Phones and web AI" : "Phones"),
  thisOne: "this device",
  since: (at) => `signed in ${at}`,
  onlineNow: "online",
  lastSeen: (at) => `online ${at}`,
  neverSeen: "never connected",
  removeDevice: "Remove",
  removeDeviceWhat: (name) => `${name} will be signed out of this account.`,
  leave: "Leave this network",
  leaveTitle: (network) => `Leave ${network}?`,
  leaveWhat: (network, registered) =>
    registered
      ? `This computer will sign out of its accounts in ${network} and stop being a machine there; its shares and assistance end. Data on it is untouched, and you can join again any time.`
      : `This computer will sign out of its accounts in ${network}.`,
  leaveGo: "Leave",
  logout: "Sign out",
  logoutTitle: (network) => `Sign out of ${network}?`,
  logoutWhat: (network) => `This device will sign out of its accounts in ${network}.`,
  logoutGo: "Sign out",
  none: "Not in a network",
  logIn: "Join",
  undo: "Undo",
  working: "Working…",
  close: "Close",
  failed: failure(enProblems, "Couldn't complete"),
};
