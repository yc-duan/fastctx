// What a new password must be, as the kernel checks it
// (`crates/fastctx-types/src/password.rs`), so the page can say so as it is
// typed rather than after it is sent. Kept to the same rules: eight
// characters of at least two kinds, not the account's name, not one of the
// passwords tried first.

export type PasswordProblem = "short" | "one-kind" | "name" | "common";

const SHORTEST = 8;

const COMMON = new Set([
  "password", "password1", "password123", "passw0rd", "p@ssw0rd", "p@ssword", "12345678", "123456789", "1234567890",
  "qwerty123", "qwertyuiop", "1q2w3e4r", "1qaz2wsx", "abc12345", "abcd1234", "a1b2c3d4", "iloveyou1", "admin123",
  "admin1234", "welcome1", "letmein1", "changeme", "changeme1", "qwer1234", "asdf1234", "zxcv1234", "11111111",
  "00000000", "88888888", "66666666", "woaini1314", "5201314a", "aa123456", "a123456789", "qq123456",
]);

export function passwordProblem(account: string, password: string): PasswordProblem | null {
  if ([...password].length < SHORTEST) return "short";
  const kinds = [/\p{L}/u.test(password), /[0-9]/.test(password), /[^\p{L}\p{N}]/u.test(password)].filter(Boolean).length;
  if (kinds < 2) return "one-kind";
  const lower = password.toLowerCase();
  if ([...account].length >= 3 && lower.includes(account.toLowerCase())) return "name";
  if (COMMON.has(lower)) return "common";
  return null;
}

/** How far past the bar a password is, 0 to 3: refused, enough, good,
 *  strong. Length and kinds beyond the bar each add a step. */
export function passwordStrength(account: string, password: string): number {
  if (!password || passwordProblem(account, password)) return 0;
  const kinds = [/\p{Ll}/u, /\p{Lu}/u, /[0-9]/, /[^\p{L}\p{N}]/u].filter((kind) => kind.test(password)).length;
  const length = [...password].length;
  return 1 + (length >= 12 ? 1 : 0) + (kinds >= 3 ? 1 : 0);
}
