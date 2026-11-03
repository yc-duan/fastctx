# Qoder CLI

Version: @qoder-ai/qodercli 1.1.81. Collected on Windows x64, 2026-11-03 UTC.

The unmodified official CLI generated the first user MCP entry with `qodercli mcp add -s user lab-echo node <local-echo-script>` and connected to it over stdio. A researcher then added an unrelated user setting and a JSONC comment. The official CLI added `lab-second`, preserving both the comment and the first service. This fixture is that program-written file before any Fastctx integration. The fresh home had no settings or AGENTS.md; no default configuration was invented.

Official package: https://registry.npmjs.org/@qoder-ai/qodercli/-/qodercli-1.1.81.tgz
Configuration: https://docs.qoder.com/cli/settings

Only the local test script path is normalized. The original capture is `samples/qoder/program-writeback.json` in the local agents lab. No Fastctx service, instruction marker, credentials or shell deny rules are present. Tests therefore exercise adding and withdrawing the complete integration. Permission probes used the published functions without a model: several distinct shell tools need separate deny entries, which the current single-edit adapter cannot express without replacing the user's deny list. Identity remains unknown because local account data is encrypted and environment credentials can override it.
