# Copilot (VS Code)

Version: VS Code 1.117.0 / 10c8e557c8b9f9ed0a87f61f1c9a44bde731c409. Inspected 2026-11-02.

Created by the actual VS Code --add-mcp CLI with isolated user-data/extensions directories and Node network APIs blocked. No GUI or model was started.

Upstream: https://code.visualstudio.com/docs/agents/reference/mcp-configuration

Only lab-specific absolute paths and the lab Fastctx server key have been normalized. The server is named `lab-existing` here so the round-trip also checks that a neighboring service survives. No real credentials, accounts or machine names are included. The fixture tests configuration editing, not an authenticated model round-trip.
