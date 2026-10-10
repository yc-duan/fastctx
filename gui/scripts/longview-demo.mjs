// Development-only, generated slices: the specimen never materializes a log.
import { setImmediate as yieldIO } from "node:timers/promises";

function line(index, mode) {
  if (mode === "unicode" && index === 17) return "长行 · " + "中文 🧑🏽‍🚀 é العربية אבג 🏳️‍🌈 \t".repeat(5000) + " · END";
  return `${String(index + 1).padStart(7, "0")}  build · 中文 · 🧑🏽‍🚀 · café · é · ${index % 997 === 0 ? "needle" : "working"} · ${(index * 17) % 103} ms`;
}

export function longviewDemo() {
  return {
    name: "longview-specimen-source",
    apply: "serve",
    configureServer(server) {
      server.middlewares.use("/__longview", async (request, response, next) => {
        const url = new URL(request.url, "http://localhost");
        if (url.pathname !== "/lines" && url.pathname !== "/find") return next();
        const length = Number(url.searchParams.get("length"));
        const start = Number(url.searchParams.get("start"));
        const mode = url.searchParams.get("mode") ?? "plain";
        if (mode === "expired") { response.writeHead(410); response.end("Output has expired"); return; }
        if (!Number.isSafeInteger(length) || !Number.isSafeInteger(start) || length < 0) {
          response.writeHead(400); response.end("Invalid line range"); return;
        }
        response.setHeader("Content-Type", "application/json; charset=utf-8");
        response.setHeader("Cache-Control", "no-store");
        if (url.pathname === "/lines") {
          const count = Number(url.searchParams.get("count"));
          if (!Number.isSafeInteger(count) || start < 0 || count < 0 || start + count > length) {
            response.writeHead(400); response.end("Invalid line range"); return;
          }
          response.end(JSON.stringify(Array.from({ length: count }, (_, offset) => line(start + offset, mode))));
          return;
        }
        const query = url.searchParams.get("query") ?? "";
        const direction = Number(url.searchParams.get("direction"));
        const offset = url.searchParams.has("offset") ? Number(url.searchParams.get("offset")) : undefined;
        if (direction !== 1 && direction !== -1) { response.writeHead(400); response.end("Invalid search direction"); return; }
        for (let index = start; index >= 0 && index < length; index += direction) {
          if (response.destroyed) return;
          const value = line(index, mode);
          const at = index === start ? offset : undefined;
          const found = direction === 1 ? value.indexOf(query, at ?? 0) : value.lastIndexOf(query, at ?? Infinity);
          if (found !== -1) { response.end(JSON.stringify({ line: index, start: found, end: found + query.length })); return; }
          await yieldIO();
        }
        response.end("null");
      });
    },
  };
}
