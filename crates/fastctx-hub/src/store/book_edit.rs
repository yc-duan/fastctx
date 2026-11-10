//! The rows and derived structures a catalog edit can change. The general
//! path remains available to callers that do not declare a narrower edit.

#[derive(Clone, Default)]
pub(crate) enum BookEdit {
    #[default]
    Full,
    /// Only audit records and their sequence change.
    Audit,
    /// In-place configuration, preserving name, level and locked status.
    Account(usize),
    /// In-place group labels, preserving all identity and authority fields.
    Groups(Vec<usize>),
    /// Token rows may be inserted, removed or reordered.
    Tokens,
}

impl BookEdit {
    pub(crate) fn list(&self, kind: &str) -> (bool, Option<&[usize]>) {
        match self {
            Self::Full => (true, None),
            Self::Account(at) if kind == "accounts" => (true, Some(std::slice::from_ref(at))),
            Self::Groups(positions) if kind == "members" => (true, Some(positions)),
            Self::Tokens if kind == "tokens" => (true, None),
            _ => (false, None),
        }
    }

    pub(crate) fn is_full(&self) -> bool {
        matches!(self, Self::Full)
    }
}
