// The activity rail's glyphs: 20px, drawn on a 20-unit grid with a 1.5
// stroke, in the colour of the text around them.

import type { Glyph } from "./tool";

export type GlyphName = Named;
type Named = Glyph | "thinking" | "chevron" | "copy" | "check" | "expand" | "stop" | "send" | "guide" | "attach" | "camera" | "file" | Frame;

/** The frame's own glyphs: the sidebar, new conversations, settings, and
 *  what can be done to a conversation (sleep, wake, move to a project,
 *  archive and restore, delete), a page and its versions; an account, its
 *  password, a phone, logging out, trust and a relay; locking a person out
 *  and letting them in, signing up; a conversation's context, the
 *  provider's cache and how long it stays; saving a copy, and a file's
 *  path; the machines as a drawing and as a list; a new version; a
 *  conversation's goal, pausing and resuming it, and the panel beside it;
 *  a repair; a project's journal and decision; a sentinel; a
 *  conversation's every step. */
type Frame =
  | "menu" | "plus" | "minus" | "settings" | "sidebar" | "machine" | "close" | "more" | "back" | "sleep" | "wake" | "move"
  | "archive" | "unarchive" | "trash" | "reread" | "scan" | "grip" | "page" | "versions" | "history" | "person" | "key" | "phone"
  | "logout" | "shield" | "lock" | "unlock" | "signup" | "relay" | "save" | "link"
  | "context" | "cache" | "clock" | "tree" | "rows" | "update"
  | "note" | "plug" | "tune"
  | "goal" | "pause" | "resume" | "panel" | "repair" | "journal" | "spec" | "sentinel" | "steps" | "dock-right" | "dock-bottom" | "dock-left"
  | Marks;

/** Which agent and which kind of device, drawn small beside a conversation's
 *  name; what is done to a turn; the places of the frame added since; what an
 *  agent asks the user; a command that takes the output of the row above;
 *  where a folder walk starts. None is a maker's mark: the agents are told
 *  apart by shapes of our own. */
type Marks =
  | "claude" | "codex" | "desktop" | "laptop" | "server" | "group" | "features" | "agents" | "folder" | "folder-open" | "fork" | "info" | "regenerate"
  | "terminal" | "cross" | "fit" | "ask" | "fed" | "home" | "disk"
  | Deeds;

/** What a run did, as its rows say it (`deeds`): tests run, a commit made,
 *  a branch sent to a remote. */
type Deeds = "test" | "commit" | "push";

const paths: Record<Named, string> = {
  menu: '<path d="M4 7h12M4 13h8"/>',
  update: '<path d="M10 3.75v8.5M6.75 9.25L10 12.5l3.25-3.25"/><path d="M4.75 15.75h10.5"/>',

  claude: '<path d="M10 3.75v12.5M3.75 10h12.5M5.6 5.6l8.8 8.8M14.4 5.6l-8.8 8.8"/>',
  codex: '<path d="M10 3.6l5.6 3.2v6.4L10 16.4l-5.6-3.2V6.8z"/><path d="M8.2 8.4L6.6 10l1.6 1.6M11.8 8.4l1.6 1.6-1.6 1.6"/>',
  desktop: '<rect x="3.75" y="4.25" width="12.5" height="8.75" rx="1.25"/><path d="M7.5 15.75h5M10 13v2.75"/>',
  laptop: '<path d="M5.25 5.75a1 1 0 0 1 1-1h7.5a1 1 0 0 1 1 1v6.25h-9.5z"/><path d="M3.25 14.75h13.5l-1.1-2.75H4.35z"/>',
  server: '<rect x="4.25" y="3.75" width="11.5" height="5.25" rx="1"/><rect x="4.25" y="11" width="11.5" height="5.25" rx="1"/><circle cx="7" cy="6.4" r="0.6" fill="currentColor"/><circle cx="7" cy="13.6" r="0.6" fill="currentColor"/>',
  // Two sheets, one behind the other: an owner's group of machines.
  group: '<rect x="3.75" y="6.75" width="10" height="9" rx="1.25"/><path d="M6.75 4.25h8.5a1 1 0 0 1 1 1v8"/>',
  features: '<rect x="4" y="4" width="5" height="5" rx="1.25"/><rect x="11" y="4" width="5" height="5" rx="1.25"/><rect x="4" y="11" width="5" height="5" rx="1.25"/><path d="M13.5 11v5M11 13.5h5"/>',
  agents: '<rect x="3.75" y="4.75" width="12.5" height="10.5" rx="2"/><path d="M6.75 8.5l2 1.5-2 1.5M10.5 11.75h3"/>',
  folder: '<path d="M3.75 6.25a1 1 0 0 1 1-1h3.2l1.5 1.5h5.8a1 1 0 0 1 1 1v6.75a1 1 0 0 1-1 1H4.75a1 1 0 0 1-1-1z"/>',
  // The same folder open: its front leaf let down before it.
  "folder-open": '<path d="M3.75 13.5V6.25a1 1 0 0 1 1-1h3.2l1.5 1.5h5.3a1 1 0 0 1 1 1v1"/><path d="M3.75 14.75l1.85-4.9a1 1 0 0 1 .94-.65h9.4a.7.7 0 0 1 .65.95l-1.6 4.25a1 1 0 0 1-.94.65H4.25a.5.5 0 0 1-.5-.3z"/>',
  fork: '<circle cx="6" cy="5" r="1.6"/><circle cx="14" cy="5" r="1.6"/><circle cx="10" cy="15.2" r="1.6"/><path d="M6 6.6v.9a2.5 2.5 0 0 0 2.5 2.5h3a2.5 2.5 0 0 0 2.5-2.5v-.9M10 10v3.6"/>',
  info: '<circle cx="10" cy="10" r="6.25"/><path d="M10 9.25v4"/><circle cx="10" cy="6.9" r="0.6" fill="currentColor"/>',
  // A speech balloon with a question in it: something asked of the user.
  ask: '<path d="M5.75 4.25h8.5a1.5 1.5 0 0 1 1.5 1.5v6a1.5 1.5 0 0 1-1.5 1.5H9.75l-3.5 2.5v-2.5h-.5a1.5 1.5 0 0 1-1.5-1.5v-6a1.5 1.5 0 0 1 1.5-1.5z"/><path d="M8.6 7.35a1.5 1.5 0 0 1 2.9.45c0 1.05-1.5 1.25-1.5 2.15"/><circle cx="10" cy="11.55" r="0.55" fill="currentColor"/>',
  regenerate: '<path d="M15.25 10a5.25 5.25 0 1 1-1.54-3.71"/><path d="M14.25 3.75v2.75H11.5"/>',
  terminal: '<rect x="3.25" y="4.25" width="13.5" height="11.5" rx="1.75"/><path d="M6.25 8l2.25 2-2.25 2M10.25 12.5h3.5"/>',
  cross: '<path d="M6 6l8 8M14 6l-8 8"/>',
  fit: '<path d="M4 7.5V4.75a.75.75 0 0 1 .75-.75H7.5M12.5 4h2.75a.75.75 0 0 1 .75.75V7.5M16 12.5v2.75a.75.75 0 0 1-.75.75H12.5M7.5 16H4.75a.75.75 0 0 1-.75-.75V12.5"/>',
  plus: '<path d="M10 4.5v11M4.5 10h11"/>',
  minus: '<path d="M4.5 10h11"/>',
  settings: '<circle cx="10" cy="10" r="2.4"/><path d="M10 3.5v1.6M10 14.9v1.6M16.5 10h-1.6M5.1 10H3.5M14.6 5.4l-1.1 1.1M6.5 13.5l-1.1 1.1M14.6 14.6l-1.1-1.1M6.5 6.5L5.4 5.4"/>',
  sidebar: '<rect x="3.75" y="4.75" width="12.5" height="10.5" rx="1.5"/><path d="M8.25 4.75v10.5"/>',
  machine: '<rect x="3.75" y="4.25" width="12.5" height="8.75" rx="1.25"/><path d="M7.5 15.75h5M10 13v2.75"/>',
  close: '<path d="M5.5 5.5l9 9M14.5 5.5l-9 9"/>',
  more: '<circle cx="5.5" cy="10" r="0.6" fill="currentColor"/><circle cx="10" cy="10" r="0.6" fill="currentColor"/><circle cx="14.5" cy="10" r="0.6" fill="currentColor"/>',
  back: '<path d="M11.5 5.5L7 10l4.5 4.5"/>',
  sleep: '<path d="M15 12.4A5.75 5.75 0 0 1 7.6 5a5.75 5.75 0 1 0 7.4 7.4z"/>',
  wake: '<path d="M3.75 14.75h12.5"/><path d="M6.5 14.75a3.5 3.5 0 0 1 7 0"/><path d="M10 6v2M5.4 8.4l1.2 1.2M14.6 8.4l-1.2 1.2"/>',
  move: '<path d="M3.75 6.25a1 1 0 0 1 1-1h3.2l1.5 1.5h5.8a1 1 0 0 1 1 1v6.75a1 1 0 0 1-1 1H4.75a1 1 0 0 1-1-1z"/><path d="M7.75 11h4.5M10.5 9.25L12.25 11l-1.75 1.75"/>',
  reread: '<path d="M15.25 10a5.25 5.25 0 1 1-1.54-3.71"/><path d="M14.25 3.75v2.75H11.5"/>',
  grip: '<circle cx="7.5" cy="5.5" r="0.9" fill="currentColor"/><circle cx="12.5" cy="5.5" r="0.9" fill="currentColor"/><circle cx="7.5" cy="10" r="0.9" fill="currentColor"/><circle cx="12.5" cy="10" r="0.9" fill="currentColor"/><circle cx="7.5" cy="14.5" r="0.9" fill="currentColor"/><circle cx="12.5" cy="14.5" r="0.9" fill="currentColor"/>',
  scan: '<path d="M3.75 7V4.75a1 1 0 0 1 1-1H7M13 3.75h2.25a1 1 0 0 1 1 1V7M16.25 13v2.25a1 1 0 0 1-1 1H13M7 16.25H4.75a1 1 0 0 1-1-1V13"/><path d="M6.25 10h7.5"/>',
  archive: '<rect x="3.75" y="4.75" width="12.5" height="3" rx="0.75"/><path d="M5 7.75v6.5a1 1 0 0 0 1 1h8a1 1 0 0 0 1-1v-6.5"/><path d="M8.25 10.75h3.5"/>',
  unarchive: '<rect x="3.75" y="4.75" width="12.5" height="3" rx="0.75"/><path d="M5 7.75v6.5a1 1 0 0 0 1 1h8a1 1 0 0 0 1-1v-6.5"/><path d="M10 13.75v-3.5M8.4 11.75L10 10.15l1.6 1.6"/>',
  trash: '<path d="M4.25 6h11.5"/><path d="M8 6V4.75a.75.75 0 0 1 .75-.75h2.5a.75.75 0 0 1 .75.75V6"/><path d="M5.6 6l.65 9.1a1 1 0 0 0 1 .9h5.5a1 1 0 0 0 1-.9L14.4 6"/><path d="M8.6 8.75v4.5M11.4 8.75v4.5"/>',
  save: '<path d="M10 3.75v8.5M6.75 9l3.25 3.25L13.25 9"/><path d="M4.25 13v2.25a1 1 0 0 0 1 1h9.5a1 1 0 0 0 1-1V13"/>',
  link: '<path d="M8.5 11.5l3-3"/><path d="M7.4 9.1 5.95 10.55a2.3 2.3 0 0 0 3.25 3.25l1.45-1.45"/><path d="M12.6 10.9l1.45-1.45a2.3 2.3 0 0 0-3.25-3.25L9.35 7.65"/>',
  page: '<rect x="3.75" y="4.25" width="12.5" height="11.5" rx="1.5"/><path d="M3.75 7.25h12.5"/><path d="M6.25 13.25l2.25-2.5 1.75 1.5 3.5-3.5"/>',
  versions: '<rect x="6.25" y="6.75" width="10" height="9" rx="1.25"/><path d="M3.75 13.25v-7.5a1 1 0 0 1 1-1h8.5"/>',
  read: '<path d="M4.5 4.5h7l4 4v7a1 1 0 0 1-1 1h-10a1 1 0 0 1-1-1v-10a1 1 0 0 1 1-1z"/><path d="M11.5 4.5v4h4"/><path d="M7 11h6M7 13.5h4"/>',
  edit: '<path d="M12.6 4.6l2.8 2.8-8.2 8.2H4.4v-2.8z"/><path d="M11 6.2l2.8 2.8"/>',
  command: '<path d="M4.5 6l3.5 4-3.5 4"/><path d="M10 14.5h5.5"/>',
  search: '<circle cx="9" cy="9" r="4.25"/><path d="M12.2 12.2l3.6 3.6"/>',
  image: '<rect x="3.75" y="4.75" width="12.5" height="10.5" rx="1.5"/><circle cx="7.75" cy="8.5" r="1.25"/><path d="M4 13.5l3.5-3 2.8 2.4 2.2-1.9 3.5 2.8"/>',
  colleague: '<circle cx="7.5" cy="7.5" r="2.5"/><path d="M3.5 15.5c.6-2.4 2.1-3.6 4-3.6s3.4 1.2 4 3.6"/><circle cx="13.5" cy="8.5" r="2"/><path d="M13.2 12.1c1.6 0 2.8.9 3.3 2.9"/>',
  web: '<circle cx="10" cy="10" r="6.25"/><path d="M3.75 10h12.5M10 3.75c1.8 1.8 2.6 3.9 2.6 6.25S11.8 14.45 10 16.25C8.2 14.45 7.4 12.35 7.4 10S8.2 5.55 10 3.75z"/>',
  history: '<path d="M4.7 10a5.3 5.3 0 1 0 1.5-3.7"/><path d="M4.4 4.3v2.5h2.5"/><path d="M10 7.3V10l1.9 1.3"/>',
  tasks: '<path d="M4.5 6l1.2 1.2L8 5"/><path d="M10.5 6.2h5"/><path d="M4.5 12l1.2 1.2L8 11"/><path d="M10.5 12.2h5"/>',
  tool: '<path d="M12.4 4.2a3.4 3.4 0 0 0-4 4.4l-4.2 4.2 2.9 2.9 4.2-4.2a3.4 3.4 0 0 0 4.4-4l-2 2-1.9-.5-.5-1.9z"/>',
  delegate: '<path d="M5.5 3.75v12.5"/><path d="M5.5 7.75c0 3.4 1.9 5.1 5.4 5.1"/><circle cx="13.6" cy="12.85" r="2.35"/>',
  // Two small four-pointed stars, filled: thinking, in the line's colour.
  thinking: '<path d="M8 5.2Q8.7 10.8 14.3 11.5 8.7 12.2 8 17.8 7.3 12.2 1.7 11.5 7.3 10.8 8 5.2z" fill="currentColor" stroke="none"/><path d="M14.9 1.9Q15.25 4.4 17.75 4.75 15.25 5.1 14.9 7.6 14.55 5.1 12.05 4.75 14.55 4.4 14.9 1.9z" fill="currentColor" stroke="none"/>',
  chevron: '<path d="M7.5 8.5l2.5 2.5 2.5-2.5"/>',
  copy: '<rect x="7" y="7" width="8.5" height="8.5" rx="1.5"/><path d="M5 12.5h-.5a1 1 0 0 1-1-1v-7a1 1 0 0 1 1-1h7a1 1 0 0 1 1 1V5"/>',
  check: '<path d="M5 10.5l3.2 3 6.8-7"/>',
  expand: '<path d="M11.5 4.5h4v4M8.5 15.5h-4v-4M15.5 4.5L11 9M4.5 15.5L9 11"/>',
  stop: '<rect x="6" y="6" width="8" height="8" rx="1.5" fill="currentColor" stroke="none"/>',
  send: '<path d="M10 15.5v-11M5.5 9L10 4.5 14.5 9"/>',
  guide: '<path d="M15.5 10h-11M9 5.5L4.5 10 9 14.5"/>',
  attach: '<path d="M14.2 9.6l-4.6 4.6a3.1 3.1 0 0 1-4.4-4.4l5.3-5.3a2.1 2.1 0 0 1 3 3l-5.2 5.2a1 1 0 0 1-1.5-1.5l4.6-4.6"/>',
  file: '<path d="M5.5 3.5h5.5l3.5 3.5v8.5a1 1 0 0 1-1 1h-8a1 1 0 0 1-1-1v-11a1 1 0 0 1 1-1z"/><path d="M11 3.5v3.5h3.5"/>',
  person: '<circle cx="10" cy="7.25" r="2.75"/><path d="M4.75 16c.7-3 2.6-4.5 5.25-4.5s4.55 1.5 5.25 4.5"/>',
  key: '<circle cx="7" cy="10" r="2.75"/><path d="M9.75 10h6.5M14 10v2.25M16.25 10v1.5"/>',
  phone: '<rect x="6" y="3.25" width="8" height="13.5" rx="1.75"/><path d="M9 14.25h2"/>',
  logout: '<path d="M8 4.25H5.25a1 1 0 0 0-1 1v9.5a1 1 0 0 0 1 1H8"/><path d="M11.5 6.75L14.75 10l-3.25 3.25M14.5 10H8"/>',
  shield: '<path d="M10 3.75l5.25 2v4c0 3.2-2.2 5.4-5.25 6.5-3.05-1.1-5.25-3.3-5.25-6.5v-4z"/>',
  lock: '<rect x="5" y="9" width="10" height="7.25" rx="1.5"/><path d="M7.25 9V6.75a2.75 2.75 0 0 1 5.5 0V9"/>',
  unlock: '<rect x="5" y="9" width="10" height="7.25" rx="1.5"/><path d="M7.25 9V6.75a2.75 2.75 0 0 1 5.3-1.05"/>',
  signup: '<circle cx="8.5" cy="7.25" r="2.75"/><path d="M3.25 16c.7-3 2.6-4.5 5.25-4.5 1.25 0 2.3.33 3.2 1"/><path d="M15 11.25v5M12.5 13.75h5"/>',
  // Three lines shortening down: how much the conversation holds.
  context: '<path d="M4 6h12M4 10h9M4 14h5"/>',
  /* A sheet written to someone: the account's own prompt. */
  note: '<path d="M5.25 3.75h6.25l3.25 3.25v9.25h-9.5z"/><path d="M11.5 3.75V7h3.25"/><path d="M7.75 10.25h4.5M7.75 13h3"/>',
  /* A plug on its cord: an MCP server. */
  plug: '<path d="M7.75 3.75V6.5M12.25 3.75V6.5"/><path d="M5.75 6.5h8.5v2.25a4.25 4.25 0 0 1-8.5 0z"/><path d="M10 13v3.25"/>',
  /* A pennant on its staff: a goal the conversation keeps working at. */
  goal: '<path d="M6 16.25V3.75"/><path d="M6 4.5h8.5l-2 3.25 2 3.25H6"/>',
  pause: '<path d="M7.5 5.5v9M12.5 5.5v9"/>',
  resume: '<path d="M7 5.1v9.8l7.6-4.9z"/>',
  /* An eye kept open: a sentinel watching for something. */
  sentinel: '<path d="M2.9 10c1.7-3 4.1-4.75 7.1-4.75s5.4 1.75 7.1 4.75c-1.7 3-4.1 4.75-7.1 4.75S4.6 13 2.9 10z"/><circle cx="10" cy="10" r="2.1"/>',
  /* A notebook laid open: a journal of a project, written once. */
  journal: '<path d="M10 5.4C8.5 4.4 6.5 4 4 4.25v10.5c2.5-.25 4.5.15 6 1.15 1.5-1 3.5-1.4 6-1.15V4.25c-2.5-.25-4.5.15-6 1.15z"/><path d="M10 5.4v10.5"/>',
  /* A sheet with a seal at its foot: a decision, kept in its versions. */
  spec: '<path d="M5.25 3.75h9.5v8.5"/><path d="M5.25 3.75v12.5h5.5"/><path d="M7.75 7.25h4.5M7.75 9.75h4.5"/><circle cx="13.75" cy="15" r="1.9"/>',
  /* A wrench: a machine's Fastctx mended layer by layer. */
  repair: '<path d="M13.2 3.9a3.5 3.5 0 0 0-4.4 4.4l-5 5a1.35 1.35 0 0 0 1.9 1.9l5-5a3.5 3.5 0 0 0 4.4-4.4l-2.1 2.1-1.8-.1-.1-1.8z"/>',
  /* A window with a leaf at its right: the panel beside a conversation. */
  panel: '<rect x="3.75" y="4.75" width="12.5" height="10.5" rx="1.5"/><path d="M11.75 4.75v10.5"/>',
  /* Two sliders: preferences. */
  tune: '<path d="M3.75 6.75h6.5M14.75 6.75h1.5M3.75 13.25h1.5M9.75 13.25h6.5"/><circle cx="12.5" cy="6.75" r="2"/><circle cx="7.5" cy="13.25" r="2"/>',
  // A cylinder: what the provider keeps of the prompt.
  cache: '<ellipse cx="10" cy="5.6" rx="5.4" ry="2.1"/><path d="M4.6 5.6v8.8c0 1.16 2.42 2.1 5.4 2.1s5.4-.94 5.4-2.1V5.6"/><path d="M4.6 10c0 1.16 2.42 2.1 5.4 2.1s5.4-.94 5.4-2.1"/>',
  tree: '<circle cx="10" cy="4.3" r="1.6"/><path d="M10 5.9v2.6M4.5 8.5h11M4.5 8.5v2.6M10 8.5v2.6M15.5 8.5v2.6"/><circle cx="4.5" cy="13.4" r="2.1"/><circle cx="10" cy="13.4" r="2.1"/><circle cx="15.5" cy="13.4" r="2.1"/>',
  /* What was said, two lines, with the steps between them folded to
     three dots: a conversation's view of every step (`world-voice.md`). */
  // Where a conversation's side stands: a window with its side ruled off
  // at the right, at the foot, or at the left.
  "dock-right": '<rect x="3.75" y="4.25" width="12.5" height="11.5" rx="1.5"/><path d="M11.75 4.25v11.5"/>',
  "dock-bottom": '<rect x="3.75" y="4.25" width="12.5" height="11.5" rx="1.5"/><path d="M3.75 11.25h12.5"/>',
  "dock-left": '<rect x="3.75" y="4.25" width="12.5" height="11.5" rx="1.5"/><path d="M8.25 4.25v11.5"/>',
  steps: '<path d="M3.75 5h12.5M3.75 15h12.5"/><circle cx="7" cy="10" r="0.9" fill="currentColor"/><circle cx="10" cy="10" r="0.9" fill="currentColor"/><circle cx="13" cy="10" r="0.9" fill="currentColor"/>',
  rows: '<circle cx="5" cy="5.75" r="0.9" fill="currentColor"/><circle cx="5" cy="10" r="0.9" fill="currentColor"/><circle cx="5" cy="14.25" r="0.9" fill="currentColor"/><path d="M8.25 5.75h7.5M8.25 10h7.5M8.25 14.25h7.5"/>',
  clock: '<circle cx="10" cy="10" r="6.25"/><path d="M10 6.4V10l2.5 1.6"/>',
  relay: '<circle cx="4.5" cy="10" r="1.5"/><circle cx="15.5" cy="10" r="1.5"/><path d="M6 10h2.25M11.75 10H14"/><rect x="8.25" y="8" width="3.5" height="4" rx="0.75"/>',
  // Where a folder walk starts: a machine's home, and each of its disks.
  home: '<path d="M4 9.25L10 4.25l6 5"/><path d="M5.75 8v7.25a.75.75 0 0 0 .75.75h7a.75.75 0 0 0 .75-.75V8"/><path d="M8.5 16v-3.5h3V16"/>',
  disk: '<rect x="3.75" y="6.25" width="12.5" height="7.5" rx="1.5"/><path d="M6.25 10h4.5"/><circle cx="13.4" cy="10" r="0.65" fill="currentColor"/>',
  test: '<path d="M7.75 3.75h4.5"/><path d="M8.6 3.75v4.4l-3.9 6.4a1.3 1.3 0 0 0 1.1 2h8.4a1.3 1.3 0 0 0 1.1-2l-3.9-6.4v-4.4"/><path d="M6.4 12.25h7.2"/>',
  commit: '<circle cx="10" cy="10" r="2.9"/><path d="M3 10h4.1M12.9 10H17"/>',
  push: '<path d="M10 13V4.25M6.5 7.75L10 4.25l3.5 3.5"/><path d="M4.75 16.25h10.5"/>',
  // A hook from the row above into this one.
  fed: '<path d="M6 4.5v6a2 2 0 0 0 2 2h7.5"/><path d="M12.75 9.75l2.75 2.75-2.75 2.75"/>',
  camera: '<path d="M3.75 7.5a1 1 0 0 1 1-1h2.1l1.2-1.75h3.9l1.2 1.75h2.1a1 1 0 0 1 1 1v7a1 1 0 0 1-1 1H4.75a1 1 0 0 1-1-1z"/><circle cx="10" cy="10.75" r="2.6"/>',
};

/** A glyph at its nominal `size`; the interface draws every glyph at that
 *  size times one scale (`--icon-scale` in the design tokens), so the set
 *  grows together and keeps its proportions. */
export function glyph(name: keyof typeof paths, size = 20): string {
  return `<svg class="glyph" width="${size}" height="${size}" style="--glyph:${size}" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${paths[name]}</svg>`;
}
