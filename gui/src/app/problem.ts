// What went wrong, in the interface's words: the kernel answers in English
// sentences meant for logs and terminals; the page says what they mean to
// the person and keeps the original for the pointer to rest on.

export interface ProblemWords {
  wrongPassword: string;
  tooManyTries: string;
  locked: string;
  unreachable: string;
  /** A hub's address that is no address. */
  badAddress: string;
  nameTaken: string;
  /** A name another machine had before it was renamed, which still names
   *  that one. */
  formerName: string;
  /** Devices on the server that cannot read the name yet. */
  olderDevices: string;
  /** A machine, or the hub, whose fastctx does not rename yet. */
  renameOlder: string;
  accountTaken: string;
  noSelfRegistration: string;
  codeUsed: string;
  codeExpired: string;
  codeWrong: string;
  weakPassword: string;
  notAllowed: string;
  offline: string;
  /** This computer's own service is not running, and nothing brings it back
   *  now. */
  serviceDown: string;
  /** This computer's own service is passing its work to its next self. */
  serviceSwitching: string;
  /** Codex's output limit, shared by every tool, is below what the World's
   *  answers need; raising it is the user's to ask for. */
  outputLimit: string;
  /** An assistance's allowance reached, until it has room again. */
  allowanceUsed(reached: Reached): string;
  somethingElse: string;
}

export const zhProblems: ProblemWords = {
  wrongPassword: "账号或密码不正确",
  tooManyTries: "尝试次数过多，请稍后再试",
  locked: "该账号已被锁定，请联系管理员",
  unreachable: "无法连接中枢服务器，请检查地址和网络",
  badAddress: "中枢服务器地址不对",
  nameTaken: "这个名称已被使用",
  formerName: "这是另一台机器的曾用名，仍指向它",
  olderDevices: "网络里还有设备在用旧版本，读不了这个名字",
  renameOlder: "Fastctx 版本较旧，更新后才能改名",
  accountTaken: "这个账号已存在",
  noSelfRegistration: "这台中枢服务器不开放注册，请联系管理员",
  codeUsed: "二维码已使用过",
  codeExpired: "二维码已过期",
  codeWrong: "二维码无效",
  weakPassword: "密码不符合要求",
  notAllowed: "没有权限",
  offline: "机器离线",
  serviceDown: "本机服务没有运行",
  serviceSwitching: "本机服务正在切换，请稍后再试",
  outputLimit: "Codex 的工具输出上限不够",
  allowanceUsed: (reached) => {
    const until = `${reached.until.getMonth() + 1}月${reached.until.getDate()}日 ${clock(reached.until)}`;
    if (reached.kind === "fiveHour") return `这个登录的 5 小时额度已用到 ${reached.share}%，协助暂停，${until} 后继续`;
    return `${reached.kind === "daily" ? "每天" : "这周"}的协助额度（约 ${reached.amount}）用完了，${until} 后继续`;
  },
  somethingElse: "操作未完成",
};

export const enProblems: ProblemWords = {
  wrongPassword: "Wrong account or password",
  tooManyTries: "Too many attempts. Try again later",
  locked: "This account is locked. Contact the administrator",
  unreachable: "Can't reach the hub. Check the address and network",
  badAddress: "That hub address isn't valid",
  nameTaken: "That name is already in use",
  formerName: "Another machine had that name and it still points there",
  olderDevices: "Some devices in this network run an older version that can't read this name",
  renameOlder: "Its Fastctx is older; update it to rename",
  accountTaken: "That account already exists",
  noSelfRegistration: "This hub doesn't allow sign-up. Contact the administrator",
  codeUsed: "This code has already been used",
  codeExpired: "This code has expired",
  codeWrong: "This code isn't valid",
  weakPassword: "The password doesn't meet the requirements",
  notAllowed: "You don't have permission",
  offline: "The machine is offline",
  serviceDown: "The local service isn't running",
  serviceSwitching: "The local service is switching over. Try again in a moment",
  outputLimit: "Codex's tool output limit is too low",
  allowanceUsed: (reached) => {
    const until = `${reached.until.toLocaleString("en", { month: "short", day: "numeric" })} ${clock(reached.until)}`;
    if (reached.kind === "fiveHour") return `This login's 5-hour limit is ${reached.share}% used, so assisting pauses; it goes on ${until}`;
    return `${reached.kind === "daily" ? "A day's" : "This week's"} assistance (≈ ${reached.amount}) is used up; it goes on ${until}`;
  },
  somethingElse: "Something went wrong",
};

const KNOWN: [RegExp, keyof ProblemWords][] = [
  [/password is wrong|does not exist there or the password/i, "wrongPassword"],
  [/too many wrong passwords/i, "tooManyTries"],
  [/is locked/i, "locked"],
  [/the name .* is taken/i, "nameTaken"],
  [/before it was renamed, and still names it/i, "formerName"],
  [/release of fastctx that takes a new name|older fastctx that renames no machine/i, "renameOlder"],
  [/is not one of the account .* machines/i, "notAllowed"],
  [/release of fastctx that cannot read such a name/i, "olderDevices"],
  [/account .* exists already/i, "accountTaken"],
  [/opens accounts only through/i, "noSelfRegistration"],
  [/token was used already/i, "codeUsed"],
  [/token has expired/i, "codeExpired"],
  [/--raise-output-limit/, "outputLimit"],
  [/(?<![\w-])token(?![\w-])|not .* the token names/i, "codeWrong"],
  [/a password has at least|a password mixes|a password does not contain|first ones anyone tries/i, "weakPassword"],
  [/not allowed|may not|forbidden|only the super administrator|permission denied/i, "notAllowed"],
  [/is offline|not online|went offline/i, "offline"],
  [/the node is not running|the node was started but exited/i, "serviceDown"],
  [/handing over to its successor/i, "serviceSwitching"],
  [/the hub's address .*: (invalid|failed to lookup|no such host|name or service not known|nodename nor servname)|write the hub's address/i, "badAddress"],
  [/connect|timed out|unreachable|refused|no route|dns|resolve|no such host|no answer within|said nothing for/i, "unreachable"],
];

/** An assistance's allowance reached, as the login's machine says it
 *  (`fastctx-node`, `usage::allowance`): a weekly or daily amount spent, or
 *  the login's five-hour window at the share it assists up to, and until
 *  when. */
export type Reached = { kind: "weekly" | "daily"; amount: string; until: Date } | { kind: "fiveHour"; share: number; until: Date };

const ALLOWANCE = /the (weekly|daily) allowance of \$([\d.,]+) this login gives \S+ is used up until (\d{4}-\d{2}-\d{2}) (\d{2}:\d{2}) UTC/;
const FIVE_HOUR = /this login's five-hour window is past the (\d+)% it assists \S+ up to until (\d{4}-\d{2}-\d{2}) (\d{2}:\d{2}) UTC/;

/** The allowance reached and until when, where `text` says so. */
export function allowanceOf(text: string): Reached | null {
  const spent = ALLOWANCE.exec(text);
  if (spent) return { kind: spent[1] as "weekly" | "daily", amount: `$${spent[2]!.replace(/\.00$/, "")}`, until: new Date(`${spent[3]}T${spent[4]}:00Z`) };
  const window = FIVE_HOUR.exec(text);
  return window ? { kind: "fiveHour", share: Number(window[1]), until: new Date(`${window[2]}T${window[3]}:00Z`) } : null;
}

function clock(at: Date): string {
  return `${at.getHours()}:${String(at.getMinutes()).padStart(2, "0")}`;
}

/** The sentence for an error the kernel is known to give, or none. */
export function known(error: unknown, words: ProblemWords): string | null {
  const detail = error instanceof Error ? error.message : String(error);
  const allowance = allowanceOf(detail);
  if (allowance) return words.allowanceUsed(allowance);
  const found = KNOWN.find(([pattern]) => pattern.test(detail));
  const said = found ? words[found[1]] : null;
  return typeof said === "string" ? said : null;
}

/** The sentence to show for `error`, and the original to keep beside it. */
export function plainly(error: unknown, words: ProblemWords): { text: string; detail: string } {
  const detail = error instanceof Error ? error.message : String(error);
  return { text: known(detail, words) ?? words.somethingElse, detail };
}

/** A failure's words for one kind of action: the known sentence where there
 *  is one, else the action's own short "could not". */
export function failure(words: ProblemWords, otherwise: string): (why: string) => string {
  return (why) => known(why, words) ?? otherwise;
}

/** The error's own text, for the hover of where its sentence is shown. */
export function detailOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
