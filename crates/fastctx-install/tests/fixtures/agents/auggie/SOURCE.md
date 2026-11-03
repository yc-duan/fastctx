# Auggie

Version: 0.36.0 / 7c61e5bb. Inspected 2026-11-02.

Created by the unmodified official npm CLI mcp add, with network APIs blocked. The native shell is launch-process in this stable version, not the name in later preview documentation.

Upstream: https://docs.augmentcode.com/cli/settings

Only lab-specific absolute paths and the lab Fastctx server key have been normalized. The server is named `lab-existing` here so the round-trip also checks that a neighboring service survives. No real credentials, accounts or machine names are included. The fixture tests configuration editing, not an authenticated model round-trip.
