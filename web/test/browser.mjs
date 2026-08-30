// Run the browser test in a real browser.
//
// The requirement is explicit that this opens a real index file over the byte
// source chosen in R2.2, not a stub -- so this launches Chromium, serves the
// fixture over http://localhost (a secure context, which OPFS requires),
// spawns a Web Worker, downloads the index into the Origin Private File
// System, and reads it back through a `FileSystemSyncAccessHandle`.
//
// It then runs the identical assertions over an in-memory source, so a
// failure says whether the problem is in the reader or in the OPFS seam.
//
//   node web/test/browser.mjs
//
// Exits non-zero if any assertion fails, so it can gate a commit.

import { createServer } from "node:http";
import { readFile, stat } from "node:fs/promises";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";

const here = fileURLToPath(new URL(".", import.meta.url));
const root = join(here, "..");

const TYPES = {
  ".html": "text/html",
  ".mjs": "text/javascript",
  ".js": "text/javascript",
  ".json": "application/json",
  ".wasm": "application/wasm",
  ".supdb": "application/octet-stream",
};

function serve(dir) {
  const server = createServer(async (req, res) => {
    try {
      const rel = normalize(decodeURIComponent(req.url.split("?")[0])).replace(
        /^(\.\.[/\\])+/,
        "",
      );
      const path = join(dir, rel);
      const s = await stat(path);
      if (!s.isFile()) throw new Error("not a file");
      const body = await readFile(path);
      const type = TYPES[extname(path)] ?? "application/octet-stream";
      // Range support, because the cached byte source reads the fixture the
      // way it would read S3: by ranged GET, never whole. Single ranges
      // only -- that is all a range fetcher sends -- and out-of-bounds asks
      // get the 416 a real object store would give.
      const range = /^bytes=(\d+)-(\d+)$/.exec(req.headers.range ?? "");
      if (range) {
        const a = Number(range[1]);
        const b = Math.min(Number(range[2]), body.length - 1);
        if (a >= body.length || a > b) {
          res
            .writeHead(416, { "content-range": `bytes */${body.length}` })
            .end();
          return;
        }
        res.writeHead(206, {
          "content-type": type,
          "content-length": b - a + 1,
          "content-range": `bytes ${a}-${b}/${body.length}`,
        });
        res.end(body.subarray(a, b + 1));
        return;
      }
      res.writeHead(200, {
        "content-type": type,
        "content-length": body.length,
      });
      res.end(body);
    } catch {
      res.writeHead(404).end("not found");
    }
  });
  return new Promise((resolve) =>
    server.listen(0, "127.0.0.1", () =>
      resolve({ server, port: server.address().port }),
    ),
  );
}

// playwright is installed globally here; resolve it from the global root
// rather than vendoring a node_modules into this repository.
async function chromium() {
  const require = createRequire(import.meta.url);
  const roots = [
    process.env.NODE_PATH,
    "/opt/node22/lib/node_modules",
    "/usr/lib/node_modules",
    "/usr/local/lib/node_modules",
  ].filter(Boolean);
  for (const r of roots) {
    try {
      return require(join(r, "playwright")).chromium;
    } catch {
      /* try the next one */
    }
  }
  throw new Error(
    "playwright not found. Install it, or set NODE_PATH to a global node_modules",
  );
}

async function main() {
  const { server, port } = await serve(root);
  const launcher = await chromium();
  const browser = await launcher.launch({
    args: ["--no-sandbox", "--disable-dev-shm-usage"],
  });
  const page = await browser.newPage();
  const console_lines = [];
  page.on("console", (m) => console_lines.push(`[${m.type()}] ${m.text()}`));
  page.on("pageerror", (e) => console_lines.push(`[pageerror] ${e.message}`));

  let result;
  try {
    await page.goto(`http://127.0.0.1:${port}/test/page.html`);
    await page.waitForFunction("window.__supdbDone === true", null, {
      timeout: 60_000,
    });
    result = await page.evaluate("window.__supdbResult");
  } finally {
    await browser.close();
    server.close();
  }

  for (const line of result.log) console.log(line);
  if (console_lines.length) {
    console.log("--- browser console ---");
    for (const l of console_lines) console.log(l);
  }
  if (!result.ok) {
    console.error(`\n${result.fail.length} browser assertion(s) failed`);
    process.exit(1);
  }
  console.log(`\nOK: ${result.log.length} browser assertions passed`);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
