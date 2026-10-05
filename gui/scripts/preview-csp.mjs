// Serves the built interface under the content policy the shell enforces,
// which the dev server does not: a resource the policy refuses (a blob:
// picture, a frame, a worker) shows up only in a release otherwise. The
// policy is the shell's own, plus the hash of each inline script, as the
// shell adds when it serves the page. Run `npm run build` first, then shoot
// against it:
//
//   node scripts/preview-csp.mjs [--port 5318]
//   node scripts/shoot.mjs --url http://localhost:5318

import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { preview } from "vite";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const at = process.argv.indexOf("--port");
const port = at > 0 ? Number(process.argv[at + 1]) : 5318;

const config = JSON.parse(readFileSync(join(root, "src-tauri", "tauri.conf.json"), "utf8"));
const html = readFileSync(join(root, "dist", "index.html"), "utf8");
const hashes = [...html.matchAll(/<script(?![^>]*\bsrc=)[^>]*>([\s\S]*?)<\/script>/g)].map(
  ([, body]) => `'sha256-${createHash("sha256").update(body).digest("base64")}'`,
);
const policy = config.app.security.csp.replace("script-src 'self'", ["script-src 'self'", ...hashes].join(" "));

const server = await preview({
  root,
  preview: { port, strictPort: true, headers: { "Content-Security-Policy": policy } },
});
server.printUrls();
console.log(`under: ${policy}`);
