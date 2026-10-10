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
  somethingElse: string;
}

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
  [/connect|timed out|unreachable|refused|no route|dns|resolve|host/i, "unreachable"],
];

/** The sentence to show for `error`, and the original to keep beside it. */
export function plainly(error: unknown, words: ProblemWords): { text: string; detail: string } {
  const detail = error instanceof Error ? error.message : String(error);
  const known = KNOWN.find(([pattern]) => pattern.test(detail));
  return { text: known ? words[known[1]] : words.somethingElse, detail };
}
