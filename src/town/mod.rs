//! The town: residents living in a project, and the sessions the World holds for them.
//!
//! Every resident is a session of some agent program on some machine. The World does four
//! things with the sessions it holds: starts them, delivers words to them (a new turn when
//! idle, folded into the running turn when busy, woken when stopped), lets anyone see what
//! they are doing, and stops them. Asking a colleague and a user talking to a resident from
//! another device are two uses of the same four operations (`specs/world-colleagues.md`).

pub(crate) mod claude;
pub(crate) mod codex;
pub(crate) mod core;
pub(crate) mod records;
