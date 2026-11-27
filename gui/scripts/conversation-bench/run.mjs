// Runs the conversation bench over recorded tapes:
//   node run.mjs [--revision REV] [--rows] [--plain] TAPE.jsonl...
// Prints what each tape found; exits 1 when any found something.
// `--revision` runs the page's code as it was at a git revision, the bench's
// own as it is now; `--plain` only as each tape came, without disturbances.
import { build } from "esbuild";
import { execFileSync } from "node:child_process";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { tmpdir } from "node:os";
import { writeFile, rm } from "node:fs/promises";

const here = path.dirname(fileURLToPath(import.meta.url));
const gui = path.resolve(here, "../..");
const args = process.argv.slice(2);
const option = (name) => { const at = args.indexOf(name); return at === -1 ? null : args[at + 1]; };
const revision = option("--revision");
const bundled = await build({
  entryPoints: [path.join(here, "replay.ts")], bundle: true, write: false, format: "esm", platform: "node",
  nodePaths: [path.join(gui, "node_modules")], define: { "import.meta.env": "{}" }, logLevel: "error",
  plugins: revision ? [{ name: "at-revision", setup(builder) {
    builder.onLoad({ filter: /\.[cm]?[jt]s$/ }, (loaded) => {
      const relative = path.relative(gui, loaded.path).replace(/\\/g, "/");
      if (!relative.startsWith("src/")) return;
      // A file the revision did not have yet, as the rows the bench compares
      // by, is read as it is now.
      let contents;
      try {
        contents = execFileSync("git", ["-C", gui, "show", `${revision}:gui/${relative}`], { encoding: "utf8", maxBuffer: 64 << 20, stdio: ["ignore", "pipe", "ignore"] });
      } catch {
        return;
      }
      return { contents, loader: relative.endsWith(".ts") ? "ts" : "js", resolveDir: path.dirname(loaded.path) };
    });
  } }] : [],
});
const file = path.join(tmpdir(), `conversation-bench-${process.pid}.mjs`);
await writeFile(file, bundled.outputFiles[0].text);
const { replay, disturbed } = await import(pathToFileURL(file).href);
await rm(file, { force: true });
let failed = false;
const verbose = args.includes("--rows");
const plain = args.includes("--plain");
const tapes = args.filter((arg, at) => !arg.startsWith("--") && args[at - 1] !== "--revision");
for (const tapePath of tapes) {
  const lines = (await readFile(tapePath, "utf8")).split("\n").filter(Boolean);
  const tape = lines.map((line) => JSON.parse(line));
  // As it came, and as pages meet it: records late, the live watch broken
  // and opened again, the latest part read anew, each now and then.
  const span = tape.length ? tape[tape.length - 1].t : 0;
  const every = (ms) => Array.from({ length: Math.floor(span / ms) }, (_, index) => (index + 1) * ms);
  const ways = plain ? [["as it came", tape]] : [
    ["as it came", tape],
    ["records 30 s late", disturbed(tape, { recordsLateMs: 30_000 }), 30_000],
    ["records 120 s late", disturbed(tape, { recordsLateMs: 120_000 }), 120_000],
    ["live broken every 40 s", disturbed(tape, { breaks: every(40_000), breakMs: 6_000 })],
    ["read anew every 25 s", disturbed(tape, { rereads: every(25_000) })],
    ["all of these", disturbed(tape, { recordsLateMs: 45_000, breaks: every(37_000), breakMs: 8_000, rereads: every(29_000) }), 45_000],
  ];
  for (const [way, taped, late = 0] of ways) {
  // What comes late is not missing until well after it was to come.
  const report = replay(taped, { recordWithinMs: 90_000 + late });
  console.log(`== ${tapePath} (${way}): ${report.pieces} pieces ${JSON.stringify(report.counts)}`);
  for (const finding of report.findings) console.log(`  [${finding.law}] t=${(finding.at / 1000).toFixed(1)}s ${finding.said}`);
  if (report.end) {
    const anew = new Map(report.end.anew.map((row) => [row.id, row]));
    const live = new Map(report.end.live.map((row) => [row.id, row]));
    // Rows still under way, or whose record is not due yet, are not missing.
    const young = new Set(report.end.young);
    const onlyLive = report.end.live.filter((row) => !anew.has(row.id) && !young.has(row.id) && row.kind !== "group-end" && row.kind !== "gap");
    const onlyAnew = report.end.anew.filter((row) => !live.has(row.id) && row.kind !== "group-end" && row.kind !== "gap");
    const waiting = report.end.live.filter((row) => !anew.has(row.id) && young.has(row.id)).length;
    console.log(`  end: live ${report.end.live.length} rows, anew ${report.end.anew.length}; only live ${onlyLive.length}, only anew ${onlyAnew.length}, not due yet ${waiting}`);
    for (const row of onlyLive.slice(0, 20)) console.log(`    only live: ${row.kind} ${row.said} ${row.id}`);
    for (const row of onlyAnew.slice(0, 20)) console.log(`    only anew: ${row.kind} ${row.said} ${row.id}`);
    if (verbose) {
      console.log("  live tail:"); for (const row of report.end.live.slice(-40)) console.log(`    ${row.kind} ${row.said} ${row.id}`);
      console.log("  anew tail:"); for (const row of report.end.anew.slice(-40)) console.log(`    ${row.kind} ${row.said} ${row.id}`);
    }
    if (onlyLive.length || onlyAnew.length) failed = true;
  }
  if (report.findings.length) failed = true;
  }
}
process.exitCode = failed ? 1 : 0;
