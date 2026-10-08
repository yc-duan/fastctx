//! What a session the user opens is told about its working environment, in
//! its host's guidance file (`world-run.md`, 模型怎样知道): the user's
//! machines, that `run` is its shell on all of them and its way to look at
//! and change files on this machine too, since the host's own shell and image
//! viewer are off; and the one rule a session needs before it touches
//! anything, that it enters a project only when the user says.
//!
//! A host may list a server's tools by name only until they are used, so
//! these words alone must lead a model to the tools.

/// The block of guidance for a host's sessions.
pub fn guidance() -> String {
    "\
## Your machines and tools

The user has several machines, connected by fastctx: this one, and others that `fastctx nodes` lists. You work on all of them through the `run` tool.

`run` is your shell: bash with the GNU tools and `rg` on every machine, Windows included. Do all shell work with it, and use it for files:
- `fastctx view PATH` shows a file: text with line numbers, images, PDFs, Word, PowerPoint and Excel.
- `edit PATH` and `write PATH` change a file and keep its encoding and line endings; `run`'s description shows how.
- `rg` searches.

`run` starts on this machine. To work on another one, write `on MACHINE` before a command, or give it a script:

```bash
on server tail -n 50 /var/log/app.log
on laptop <<'EOF'
cd C:/work/app && cargo test
EOF
```

When the user speaks of one of the machines, work on it this way.

fastctx also keeps the user's ongoing work as projects. A fastctx project is not a folder or a repository: it is a named piece of work that can span several machines. Its record says what it is for and where its code and data are on each machine; its journals hold the history of the work. `fastctx project list` lists them. Enter one with `fastctx project enter NAME` only when the user tells you which project to work on; entering gives you its map.

`ask_colleague` starts a GPT colleague that takes a part of the work and does it on its own while you go on.
"
    .to_string()
}
