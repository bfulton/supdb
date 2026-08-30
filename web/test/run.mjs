// The browser test, in the browser.
//
// Everything here runs against a real index file written by `logshed build`
// and a real `FileSystemSyncAccessHandle` -- not a stub, which the
// requirements are explicit about, and rightly: a stub would test the framing
// code and nothing about whether the synchronous-read premise of R2.2(a)
// actually holds in a browser.
//
// The expected answers come from the same fixture's `expected.json`, which
// `web/test/browser.mjs` generates by asking the *native* reader. So this is
// the same differential test `tests/blob.rs` runs, carried across the wasm
// boundary and an OPFS handle.

const log = [];
const fail = [];

function check(name, got, want) {
  const g = JSON.stringify(got);
  const w = JSON.stringify(want);
  if (g === w) {
    log.push(`ok   ${name}`);
  } else {
    log.push(`FAIL ${name}\n  got  ${g}\n  want ${w}`);
    fail.push(name);
  }
}

function assert(name, cond, detail) {
  if (cond) log.push(`ok   ${name}`);
  else {
    log.push(`FAIL ${name}: ${detail ?? ""}`);
    fail.push(name);
  }
}

let nextId = 1;
function rpc(worker, op, args) {
  const id = nextId++;
  return new Promise((resolve, reject) => {
    const on = (e) => {
      if (e.data.id !== id) return;
      worker.removeEventListener("message", on);
      if (e.data.error) reject(new Error(e.data.error));
      else resolve(e.data.ok);
    };
    worker.addEventListener("message", on);
    worker.postMessage({ id, op, args });
  });
}

async function main() {
  const expected = await (await fetch("./out/expected.json")).json();

  for (const source of ["opfs", "memory"]) {
    const worker = new Worker("../worker.mjs", { type: "module" });
    // Absolute, because these are resolved inside the worker, whose base URL
    // is the worker script's rather than this page's.
    const opened = await rpc(worker, "open", {
      wasmUrl: new URL("../supdb.wasm", location.href).href,
      indexUrl: new URL("./out/day.supdb", location.href).href,
      name: `day-${source}.supdb`,
      source,
    });
    assert(
      `${source}: opened over the source it was asked for`,
      opened.source === source,
      JSON.stringify(opened),
    );
    if (source === "opfs") {
      assert(
        "opfs: a synchronous access handle over the downloaded object",
        opened.size === expected.file_bytes,
        `handle says ${opened.size}, the file is ${expected.file_bytes}`,
      );
    }

    // R4.5
    check(`${source}: keys`, await rpc(worker, "keys"), expected.keys);
    check(
      `${source}: index bytes`,
      await rpc(worker, "indexBytes"),
      expected.index_bytes,
    );

    // R4.2 -- a real lookup, byte for byte against the native reader.
    for (const c of expected.lookups) {
      const got = await rpc(worker, "lookup", { key: c.key });
      check(`${source}: lookup ${c.key}`, got, c.values);
    }

    // R4.3 -- the count, three ways, all of which must agree.
    for (const c of expected.counts) {
      check(`${source}: count ${c.key}`, await rpc(worker, "count", { key: c.key }), c.count);
      check(
        `${source}: countFixed ${c.key}`,
        await rpc(worker, "countFixed", { key: c.key, width: expected.posting_bytes }),
        c.count,
      );
      check(
        `${source}: storedBytes ${c.key}`,
        await rpc(worker, "storedBytes", { key: c.key }),
        c.stored_bytes,
      );
    }

    // R4.4
    const scanned = await rpc(worker, "scanCounts", {
      from: expected.scan.from,
      limit: expected.scan.limit,
    });
    check(`${source}: scanCounts`, scanned, expected.scan.rows);

    // A key that is not there answers, rather than throwing.
    check(`${source}: absent key counts zero`, await rpc(worker, "count", { key: "no=such" }), 0);
    check(`${source}: absent key looks up empty`, await rpc(worker, "lookup", { key: "no=such" }), []);

    await rpc(worker, "close");
    worker.terminate();
  }
}

main()
  .then(() => {
    window.__supdbResult = { ok: fail.length === 0, fail, log };
  })
  .catch((e) => {
    window.__supdbResult = {
      ok: false,
      fail: ["threw"],
      log: log.concat([String(e.stack ?? e)]),
    };
  })
  .finally(() => {
    document.getElementById("log").textContent = window.__supdbResult.log.join("\n");
    window.__supdbDone = true;
  });
