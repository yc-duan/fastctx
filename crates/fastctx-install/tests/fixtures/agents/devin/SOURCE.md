# Devin Desktop / Local

Version: CLI 3000.11.234 / 92c83011d113. Inspected 2026-11-02.

Initial config and MCP file created by the official CLI inside an offline Linux network namespace. No model call or credential was used. Shared harness documented by Devin Desktop.

Upstream: https://docs.devin.ai/cli/extensibility/mcp/configuration

Only lab-specific absolute paths and the lab Fastctx server key have been normalized. The server is named `lab-existing` here so the round-trip also checks that a neighboring service survives. No real credentials, accounts or machine names are included. The fixture tests configuration editing, not an authenticated model round-trip.
