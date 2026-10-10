// What went wrong, in the interface's words: the kernel answers in English
// sentences meant for logs and terminals; the page says what they mean to
// the person and keeps the original for the pointer to rest on.

export interface ProblemWords {
  wrongPassword: string;
  tooManyTries: string;
  locked: string;
  unreachable: string;
  nameTaken: string;
  accountTaken: string;
  noSelfRegistration: string;
  codeUsed: string;
  codeExpired: string;
  codeWrong: string;
  weakPassword: string;
  notAllowed: string;
  offline: string;
  somethingElse: string;
}

export const zhProblems: ProblemWords = {
  wrongPassword: "账号或密码不正确",
  tooManyTries: "尝试次数过多，请稍后再试",
  locked: "该账号已被锁定，请联系管理员",
  unreachable: "无法连接服务器，请检查地址和网络",
  nameTaken: "这个名称已被使用",
  accountTaken: "这个账号已存在",
  noSelfRegistration: "该服务器不开放注册，请联系管理员",
  codeUsed: "二维码已使用过",
  codeExpired: "二维码已过期",
  codeWrong: "二维码无效",
  weakPassword: "密码不符合要求",
  notAllowed: "没有权限",
  offline: "机器离线",
  somethingElse: "操作未完成",
};

export const enProblems: ProblemWords = {
  wrongPassword: "Wrong account or password",
  tooManyTries: "Too many attempts. Try again later",
  locked: "This account is locked. Contact the administrator",
  unreachable: "Can't reach the server. Check the address and network",
  nameTaken: "That name is already in use",
  accountTaken: "That account already exists",
  noSelfRegistration: "This server doesn't allow sign-up. Contact the administrator",
  codeUsed: "This code has already been used",
  codeExpired: "This code has expired",
  codeWrong: "This code isn't valid",
  weakPassword: "The password doesn't meet the requirements",
  notAllowed: "You don't have permission",
  offline: "The machine is offline",
  somethingElse: "Something went wrong",
};

const KNOWN: [RegExp, keyof ProblemWords][] = [
  [/password is wrong|does not exist there or the password/i, "wrongPassword"],
  [/too many wrong passwords/i, "tooManyTries"],
  [/is locked/i, "locked"],
  [/the name .* is taken/i, "nameTaken"],
  [/account .* exists already/i, "accountTaken"],
  [/opens accounts only through/i, "noSelfRegistration"],
  [/token was used already/i, "codeUsed"],
  [/token has expired/i, "codeExpired"],
  [/token|not .* the token names/i, "codeWrong"],
  [/a password has at least|a password mixes|a password does not contain|first ones anyone tries/i, "weakPassword"],
  [/not allowed|may not|forbidden|only the super administrator|permission denied/i, "notAllowed"],
  [/is offline|not online|went offline/i, "offline"],
  [/connect|timed out|unreachable|refused|no route|dns|resolve|no such host/i, "unreachable"],
];

/** The sentence for an error the kernel is known to give, or none. */
export function known(error: unknown, words: ProblemWords): string | null {
  const detail = error instanceof Error ? error.message : String(error);
  const found = KNOWN.find(([pattern]) => pattern.test(detail));
  return found ? words[found[1]] : null;
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
