# Continue (IDE)

Version: 1.3.40 / 5522c6f44ca0ac3528b37244818fbfa39b5af470. Inspected 2026-11-02.

Source-derived JSONC example accepted by the actual global loader/schema/converter with local filesystem mocks. This is the IDE loader; the CLI does not load this directory. No transport or model was started.

Upstream: https://github.com/continuedev/continue/blob/5522c6f44ca0ac3528b37244818fbfa39b5af470/core/context/mcp/json/loadJsonMcpConfigs.ts

Only lab-specific absolute paths and the lab Fastctx server key have been normalized. The server is named `lab-existing` here so the round-trip also checks that a neighboring service survives. No real credentials, accounts or machine names are included. The fixture tests configuration editing, not an authenticated model round-trip.
