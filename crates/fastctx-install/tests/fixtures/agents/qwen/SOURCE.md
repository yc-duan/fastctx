# Qwen Code

Version: 0.26.6 / 042c023b7beb4ad2a9ef95b2906b3564e3f23089. Inspected 2026-11-02.

Researcher supplied the first MCP. The official CLI connected to the local echo server and added a second entry and $version, preserving comments. Published permission-manager code was exercised offline: Bash disabled, MCP retained.

Upstream: https://github.com/QwenLM/qwen-code/tree/v0.26.6

Only lab-specific absolute paths and the lab Fastctx server key have been normalized. The server is named `lab-existing` here so the round-trip also checks that a neighboring service survives. No real credentials, accounts or machine names are included. The fixture tests configuration editing, not an authenticated model round-trip.
