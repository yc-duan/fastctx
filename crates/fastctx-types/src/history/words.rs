//! The history instructions supplied to every host.

pub const TEMPLATE: &str = r####"HISTORY
This project's history is in <path> on <machine>: one Markdown file per topic, holding
what happened and what was settled about it, oldest first. A topic is one thing someone
would look up as a whole: a mechanism, a component, a recurring trouble, an area of
decisions. The history is the user's own record, not this conversation's and not this
computer's: Fastctx keeps it on all of the user's computers and syncs every change
automatically. Each file starts with
---
name: <the topic: the file's name, in lower-case letters, digits and hyphens>
description: <one line: what it covers, then the words someone would search for it with>
---
Name a topic after the thing it is about, not after what was found. The names and
descriptions of all of them are below; nothing else of the history is in your
context. You find the rest with `rg` and read it with `fastctx view`, so the history
is written for grep, the descriptions most of all.

{{ name: description, one a line }}

The history is how this work carries on across days, weeks and months, through many
conversations and many compactions, so most turns add nothing to it. Write only when
something has become settled that a later conversation would need and could not recover
from the code, git or these files: a decision of the user's, a finding that took real work,
a path that failed and why, and when you stop, what is still open and what comes next. It
is not a log: what was done is in git, and how things stand at this moment will have moved
by the time anyone reads it, so they look again. Routine steps, progress, what you are
about to try, and anything still unsettled are not history yet. Each thing goes in the file
whose topic it is, so one turn may add to several. Search first: if a file's topic is this
very thing, append a paragraph to it; if none is, start a new file for it, however short.
Begin each paragraph with its date and make it whole on its own: grep shows one line, so
name the subject instead of writing "it", and write what someone may search for exactly as
it is: identifiers, paths, error text, versions, commits. The user's decisions go after
"Decided", quoted in their exact words; questions still open begin with "Open". When an
earlier paragraph is no longer true, do not delete it: in that old paragraph, right after
its date, insert "Superseded (see <today's date>)", so that whoever finds it by grep sees
at once that it no longer holds; then append what is true now as a new paragraph. Change
the files only with `write` and `edit`; nothing else reaches the user's other computers,
and anything else is undone. Never rewrite a file wholesale.

A description is how the next reader finds a topic. Say in plain words what it covers,
then add the words they may search with: how the trouble shows (symptoms, error text), its
names in the code, the general term for it, and the same in the user's other language if
there is one. When a search misses a file that you then find another way, add the words you
searched with to its description."####;

pub const READ_ONLY: &str = "READ ONLY: this history is for you to read. Give what should go in it, with the paths of the\nevidence, to the conversation that asked you.";
