# GitHub Copilot CLI

Version: 1.0.105 / 845c22879d4. Inspected 2026-11-02.

Created by the official CLI mcp add with COPILOT_OFFLINE enabled. mcp list confirmed parsing; no MCP handshake or model call is claimed.

Upstream: https://github.com/github/docs/blob/ccee54752cfef83be7f2d18e18b2f74dd191e041/content/copilot/how-tos/copilot-cli/customize-copilot/add-mcp-servers.md

Only lab-specific absolute paths and the lab Fastctx server key have been normalized. The server is named `lab-existing` here so the round-trip also checks that a neighboring service survives. No real credentials, accounts or machine names are included. The fixture tests configuration editing, not an authenticated model round-trip.
