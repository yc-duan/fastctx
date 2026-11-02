# Zed

Version: 1.28.0 / 504a7f2cf25b817cda3998cab93473adef15f93c. Inspected 2026-11-02.

Source-derived editor fixture, not a runtime capture. The settings schema and terminal always_deny handling were read in the fixed source.

Upstream: https://github.com/zed-industries/zed/blob/504a7f2cf25b817cda3998cab93473adef15f93c/crates/agent/src/tool_permissions.rs

Only lab-specific absolute paths and the lab Fastctx server key have been normalized. The server is named `lab-existing` here so the round-trip also checks that a neighboring service survives. No real credentials, accounts or machine names are included. The fixture tests configuration editing, not an authenticated model round-trip.
