//! Goal command text and refusals, shared by the node and its clients.

/// A resident or page tried to write the goal record directly; a goal changes
/// only through the goal command's own restricted state transitions.
pub const DIRECT_RECORD: &str = "A goal is changed through the goal command, not by writing its account record.";
pub const NO_GOAL: &str = "This conversation has no goal.";
pub const UNOFFERED: &str = "Goals are not available in this version of Fastctx.";
pub const BLANK_GOAL: &str = "The goal cannot be empty.";
pub const BLANK_STATEMENT: &str = "Complete and blocked require a nonempty statement file.";
pub const RETIRED: &str = "This conversation's goal ended when the conversation left the World.";
pub const USER_PAUSED: &str = "The user paused the goal.";
pub const AGENT_PAUSED: &str = "The agent says its owner asked it to pause.";
pub const OWNER_ONLY: &str = "Only the user or the requesting conversation may resume this goal.";
pub const MAIN_ONLY: &str = "Only this goal's main agent may mark it complete or blocked.";
pub const ACCOUNT_MISSING: &str = "This conversation has no matching account for its goal.";
pub const ACCOUNT_AMBIGUOUS: &str = "Choose the goal's account and hub.";
pub const ACCOUNT_CHANGED: &str = "The goal belongs to another account.";
pub const CONVERSATION_CHANGED: &str = "This account's goal key belongs to another conversation.";
pub const OLDER_NODE: &str = "The conversation's home cannot manage goals; update it first.";
pub const RECORDS_MISSING: &str = "The account-record service has not been connected to goals.";
pub const CONTEXT_MISSING: &str = "The named-context service has not been connected to goals.";

pub const ARCHIVED: &str = "The conversation was archived or removed.";

pub const DELETE_CHANGED: &str = "The goal changed while its conversation was being deleted; retry the deletion.";

pub const HELP: &str = "fastctx goal                          show the goal and its state\nfastctx goal complete --file <draft>  the goal is done; the draft says what shows each part\nfastctx goal blocked --file <draft>   you cannot go on; the draft says what blocks you\nfastctx goal pause                    only when the user asks you to stop\n\nThe user sets the goal and resumes it; you mark it done, blocked, or paused at their word.\n\nFor a colleague you requested:\nfastctx goal pause <colleague>        pause its goal\nfastctx goal resume <colleague>       resume its goal";
pub const NO_OPERATION: &str = "A goal change needs an operation ID.";
pub const REUSED_OPERATION: &str = "This goal operation ID already names another change.";
pub const LOST_REVISION: &str = "The account record lost an accepted goal revision.";
pub const PROVIDER_TIMEOUT: &str = "The goal service did not answer within 60 seconds.";
pub const EXPECTED_GOAL: &str = "The conversation's home did not answer with its goal.";
pub const INACTIVE_JOB: &str = "The goal command does not belong to this active job.";
pub const REMOTE_JOB: &str = "A goal command away from its conversation must name its running job.";
pub const WATCH_ACCOUNT: &str = "This goal belongs to another account.";
pub const CLI_CONFLICT: &str = "the goal changed; read its current version";
pub const INVALID_READ: &str = "A goal read returned a mutation conflict.";
pub const WATCH_ENDED: &str = "The goal watch ended.";

pub const OLDER_HUB: &str = "The hub cannot carry goal controls; update it first.";
pub const OLDER_COMMAND_HUB: &str = "The hub is too old to forward resident goal commands; update it first.";
pub const OLDER_COMMAND_NODE: &str = "This node is too old to handle requester goal commands; update it first.";
