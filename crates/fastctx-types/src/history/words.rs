//! The history instructions supplied to every host.

pub const DISABLED: &str =
    "Notes are turned off for this account. Enable Notes in Fastctx account settings before creating or changing them.";

/// Why a note is not deleted for good where its hub is of a release before.
pub const DELETION_UNSUPPORTED: &str = "this network's hub must be updated before a note can be deleted for good; \
     write its file empty to take it down instead";

pub const TEMPLATE: &str = r####"HISTORY
This project's history is in <path> on <machine>: one Markdown file per topic, each a
coherent account of that topic as it stands now: how it works, what was decided and why,
what was tried and did not work, what is still open. A topic is one thing someone would
look up as a whole: a mechanism, a component, a recurring trouble, an area of decisions.
The history is the user's own record, not this conversation's and not this computer's:
Fastctx keeps it on all of the user's computers and syncs every change automatically.
Each file starts with
---
name: <the topic: the file's name, in lower-case letters, digits and hyphens>
description: <one line: what it covers, then the words someone would search for it with>
---
Name a topic after the thing it is about, not after what was found. The names and
descriptions of all of them are below; nothing else of the history is in your context.
You find the rest with `rg` and read it with `fastctx view`.

{{ name: description, one a line }}

The history is how this work carries on across days, weeks and months, through many
conversations and many compactions. Write what a later conversation would need and could
not recover from the code, git or these files: what the user decided, in their own words
where the words matter; a finding that took real work; a path that failed and why; and,
when you stop, what is still open and what comes next. It is not a log: what was done is
in git. Write each file to read well from top to bottom as the topic stands now: when
something in it no longer holds, change it or take it out; when a whole topic no longer
matters, write its file empty, which takes it down for the user to bring back if wanted,
or delete it for good, every version of it, with `fastctx history delete <file>`. Search
first: if a file's topic is this very thing, change that file; if none is, start a new
one. Change the files only with `write`, `edit` and `fastctx history delete`; nothing
else reaches the user's other computers, and anything else is undone.

A description is how the next reader finds a topic. Say in plain words what it covers,
then add the words they may search with: how the trouble shows (symptoms, error text), its
names in the code, the general term for it, and the same in the user's other language if
there is one. When a search misses a file that you then find another way, add the words you
searched with to its description."####;
