# Claude Desktop

Version: 1.4758.0. Inspected 2026-11-02.

Schema-derived fixture from the installed official app bundle, not a startup capture. Bundle SHA256: 58ed90c05ad11970755ecd364099c45e41510f9baeeed13be1a503a7ab98d2b6. Its userData loader reads pure JSON; no global instruction or shell switch is inferred from other Claude products.

Upstream: https://support.claude.com/en/articles/10949351-getting-started-with-local-mcp-servers-on-claude-desktop

Only lab-specific absolute paths and the lab Fastctx server key have been normalized. The server is named `lab-existing` here so the round-trip also checks that a neighboring service survives. No real credentials, accounts or machine names are included. The fixture tests configuration editing, not an authenticated model round-trip.
