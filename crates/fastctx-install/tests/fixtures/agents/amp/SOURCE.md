# Amp

Version: @ampcode/cli 0.0.1793649628-g18d621. Collected on Windows x64, 2026-11-02.

The official program's own `mcp add` wrote this `settings.json` in an isolated home (its embedded JavaScript run with the keyring stubbed and the network blocked); the server it added is renamed here so that no Fastctx entry is present, and a comment is added, as Amp reads its settings as JSONC. Amp reads `settings.json` alone where it is there, else `settings.jsonc`, never both; MCP servers are under the literal key `amp.mcpServers`.

Upstream: https://ampcode.com/docs/cli/settings
