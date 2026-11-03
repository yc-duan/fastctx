# Trae IDE

Version: IDE 3.6.13 / distro 0a18739abab6297f022e9a1c870feefe54bf8197. Inspected 2026-11-02.

Source-derived JSONC fixture checked against the unpacked editor scanner, not a runtime capture. The profile User directory is distinct from its .trae rules directory.

Upstream: https://docs.trae.ai/ide/model-context-protocol

Only lab-specific absolute paths and the lab Fastctx server key have been normalized. The server is named `lab-existing` here so the round-trip also checks that a neighboring service survives. No real credentials, accounts or machine names are included. The fixture tests configuration editing, not an authenticated model round-trip.
