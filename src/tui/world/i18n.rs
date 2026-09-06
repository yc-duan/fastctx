//! Complete World UI copy; technical values and exported files retain their protocol spelling.

use crate::control::i18n::Language;

#[path = "locales/de.rs"]
mod de;
#[path = "locales/es.rs"]
mod es;
#[path = "locales/fr.rs"]
mod fr;
#[path = "locales/id.rs"]
mod id;
#[path = "locales/it.rs"]
mod it;
#[path = "locales/ja.rs"]
mod ja;
#[path = "locales/ko.rs"]
mod ko;
#[path = "locales/nl.rs"]
mod nl;
#[path = "locales/pl.rs"]
mod pl;
#[path = "locales/pt_br.rs"]
mod pt_br;
#[path = "locales/ru.rs"]
mod ru;
#[path = "locales/tr.rs"]
mod tr;
#[path = "locales/uk.rs"]
mod uk;
#[path = "locales/vi.rs"]
mod vi;
#[path = "locales/zh_cn.rs"]
mod zh_cn;
#[path = "locales/zh_tw.rs"]
mod zh_tw;

macro_rules! define_messages {
    ($($field:ident: $value:literal),* $(,)?) => {
        #[derive(Debug)]
        pub(crate) struct WorldMessages { $(pub(crate) $field: &'static str,)* }
        const EN: WorldMessages = WorldMessages { $($field: $value,)* };
    };
}

define_messages! {
    title: "World",
    overview: "Overview",
    machines: "Machines",
    grants: "Access",
    health: "Health",
    events: "Events",
    service: "Node service",
    hub: "Local hub",
    welcome: "One world, many machines.",
    welcome_body: "Connect your trusted machines so your AI can work across them. Create a World using your own running hub, or paste an invite to join one.",
    create: "Create a World",
    join: "Join a World",
    actions: "Actions",
    refresh: "Refresh from hub",
    inspect: "Check local state",
    reconnect: "Reconnect to hub",
    invite: "Invite a machine",
    description: "Machine description",
    private_note: "My private note",
    effective_note: "Effective note",
    edit_description: "Edit this machine's description",
    edit_note: "Edit my note for this machine",
    description_help: "Shared with the World. Edit it on the machine that owns it. Up to 256 characters; Ctrl-J adds a line break.",
    note_help: "Only this controller sees this note. It overrides the description in AI output. Clear it to use the description again.",
    revoke: "Revoke this member",
    leave: "Leave this World",
    add_grant: "Add an allow rule",
    delete_grant: "Delete this rule",
    install: "Install automatic service",
    start: "Start node",
    restart: "Restart node",
    stop: "Stop node",
    uninstall: "Uninstall service",
    choose_hub: "Choose hub data directory",
    export: "Save invite to a file",
    next_events: "Load next events",
    event_cursor: "Read from a cursor",
    open_agents: "Connect agent apps",
    loading: "Loading local state…",
    empty: "Nothing to show yet.",
    permission: "Permission denied",
    error: "Could not load this view",
    unavailable: "Node service is unavailable",
    unavailable_body: "Start the node or install its service. Local tools still work. After an upgrade, restart the service to use this build.",
    stale: "Last known state · not updating",
    unknown: "Unknown",
    none: "None",
    online: "Online",
    offline: "Offline",
    revoked: "Revoked",
    connecting: "Connecting",
    connected: "Connected",
    stopped: "Stopped",
    installed: "Installed",
    not_installed: "Not installed",
    yes: "Yes",
    no: "No",
    this_machine: "This machine",
    observed: "Observed",
    last_seen: "Last seen",
    inventory_time: "Inventory collected",
    host: "Host name",
    system: "System",
    hardware: "Hardware",
    memory: "Memory",
    disks: "Disks",
    gpus: "GPUs",
    home: "Home directory",
    user: "User",
    tags: "Tags",
    capabilities: "Capabilities",
    addresses: "Addresses",
    shell: "Shell",
    version: "Version",
    network: "Network mode",
    interface: "Physical interface",
    tls: "TLS mode",
    heartbeat: "Last heartbeat",
    ack: "Last acknowledgement",
    rtt: "Hub RTT",
    outbox: "Queued messages",
    epoch: "Key epoch",
    replaced: "Replaced sessions",
    grant_revision: "Access revision",
    grant_stale: "Newer access rules are pending",
    engine: "Hosting control center",
    sessions: "Agent connections",
    running_calls: "Running calls",
    name: "Machine name",
    hub_address: "Hub address",
    bootstrap: "Bootstrap password",
    invite_string: "Invite string",
    auto_service: "Install automatic service",
    setup_help: "On an always-on machine, run fastctx hub serve. Use its address and the one-time bootstrap password shown there. The hub must be reachable from your machines.",
    join_help: "Paste the whole invite. A suggested name may be included; you can choose another available name. The invite is used once.",
    name_help: "1–32 lowercase letters, digits or hyphens. No leading/trailing hyphen. This routing name cannot be renamed later.",
    network_help: "auto tries the physical network first, then system routing. direct bypasses proxies/TUN. system follows this computer's network.",
    optional_help: "Optional. Leave blank to use the default.",
    tags_help: "Optional tags, separated by spaces or commas.",
    hours: "Valid for hours",
    hours_help: "1–8760 hours. The invite contains a secret; share it only with the intended machine.",
    hubs_help: "Optional replacement hub addresses, separated by spaces or commas.",
    principal: "Who may act",
    targets: "On which machines",
    verbs: "Allowed tools",
    expires: "Expires at (UTC)",
    principal_help: "A principal identity, or * for everyone. It need not be a machine name.",
    targets_help: "Machine names, tag:office or all; separate selectors with spaces or commas.",
    verbs_help: "Tool names, separated by spaces or commas; * allows every tool.",
    expires_help: "Optional future RFC 3339 time, for example 2026-12-31T23:59:59Z.",
    grants_default: "Default access: every member may use every tool on every machine. The first rule replaces this default.",
    grants_help: "Rules allow a principal to use tools on selected machines. Matching rules add together; other remote calls are denied. Local tools are governed by the local user and agent app.",
    grants_empty: "This signed empty snapshot restores default access for every member, machine and tool.",
    path: "File path",
    directory: "Data directory",
    path_help: "An absolute path in an existing directory. Existing files will not be overwritten.",
    hub_help: "A hub is optional on this machine. Choose the data directory used by fastctx hub serve --data. Opening this page does not start a hub or probe the network.",
    hub_absent: "No hub status in this directory.",
    listen: "Listening on",
    pending_invites: "Unused invites",
    data_connections: "Data connections",
    rotation_pending: "Key rotation pending",
    cursor: "After event cursor",
    cursor_help: "0 starts at the oldest retained event. Only events after this cursor are loaded.",
    caught_up: "Caught up with the event log.",
    more_events: "More events are available.",
    event_gap: "Earlier events are no longer retained by the hub.",
    event_kind: "Event kind",
    subject: "Subject",
    facts: "Details",
    continue_action: "Continue",
    save: "Save",
    cancel: "Cancel",
    confirm: "Confirm",
    back: "Back",
    discard: "Discard changes",
    discard_body: "Your unsaved input will be discarded. Keep editing or discard it?",
    keep_editing: "Keep editing",
    preview: "Review this action",
    busy: "Working…",
    busy_body: "This action runs in the background. You can return to the World pages while it finishes. A sent change cannot be undone by closing this view.",
    done: "Action completed",
    partial: "Completed with warnings",
    failed: "Action failed",
    uncertain: "The result is uncertain. The change may already have happened. Check the current state before trying again.",
    running_exit: "An action is still running. Wait for its result before closing FastCtx.",
    revoke_warning: "This permanently revokes the selected member and rotates the World key for every remaining member. They must receive the new epoch to continue. The revoked name cannot be reused; joining again needs a new invite and name.",
    leave_warning: "This stops the node and deletes this machine's enrollment, keys, description and private notes. If the hub cannot be notified, another member must revoke this name. Rejoining needs a new invite and name. Restart agent sessions afterwards.",
    grant_warning: "This publishes a new access snapshot. The first rule replaces the default that allows everyone everything. Review the principal, machines and tools before continuing.",
    delete_grant_warning: "This deletes the selected rule. Deleting the last rule restores full default access for every member, machine and tool.",
    service_warning: "World calls may be interrupted. Local tools remain available. Stopping or uninstalling the service keeps this machine enrolled; start it again to restore World access.",
    hub_revoke_warning: "The hub will refuse this member. World key rotation completes when a trusted member processes the revocation. The name cannot be reused.",
    enroll_warning: "This saves an enrollment on this computer and connects it to the selected World. If automatic service installation fails, the enrollment remains and can be started from Node service.",
    enrollment_done: "Enrollment is saved. Start the node if needed, connect your agent apps, then restart existing agent sessions to load the World tools.",
    leave_done: "This machine has left the World. Reconnect agent apps and restart existing sessions to return to the local tool surface.",
    invite_help: "Keep this invite private. Select and copy the complete string, or save it to a file. Exporting again uses this same invite.",
    saved: "Saved",
    queued: "Queued for delivery",
    waiting_node: "Saved; will sync when the node starts",
    waiting_connection: "Saved; will sync after reconnection",
    validation: "Check this field",
    required: "This field is required.",
    too_long: "Limit: {limit} characters; this input has {count}. The insertion was not applied.",
    single_line: "This field needs one line. Remove control characters and line breaks.",
    shortcuts: "← → Pages  ↑ ↓ Select  Enter Open  a Actions  Esc Back",
    form_keys: "Tab Next  Shift-Tab Previous  Ctrl-S Submit  Esc Back",
    edit_keys: "Ctrl-A Select all  Ctrl-U Clear  Ctrl-J New line  F2 Show/hide",
    scroll_keys: "↑ ↓ Scroll  PgUp/PgDn Page  Esc Back",
    confirm_keys: "← → Choose  Enter Accept  Esc Cancel",
    receipt_keys: "Enter Continue  ↑ ↓ Scroll  Esc Back",
}

pub(crate) fn messages(language: Language) -> &'static WorldMessages {
    match language {
        Language::En => &EN,
        Language::ZhCn => &zh_cn::MESSAGES,
        Language::ZhTw => &zh_tw::MESSAGES,
        Language::Ja => &ja::MESSAGES,
        Language::Ko => &ko::MESSAGES,
        Language::Es => &es::MESSAGES,
        Language::Fr => &fr::MESSAGES,
        Language::De => &de::MESSAGES,
        Language::PtBr => &pt_br::MESSAGES,
        Language::Ru => &ru::MESSAGES,
        Language::It => &it::MESSAGES,
        Language::Tr => &tr::MESSAGES,
        Language::Pl => &pl::MESSAGES,
        Language::Nl => &nl::MESSAGES,
        Language::Vi => &vi::MESSAGES,
        Language::Id => &id::MESSAGES,
        Language::Uk => &uk::MESSAGES,
    }
}
