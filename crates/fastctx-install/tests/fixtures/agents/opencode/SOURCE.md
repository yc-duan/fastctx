# OpenCode

Version: 1.19.5 / 810c0eba1a7348036f8bdf61e6632d0c36c6876c. Inspected 2026-11-02.

Researcher supplied the first MCP and bash deny, then the official Windows CLI read it, completed a local echo MCP handshake and added a second entry while preserving comments.

Upstream: https://github.com/anomalyco/opencode/tree/v1.19.5

Only lab-specific absolute paths and the lab Fastctx server key have been normalized. The server is named `lab-existing` here so the round-trip also checks that a neighboring service survives. No real credentials, accounts or machine names are included. The fixture tests configuration editing, not an authenticated model round-trip.

The item that turns its shell off was taken out of the sample, so that it stands as before Fastctx: connecting puts it in, and taking out takes it away.
