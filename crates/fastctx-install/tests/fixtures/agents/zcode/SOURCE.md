# ZCode

Version: desktop 3.17.2, embedded CLI 0.17.1; source a60ba6a7e5a06742270ca605f5d2cc2766afa7e1. Collected on Windows x64, 2026-11-03.

`home/cli/config.json` is the hand-written native configuration the packaged CLI read in an isolated home, without its MCP server, so that it holds none; `.agents/mcp.json` beside the home is the shared file the desktop module was run against. While the native `mcp.servers` holds no server, ZCode reads the shared `mcpServers` instead, so Fastctx's server goes there. Paths of the lab's echo script are normalized.

Upstream: https://zcode.z.ai/en/docs/mcp-services
