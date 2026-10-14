// What the machines page says of the people of a network (`world-gui.md`,
// 账号、机器、共享与协助): each account a box on the drawing and, for the
// network's administrator, the dashed box that adds a person, each person's
// "⋯", and whether anyone may sign up by themselves.

import type { PasswordProblem } from "./password";
import { enPasswordFloor, enPasswordRule, enStrength, zhPasswordFloor, zhPasswordRule, zhStrength } from "./account-words";
import { enProblems, failure, zhProblems } from "./problem";

export interface PeopleWords {
  /** The dashed box that adds a person. */
  addPerson: string;
  /** On a person's box while none of their machines has come yet. */
  noMachinesYet: string;
  locked: string;
  /** A person's "⋯", for those who hear the drawing. */
  more(account: string): string;
  resetPassword: string;
  lock: string;
  unlock: string;
  makeAdmin: string;
  removePerson: string;
  /** What a person is (`world-network.md`, 一个中枢，几个账号): the two
   *  choices when adding one, each with what it means; making one a
   *  visitor at once, or asking them to become a member; and the tags on
   *  a box. */
  roleMember: string;
  roleMemberWhat: string;
  roleVisitor: string;
  roleVisitorWhat: string;
  makeVisitor: string;
  makeMember: string;
  cancelMember: string;
  visitorTitle(account: string): string;
  visitorWhat: string;
  visitorGo: string;
  memberAsked(account: string): string;
  memberCancelled(account: string): string;
  visitorTag: string;
  waitingTag: string;
  addTitle: string;
  name: string;
  nameHint: string;
  password: string;
  passwordAgain: string;
  passwordsDiffer: string;
  passwordRule: Record<PasswordProblem, string>;
  passwordFloor: string;
  strength: [string, string, string];
  addGo: string;
  added(account: string): string;
  /** Where the person logs in: the server's address and their account. */
  hub: string;
  account: string;
  copyLogin: string;
  /** What is copied for the person to log in with. */
  loginText(address: string, account: string): string;
  copied: string;
  done: string;
  resetTitle(account: string): string;
  resetWhat: string;
  resetGo: string;
  lockedSaid(account: string): string;
  unlockedSaid(account: string): string;
  makeAdminTitle(account: string): string;
  makeAdminWhat(account: string): string;
  makeAdminGo: string;
  removeTitle(account: string): string;
  removeWhat(machines: string[], projects: string[]): string;
  removeGo: string;
  /** Whether anyone who reaches the server may open an account. */
  signUp: string;
  signUpState(on: boolean): string;
  signUpTitle: string;
  signUpWhat: string;
  signUpGo: string;
  signUpOpened: string;
  signUpClosed: string;
  cancel: string;
  working: string;
  failed(why: string): string;
}

export const zhPeople: PeopleWords = {
  addPerson: "加一个人",
  noMachinesYet: "还没有机器",
  locked: "已锁定",
  more: (account) => `${account} · 更多`,
  resetPassword: "重设密码",
  lock: "锁定",
  unlock: "解锁",
  makeAdmin: "交出管理员",
  removePerson: "移除这个人",
  roleMember: "成员",
  roleMemberWhat: "你用得了这个人的每台机器",
  roleVisitor: "访客",
  roleVisitorWhat: "机器只归这个人自己，你只看得到有哪几台",
  makeVisitor: "改成访客",
  makeMember: "改成成员",
  cancelMember: "撤回改成成员",
  visitorTitle: (account) => `把 ${account} 改成访客？`,
  visitorWhat: "当场生效：这个人的机器你不再用得了，只看得到有哪几台；这个人给出的共享与协助随之收回。",
  visitorGo: "改成访客",
  memberAsked: (account) => `等 ${account} 同意`,
  memberCancelled: (account) => `${account} 照旧是访客`,
  visitorTag: "访客",
  waitingTag: "等待同意",
  addTitle: "加一个人",
  name: "名字",
  nameHint: "小写字母开头，可含数字和连字符",
  password: "初始密码",
  passwordAgain: "确认密码",
  passwordsDiffer: "两次输入的密码不一致",
  passwordRule: zhPasswordRule,
  passwordFloor: zhPasswordFloor,
  strength: zhStrength,
  addGo: "加上",
  added: (account) => `已加上 ${account}`,
  hub: "中枢服务器",
  account: "账号",
  copyLogin: "复制登录信息",
  loginText: (address, account) => `中枢服务器 ${address}\n账号 ${account}`,
  copied: "已复制",
  done: "完成",
  resetTitle: (account) => `重设 ${account} 的密码`,
  resetWhat: "已登录的设备不受影响",
  resetGo: "重设",
  lockedSaid: (account) => `已锁定 ${account}`,
  unlockedSaid: (account) => `已解锁 ${account}`,
  makeAdminTitle: (account) => `把管理员交给 ${account}？`,
  makeAdminWhat: (account) => `交出之后你是成员，${account} 可以使用这个网络里的所有机器。`,
  makeAdminGo: "交出",
  removeTitle: (account) => `移除 ${account}？`,
  removeWhat: (machines, projects) =>
    [
      "此操作无法撤销。",
      machines.length ? `名下的机器 ${machines.join("、")} 随之移除。` : "",
      projects.length ? `项目 ${projects.join("、")} 挪进归档，可以转交给别人。` : "",
      "共享与协助随之结束。",
    ].join(""),
  removeGo: "移除",
  signUp: "开放注册",
  signUpState: (on) => (on ? "开放注册：开" : "开放注册：关"),
  signUpTitle: "开放注册？",
  signUpWhat: "能连上这台中枢服务器的任何人都能自己注册一个访客账号；中枢服务器在公网上时，陌生人也能注册。",
  signUpGo: "开放",
  signUpOpened: "已开放注册",
  signUpClosed: "已关闭注册",
  cancel: "取消",
  working: "处理中…",
  failed: failure(zhProblems, "操作失败"),
};

export const enPeople: PeopleWords = {
  addPerson: "Add a person",
  noMachinesYet: "no machines yet",
  locked: "locked",
  more: (account) => `${account} · more`,
  resetPassword: "Reset password",
  lock: "Lock",
  unlock: "Unlock",
  makeAdmin: "Hand over admin",
  removePerson: "Remove this person",
  roleMember: "Member",
  roleMemberWhat: "You can use every machine of theirs",
  roleVisitor: "Visitor",
  roleVisitorWhat: "Their machines are theirs alone; you only see which there are",
  makeVisitor: "Make a visitor",
  makeMember: "Make a member",
  cancelMember: "Keep as a visitor",
  visitorTitle: (account) => `Make ${account} a visitor?`,
  visitorWhat: "At once: you can no longer use their machines, only see which there are; what they shared and let assist is taken back.",
  visitorGo: "Make a visitor",
  memberAsked: (account) => `Waiting for ${account} to agree`,
  memberCancelled: (account) => `${account} stays a visitor`,
  visitorTag: "visitor",
  waitingTag: "waiting to agree",
  addTitle: "Add a person",
  name: "Name",
  nameHint: "Starts with a lowercase letter; digits and hyphens allowed",
  password: "Initial password",
  passwordAgain: "Confirm password",
  passwordsDiffer: "Passwords don't match",
  passwordRule: enPasswordRule,
  passwordFloor: enPasswordFloor,
  strength: enStrength,
  addGo: "Add",
  added: (account) => `Added ${account}`,
  hub: "Hub",
  account: "Account",
  copyLogin: "Copy login details",
  loginText: (address, account) => `Hub ${address}\nAccount ${account}`,
  copied: "Copied",
  done: "Done",
  resetTitle: (account) => `Reset ${account}'s password`,
  resetWhat: "Signed-in devices stay signed in",
  resetGo: "Reset",
  lockedSaid: (account) => `Locked ${account}`,
  unlockedSaid: (account) => `Unlocked ${account}`,
  makeAdminTitle: (account) => `Make ${account} the administrator?`,
  makeAdminWhat: (account) => `You'll become a member, and ${account} will be able to use every machine in the network.`,
  makeAdminGo: "Hand over",
  removeTitle: (account) => `Remove ${account}?`,
  removeWhat: (machines, projects) =>
    [
      "This can't be undone. ",
      machines.length ? `Their machines ${machines.join(", ")} are removed with them. ` : "",
      projects.length ? `Projects ${projects.join(", ")} are archived and can be handed to someone else. ` : "",
      "Their shares and assistance end.",
    ].join(""),
  removeGo: "Remove",
  signUp: "Open sign-up",
  signUpState: (on) => (on ? "Open sign-up: on" : "Open sign-up: off"),
  signUpTitle: "Open sign-up?",
  signUpWhat: "Anyone who can reach this hub will be able to create a visitor account; if the hub is on the internet, strangers can too.",
  signUpGo: "Open",
  signUpOpened: "Sign-up opened",
  signUpClosed: "Sign-up closed",
  cancel: "Cancel",
  working: "Working…",
  failed: failure(enProblems, "Couldn't complete"),
};
