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

    // The O(extents) form must give the identical answer on a fixed-width
    // posting list. If it ever does not, the arithmetic is wrong and a
    // breakdown panel would be quietly wrong with it.
    const fixed = await rpc(worker, "scanCountsFixed", {
      from: expected.scan.from,
      limit: expected.scan.limit,
      width: expected.posting_bytes,
    });
    check(`${source}: scanCountsFixed agrees with scanCounts`, fixed, expected.scan.rows);

    // A key that is not there answers, rather than throwing.
    check(`${source}: absent key counts zero`, await rpc(worker, "count", { key: "no=such" }), 0);
    check(`${source}: absent key looks up empty`, await rpc(worker, "lookup", { key: "no=such" }), []);

    await rpc(worker, "close");
    worker.terminate();
  }

  await cachedSource();
}

// R6: a reader over ranged HTTP with a cache smaller than the file. The
// index is never downloaded whole -- the open fetches the sections it
// plans, a point read fetches the blocks its key lives in, and the extent
// counts fetch nothing at all. The fixture is segment-shaped on purpose:
// ~100 keys of index over megabytes of data, which is where sparseness
// pays, rather than a wide dictionary that would flatter the index side.
async function cachedSource() {
  const seg = await (await fetch("./out/expected-segment.json")).json();
  const worker = new Worker("../worker.mjs", { type: "module" });
  const opened = await rpc(worker, "open", {
    wasmUrl: new URL("../supdb.wasm", location.href).href,
    indexUrl: new URL("./out/segment.supdb", location.href).href,
    // A fresh cache per run: the cache is named for the object *version*,
    // and the fixture is rebuilt per run.
    name: `segment-${Date.now()}`,
    source: "cached",
    budgetBytes: seg.cache_budget_bytes,
  });

  assert(
    "cached: the budget is smaller than the file, or this proves nothing",
    seg.cache_budget_bytes < seg.file_bytes,
    `budget ${seg.cache_budget_bytes} vs file ${seg.file_bytes}`,
  );
  check("cached: keys", opened.keys, seg.keys);
  check("cached: object length seen over HTTP", opened.length, seg.file_bytes);
  // The up-front cost is the planned open, not the object: superblock probe,
  // key index, block table, log word -- page-rounded. This equality is the
  // "you did not download the file" proof, and the native fixture computed
  // the number from `open_ranges` so it also pins JS paging to the plan.
  check("cached: open fetched exactly its plan", opened.openFetchedBytes, seg.open_fetch_bytes);
  assert(
    "cached: the open fetch is a fraction of the object",
    opened.openFetchedBytes < seg.file_bytes / 4,
    `${opened.openFetchedBytes} of ${seg.file_bytes}`,
  );

  // Extent counts and the dictionary scan: answered from the resident
  // sections, so the cache must not fetch another byte for them.
  const afterOpen = await rpc(worker, "cacheStats");
  for (const p of seg.probes) {
    check(
      `cached: countFixed ${p.key}`,
      await rpc(worker, "countFixed", { key: p.key, width: seg.value_bytes }),
      p.count,
    );
    check(
      `cached: storedBytes ${p.key}`,
      await rpc(worker, "storedBytes", { key: p.key }),
      p.stored_bytes,
    );
  }
  const fixed = await rpc(worker, "scanCountsFixed", {
    from: seg.scan.from,
    limit: seg.scan.limit,
    width: seg.value_bytes,
  });
  check("cached: scanCountsFixed ranks the dictionary", fixed, seg.scan.rows);
  const afterCounts = await rpc(worker, "cacheStats");
  check(
    "cached: counts and the scan fetched nothing",
    afterCounts.fetchedBytes,
    afterOpen.fetchedBytes,
  );

  // Point reads: plan, ensure, then read -- and the values themselves are
  // checked against the native reader through the fixture's FNV hash.
  for (const p of seg.probes) {
    const plan = await rpc(worker, "planRanges", { keys: [p.key] });
    assert(
      `cached: ${p.key} plans its data before reading it`,
      plan.length >= 1 && plan.reduce((n, r) => n + r[1], 0) >= p.stored_bytes,
      JSON.stringify(plan),
    );
    await rpc(worker, "ensure", { keys: [p.key] });
    const got = await rpc(worker, "lookupHash", { key: p.key });
    check(`cached: lookup ${p.key}`, got, { count: p.count, hash: p.value_hash });
    check(`cached: count ${p.key}`, await rpc(worker, "count", { key: p.key }), p.count);
  }

  // An absent key plans nothing, fetches nothing, answers zero.
  await rpc(worker, "ensure", { keys: ["no=such"] });
  check("cached: absent key counts zero", await rpc(worker, "count", { key: "no=such" }), 0);

  const stats = await rpc(worker, "cacheStats");
  assert(
    "cached: total fetched is less than the file",
    stats.fetchedBytes < seg.file_bytes,
    `${stats.fetchedBytes} of ${seg.file_bytes}`,
  );
  assert(
    "cached: total fetched is less than the data region alone",
    stats.fetchedBytes < seg.data_bytes,
    `${stats.fetchedBytes} of ${seg.data_bytes} data bytes`,
  );
  assert(
    "cached: resident bytes never exceed the budget",
    stats.residentBytes <= stats.budgetBytes,
    `${stats.residentBytes} resident vs ${stats.budgetBytes}`,
  );
  assert(
    "cached: the budget evicted, which is what makes it a budget",
    stats.evicted > 0,
    JSON.stringify(stats),
  );

  await rpc(worker, "close");
  worker.terminate();
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
