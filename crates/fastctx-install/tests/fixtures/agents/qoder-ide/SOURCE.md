# Qoder IDE

Version: Qoder IDE 1.38.0, VS Code base 1.106.3. Inspected on Windows x64, 2026-11-03 UTC.

The official Windows installer was unpacked; its Electron CLI ran `--version` offline. The GUI was not started. This is a hand-authored existing-user MCP configuration, based on the official example and validated by the original packaged `_mcpServerToAcpConfig` method. It is not a claimed GUI-generated capture. The original `_ensureMcpConfigFile` template is an empty `mcpServers` object.

Documentation: https://docs.qoder.com/user-guide/chat/model-context-protocol
Packaged source: `out/vs/workbench/workbench.desktop.main.js`, `_getDefaultLocalMcpConfigFileUri`, `_ensureMcpConfigFile`, `_mcpServerToAcpConfig`. The default file is `~/.qoder/mcp.json`; the CLI uses a separate `settings.json` and its environment override is not assumed to redirect the IDE.

Only the local echo-script path and line endings are normalized. The original capture is `samples/qoder/ide/registered/mcp.json` in the local agents lab. No Fastctx service, credentials or shell settings are present. The fixture tests MCP editing and withdrawal; it does not establish an authenticated GUI session, global instruction loading or shell disabling. The npm CLI also installs the `qoder` command, so that command alone is not evidence that the IDE is installed.
