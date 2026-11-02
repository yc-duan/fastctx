# Kiro IDE

Version: IDE 1.4.62 / bundled agent 1.1.641. Inspected 2026-11-02.

Source-derived fixture. Unpacked loader probes confirmed that even empty permissions.yaml hides permissions.json; no shell edit is claimed.

Upstream: https://kiro.dev/docs/mcp/configuration/

Only lab-specific absolute paths and the lab Fastctx server key have been normalized. The server is named `lab-existing` here so the round-trip also checks that a neighboring service survives. No real credentials, accounts or machine names are included. The fixture tests configuration editing, not an authenticated model round-trip.
