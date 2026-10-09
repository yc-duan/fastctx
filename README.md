# FastCtx

FastCtx is being rebuilt around **World**: one world, many machines. Agents such as Codex and Claude Code live in it as residents of projects that span the user's own machines. Their hands are the same on every machine: a shell, file viewing, file writing. Their history is kept in journals that every later agent can read. Machines talk only through the user's own hub.

## Accounts, and the agents' own terms

A hub keeps accounts. An account can lend its machines, and the agents signed in on them, to other accounts on the same hub; a conversation can work for an account on a machine another account owns. An account is a way of dividing up a World, not a person, and fastctx does not know or check who is behind one.

Codex and Claude Code come with their makers' terms of use, which say who may use a subscription and how. fastctx starts only the official programs, unmodified, signed in as whoever signed in on that machine, and every request an agent makes to its maker goes from the machine it runs on, over that machine's own network: fastctx never relays, tunnels or proxies those requests. Whether lending a machine's agents to another account fits those terms is for you to decide, and we ask everyone who uses fastctx to keep to them.

## Status

This branch is the rebuild, and nothing on it has been released yet. The released version is 0.2.6, on npm as `fastctx` (and `codex-fastctx`) and on GitHub Releases; its source is the `v0.2.6` tag.

Licensed under the Apache License 2.0; see `LICENSE-APACHE` and `NOTICE`.
