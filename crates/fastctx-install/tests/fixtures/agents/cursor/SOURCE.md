# Cursor

Version: 3.27.22 / b1aa50cb6db84fd3e8c808259cb172ea73425c1f. Inspected 2026-11-02.

Source-derived example checked against the unpacked official editor parser. Not a runtime capture. Hooks need a required version plus a rule, which one shell edit cannot express.

Upstream: https://cursor.com/docs/hooks

Only lab-specific absolute paths and the lab Fastctx server key have been normalized. The server is named `lab-existing` here so the round-trip also checks that a neighboring service survives. No real credentials, accounts or machine names are included. The fixture tests configuration editing, not an authenticated model round-trip.
