# Fixture manifest, version 1

`manifest.json` describes immutable synthetic inputs and the answers a consumer must check. It is UTF-8 JSON with `format: 1`. Readers reject other versions. Paths are relative POSIX paths beneath the fixture root. Sizes are bytes; the longest line includes its terminating newline. SHA-256 values are lowercase hexadecimal hashes of exact file bytes.

`total_bytes` is the sum of listed files, excluding `manifest.json` itself. Runtime worlds, timestamps, identities and job metadata are created separately by the drill and are not part of this immutable hash inventory. `files` is ordered: the image, main conversations, child records and metadata, the pathological source, binary assets, then skills. The generator writes the same manifest and files for the same profile, history multiplier and seed.

| Field | Meaning |
|---|---|
| `format`, `seed`, `profile`, `history` | Format version, deterministic seed, workload profile and history multiplier (`1` or `10`) |
| `settings` | Exact profile parameters: conversation count, workload/hot rows, row bytes, longest line, output/attachment bytes and square image side |
| `total_bytes`, `workload_rows`, `expected_events` | File bytes, ordinary source rows, and projected persistent events across all transcript sources |
| `main_bytes`, `subagent_bytes`, `subagent_fraction` | Normal source byte totals and sidechain share; the pathological source is separate |
| `conversations` | Resident/session IDs, source path, two child IDs, ordinary row count, projected event count and optional `colleague_of` resident |
| `files` | File inventory described below |
| `jobs` | Number of synthetic completed job records to create in the isolated world |
| `picture` | The same file entry used for `assets/picture.png`, including dimensions |
| `newest_row` | Numeric UUID suffix of the newest ordinary row of the hot conversation |
| `not_implemented` | Explicitly unimplemented consumers; not a claim that other inputs have been exercised |

Every file entry has `path`, `kind`, `bytes`, and `sha256`. Kinds are `claude`, `subagent`, `metadata`, `image`, `attachment`, `job-output`, and `skill`. Transcript entries also have `records`, `lines`, `longest_line`, `expected_events`, and `uuids`. Subagents name their `agent`; the pathological source has `adversarial: true`. The image adds `width` and `height`.

Source rows and projected events are different answers. Each four-row ordinary cycle contains a user message, tool use, tool result and assistant message. The parser pairs the tool use/result into one event, yielding three persistent events. Each completed child launch contributes two source rows and three persistent events; the picture adds one. The isolated long-line source contributes two rows and one event. The raw subagent-activity API exposes source activities, so that probe checks `records`, while parser calibration checks `expected_events` and `uuids`.

The hot conversation keeps the same newest ordinary rows at both history multipliers. Additional history uses negative UUID suffixes, preserving the visible tail and older-page anchors. Fixed images, attachments and job output do not scale with history, so a tenfold row count need not imply tenfold file bytes. The normal main/sidechain byte ratio is 5:4; source row allocation is rounded to complete four-row cycles.

`calibration.verify` independently rereads the inventory and checks hashes, sizes, record counts, longest lines and the account's row budget. The Rust `--calibrate-scale` entry point checks actual Claude parser events and UUIDs. Neither check rewrites the manifest to make a failed fixture pass.
