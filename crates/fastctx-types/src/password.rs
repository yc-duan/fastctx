//! What a new password must be (`world-network.md`, 登录与入网). Every way a
//! password is set checks it here: opening an account, changing or
//! resetting one, making a hub. The hub never sees a password (OPAQUE), so
//! the check is made where the password is typed; the interface mirrors
//! these rules to say so before anything is sent.
//!
//! The bar is a password a person can keep in mind: eight characters of at
//! least two kinds, not one of the passwords guessed first, not the
//! account's own name. Guessing on the wire is out of reach (OPAQUE) and
//! guessing at the hub is slowed per account and per address, so length
//! beyond this buys little.

use std::fmt;

/// The fewest characters a password has.
pub const SHORTEST: usize = 8;

/// Why a password is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    Short,
    /// Letters only, digits only, or symbols only.
    OneKind,
    /// The account's name is in it.
    Name,
    /// One of the passwords tried first.
    Common,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Problem::Short => "a password has at least 8 characters",
            Problem::OneKind => "a password mixes at least two of letters, digits and symbols",
            Problem::Name => "a password does not contain the account's name",
            Problem::Common => "that password is one of the first ones anyone tries",
        })
    }
}

/// Passwords tried first, lowercased; a password is refused when it is one
/// of these, whatever its case.
const COMMON: &[&str] = &[
    "password", "password1", "password123", "passw0rd", "p@ssw0rd", "p@ssword", "12345678", "123456789", "1234567890",
    "qwerty123", "qwertyuiop", "1q2w3e4r", "1qaz2wsx", "abc12345", "abcd1234", "a1b2c3d4", "iloveyou1", "admin123",
    "admin1234", "welcome1", "letmein1", "changeme", "changeme1", "qwer1234", "asdf1234", "zxcv1234", "11111111",
    "00000000", "88888888", "66666666", "woaini1314", "5201314a", "aa123456", "a123456789", "qq123456",
];

/// What is wrong with `password` as a new password for `account`, if
/// anything.
pub fn problem(account: &str, password: &str) -> Option<Problem> {
    if password.chars().count() < SHORTEST {
        return Some(Problem::Short);
    }
    let kinds = [
        password.chars().any(char::is_alphabetic),
        password.chars().any(|c| c.is_ascii_digit()),
        password.chars().any(|c| !c.is_alphanumeric()),
    ];
    if kinds.iter().filter(|kind| **kind).count() < 2 {
        return Some(Problem::OneKind);
    }
    let lower = password.to_lowercase();
    if account.chars().count() >= 3 && lower.contains(&account.to_lowercase()) {
        return Some(Problem::Name);
    }
    if COMMON.contains(&lower.as_str()) {
        return Some(Problem::Common);
    }
    None
}

/// `problem` as the error every setting of a password returns.
pub fn check(account: &str, password: &str) -> Result<(), String> {
    problem(account, password).map_or(Ok(()), |problem| Err(problem.to_string()))
}
