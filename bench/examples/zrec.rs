//! The per-record form, probed against the comparators before it is built
//! into the engine.
//!
//! The compact form decodes a group of about fourteen records to read one
//! of them, so a point read pays a seven-kilobyte decode for a record of a
//! hundred and sixteen bytes, and a scan of fifty records decodes the whole
//! of every group it touches. Under the bound at five million keys the
//! decode is the largest cost the form's reads have left, and the cells the
//! engine loses there are E's scans and D's reads
//! (`scratch/wave2/stage4.md`). This probe writes the same records as a form
//! whose records decode on their own -- a group of sixteen records, its
//! first key raw, each record's key front-coded against it (the shared
//! prefix's length and the suffix), each value compressed on its own with
//! LZ4 against a dictionary of the segment's own first values, and a table
//! of record offsets at the group's head -- and runs read-only mixes against
//! it, the current form (`supdb-nocachezpages`, the form forced), LMDB with
//! its readahead off and RocksDB, in one process, arms interleaved within a
//! round. A point read decodes one record; a scan decodes exactly the
//! records it emits.
//!
//! What it does not do: inserts (D's and E's 5%, which is why the mixes are
//! not named after the suite's), the controller, the block cache, any write
//! path. It is a probe of the read structure, and its numbers are the
//! mechanism's, not a row's.
//!
//!   cargo run --release --example zrec -- KEYS ROUNDS OPS ARM,ARM,... [--bound BYTES]
//!
//! Each round: for each arm, load KEYS records (the suite's keys and
//! values), then OPS uniform point reads (the suite's read pass), OPS
//! Zipfian point reads (C's), OPS/10 Zipfian scans of fifty (E's scans
//! without its inserts), each timed, with the process's major faults
//! around it and the bytes it answered, which every arm must agree on; the
//! store is dropped and removed before the next arm. Under `--bound` the
//! process enters a cgroup of its own first, as `bench ab --bound` does;
//! RocksDB as tuned does not survive 640 MiB at five million keys, so the
//! RocksDB there is `rocksdb-nosync`, the default options, whose reads are a
//! durable RocksDB's and whose sync policy only touches the load.
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use lz4_flex::block::{
    compress_into_with_dict, decompress_into_with_dict, get_maximum_output_size,
};
use memmap2::{Advice, Mmap};
use supdb_bench::bound::Bound;
use supdb_bench::engines::{self, Batch, Engine};
use supdb_bench::workload::{db_key_into, KeyDist, KeyGen, Payload, Rng};

/// Records a group; the key count at the probe's shape is sixteen bytes.
const GROUP: usize = 16;
const KEY: usize = 16;
const SCAN: usize = 50;
/// The dictionary: the segment's first values, under LZ4's 64 KiB window
/// with room for the record compressed against it.
const DICT_BYTES: usize = 60 << 10;

/// The per-record form over one segment: the file's groups through a
/// mapping advised random, as the compact form's is, and the groups' first
/// keys and offsets in memory, as the compact form's fence is.
struct ZRec {
    map: Mmap,
    first: Vec<[u8; KEY]>,
    offset: Vec<u32>,
    total: usize,
    dict: Vec<u8>,
}

/// A record in a group: the shared prefix length with the group's first
/// key, the suffix, the value's length and its compressed bytes.
struct Rec<'a> {
    plen: usize,
    suffix: &'a [u8],
    ulen: usize,
    cval: &'a [u8],
}

fn rec_at(grp: &[u8], off: usize) -> Rec<'_> {
    let plen = grp[off] as usize;
    let slen = grp[off + 1] as usize;
    let suffix = &grp[off + 2..off + 2 + slen];
    let at = off + 2 + slen;
    let ulen = grp[at] as usize;
    let clen = grp[at + 1] as usize;
    let cval = &grp[at + 2..at + 2 + clen];
    Rec {
        plen,
        suffix,
        ulen,
        cval,
    }
}

/// The record's key (the group's first key to `plen`, then the suffix)
/// compared with `key`, without assembling it.
fn cmp_key(first: &[u8], r: &Rec<'_>, key: &[u8]) -> std::cmp::Ordering {
    use std::cmp::Ordering::*;
    let head = &first[..r.plen];
    let n = head.len().min(key.len());
    match head[..n].cmp(&key[..n]) {
        Equal => {}
        o => return o,
    }
    if key.len() < head.len() {
        return Greater;
    }
    r.suffix.cmp(&key[n..])
}

impl ZRec {
    /// The form written from the generator's records, in key order, and mapped.
    fn build(path: &Path, size: u64) -> ZRec {
        // The dictionary: the first values, up to DICT_BYTES, from a
        // generator of their own.
        let mut dict = Vec::with_capacity(DICT_BYTES);
        let mut g = Gen::new(size);
        while let Some((_, v)) = g.next() {
            if dict.len() + v.len() > DICT_BYTES {
                break;
            }
            dict.extend_from_slice(v);
        }
        let mut out = BufWriter::with_capacity(1 << 20, File::create(path).unwrap());
        let cap = (size as usize) / GROUP + 1;
        let mut first = Vec::with_capacity(cap);
        let mut offset = Vec::with_capacity(cap);
        let mut pos = 0u32;
        let mut cbuf = vec![0u8; get_maximum_output_size(256)];
        let mut body: Vec<u8> = Vec::with_capacity(GROUP * 160);
        // One group's records, copied out of the generator's pool.
        let mut gk = [[0u8; KEY]; GROUP];
        let mut gv: Vec<Vec<u8>> = (0..GROUP).map(|_| Vec::with_capacity(256)).collect();
        let mut g = Gen::new(size);
        let mut total = 0usize;
        loop {
            let mut n_in = 0usize;
            while n_in < GROUP {
                let Some((k, v)) = g.next() else { break };
                gk[n_in] = k;
                gv[n_in].clear();
                gv[n_in].extend_from_slice(v);
                n_in += 1;
            }
            if n_in == 0 {
                break;
            }
            total += n_in;
            let fk = gk[0];
            first.push(fk);
            offset.push(pos);
            body.clear();
            let mut offs = [0u16; GROUP];
            for i in 0..n_in {
                let (k, v) = (&gk[i], &gv[i]);
                offs[i] = (KEY + 2 * GROUP + body.len()) as u16;
                let plen = k.iter().zip(fk.iter()).take_while(|(a, b)| a == b).count();
                let n = compress_into_with_dict(v, &mut cbuf, &dict).unwrap();
                assert!(
                    n < 256 && v.len() < 256,
                    "a record's value fits a byte of length"
                );
                body.push(plen as u8);
                body.push((k.len() - plen) as u8);
                body.extend_from_slice(&k[plen..]);
                body.push(v.len() as u8);
                body.push(n as u8);
                body.extend_from_slice(&cbuf[..n]);
            }
            out.write_all(&fk).unwrap();
            for o in offs {
                out.write_all(&o.to_le_bytes()).unwrap();
            }
            out.write_all(&body).unwrap();
            pos += (KEY + 2 * GROUP + body.len()) as u32;
        }
        out.flush().unwrap();
        let f = out.into_inner().unwrap();
        f.sync_all().unwrap();
        drop(f);
        // Mapped through a read-only handle: the writer's was write-only.
        let f = File::open(path).unwrap();
        let map = unsafe { Mmap::map(&f).unwrap() };
        map.advise(Advice::Random).unwrap();
        ZRec {
            map,
            first,
            offset,
            total,
            dict,
        }
    }

    fn groups(&self) -> usize {
        self.first.len()
    }

    fn count_of(&self, g: usize) -> usize {
        if g + 1 == self.groups() {
            let rem = self.total % GROUP;
            if rem == 0 {
                GROUP
            } else {
                rem
            }
        } else {
            GROUP
        }
    }

    fn group_bytes(&self, g: usize) -> &[u8] {
        let lo = self.offset[g] as usize;
        let hi = if g + 1 < self.groups() {
            self.offset[g + 1] as usize
        } else {
            self.map.len()
        };
        &self.map[lo..hi]
    }

    /// The group `key` would be in: the last whose first key is at or
    /// below it, the first group for a key below every first.
    fn group_of(&self, key: &[u8]) -> usize {
        let n = self.first.partition_point(|f| &f[..] <= key);
        n.saturating_sub(1)
    }

    fn rec_off(grp: &[u8], i: usize) -> usize {
        u16::from_le_bytes([grp[KEY + 2 * i], grp[KEY + 2 * i + 1]]) as usize
    }

    /// The value's bytes, decoded into `out`, consumed into `sink` as the
    /// harness consumes every value it is lent; 0 for a miss.
    fn get(&self, key: &[u8], out: &mut [u8], sink: &mut Vec<u8>) -> usize {
        let g = self.group_of(key);
        let grp = self.group_bytes(g);
        let first = &grp[..KEY];
        let (mut lo, mut hi) = (0usize, self.count_of(g));
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let r = rec_at(grp, Self::rec_off(grp, mid));
            match cmp_key(first, &r, key) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => {
                    let n =
                        decompress_into_with_dict(r.cval, &mut out[..r.ulen], &self.dict).unwrap();
                    sink.clear();
                    sink.extend_from_slice(&out[..n]);
                    return n;
                }
            }
        }
        0
    }

    /// `n` records from the first key at or after `from`: each key
    /// assembled and each value decoded, both consumed; the bytes visited.
    fn range(&self, from: &[u8], n: usize, out: &mut [u8], sink: &mut Vec<u8>) -> usize {
        let mut g = self.group_of(from);
        let mut grp = self.group_bytes(g);
        let mut count = self.count_of(g);
        let mut first = &grp[..KEY];
        // The first record at or after `from` in its group.
        let (mut lo, mut hi) = (0usize, count);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let r = rec_at(grp, Self::rec_off(grp, mid));
            if cmp_key(first, &r, from) == std::cmp::Ordering::Less {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let mut i = lo;
        let mut seen = 0usize;
        let mut bytes = 0usize;
        let mut kb = [0u8; KEY];
        while seen < n {
            if i >= count {
                g += 1;
                if g >= self.groups() {
                    break;
                }
                grp = self.group_bytes(g);
                count = self.count_of(g);
                first = &grp[..KEY];
                i = 0;
                continue;
            }
            let r = rec_at(grp, Self::rec_off(grp, i));
            kb[..r.plen].copy_from_slice(&first[..r.plen]);
            kb[r.plen..r.plen + r.suffix.len()].copy_from_slice(r.suffix);
            let vn = decompress_into_with_dict(r.cval, &mut out[..r.ulen], &self.dict).unwrap();
            std::hint::black_box(&kb[..r.plen + r.suffix.len()]);
            sink.clear();
            sink.extend_from_slice(&out[..vn]);
            bytes += vn;
            seen += 1;
            i += 1;
        }
        bytes
    }
}

/// The process's minor and major faults so far.
fn faults() -> (u64, u64) {
    let Ok(stat) = std::fs::read_to_string("/proc/self/stat") else {
        return (0, 0);
    };
    let Some((_, rest)) = stat.rsplit_once(')') else {
        return (0, 0);
    };
    let f: Vec<&str> = rest.split_whitespace().collect();
    let at = |i: usize| f.get(i).and_then(|v| v.parse().ok()).unwrap_or(0);
    (at(7), at(9))
}

/// The suite's records: sixteen-digit keys in order, hundred-byte values
/// half motif and half entropy, drawn as the suite's load draws them.
/// The suite's records in key order, one at a time: the key of `i` and a
/// value from the suite's payload pool, so nothing of the store is held
/// in the process beside the store.
struct Gen {
    payload: Payload,
    rng: Rng,
    i: u64,
    size: u64,
}

impl Gen {
    fn new(size: u64) -> Gen {
        Gen {
            payload: Payload::new(100, 0.5, 0xE1),
            rng: Rng::new(0xE2),
            i: 0,
            size,
        }
    }
    fn next(&mut self) -> Option<([u8; 16], &[u8])> {
        if self.i >= self.size {
            return None;
        }
        let mut kb = [0u8; 16];
        db_key_into(self.i, &mut kb);
        self.i += 1;
        Some((kb, self.payload.get(&mut self.rng)))
    }
}

/// One arm's store: the per-record form, or a suite engine by name.
enum Arm {
    Z(ZRec),
    E(Box<dyn Engine>),
}

struct Mix {
    name: &'static str,
    dist: KeyDist,
    scan: bool,
}

const MIXES: [Mix; 3] = [
    Mix {
        name: "uniform-reads",
        dist: KeyDist::Uniform,
        scan: false,
    },
    Mix {
        name: "zipf-reads   ",
        dist: KeyDist::Zipfian,
        scan: false,
    },
    Mix {
        name: "zipf-scans   ",
        dist: KeyDist::Zipfian,
        scan: true,
    },
];

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut bound: Option<u64> = None;
    if let Some(i) = args.iter().position(|a| a == "--bound") {
        bound = Some(args[i + 1].parse().expect("--bound BYTES"));
        args.drain(i..i + 2);
    }
    // The bound before any thread or allocation worth charging, as the
    // suite enters it.
    let entered = bound.map(|b| Bound::enter(b).expect("the bound"));
    let size: u64 = args.first().expect("KEYS").parse().unwrap();
    let rounds: usize = args.get(1).map_or(3, |s| s.parse().unwrap());
    let ops: usize = args.get(2).map_or(200_000, |s| s.parse().unwrap());
    let arms: Vec<String> = args
        .get(3)
        .map(|s| s.split(',').map(str::to_string).collect())
        .unwrap_or_else(|| {
            vec![
                "zrec".to_string(),
                "supdb-nocachezpages".to_string(),
                "lmdb-nordahead".to_string(),
                "rocksdb-tuned".to_string(),
            ]
        });
    let dir: PathBuf = std::env::temp_dir().join(format!("supdb-zrec-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    println!(
        "{size} keys, {rounds} rounds, {ops} reads a mix and {} scans of {SCAN}, bound {}",
        ops / 10,
        entered
            .as_ref()
            .map_or("none".to_string(), |b| format!("{} bytes", b.limit()))
    );
    let mut out = vec![0u8; 512];
    let mut sink: Vec<u8> = Vec::with_capacity(8 << 10);
    let mut kb = [0u8; 16];
    for round in 1..=rounds {
        for arm in &arms {
            let d = dir.join(arm);
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            let t = Instant::now();
            let (mut store, size_bytes) = if arm == "zrec" {
                let z = ZRec::build(&d.join("zrec.bin"), size);
                let bytes = z.map.len() as u64;
                (Arm::Z(z), bytes)
            } else {
                let mut e = engines::open(arm, &d, 3).unwrap();
                let mut b = Batch::with_capacity(1000, 100);
                let mut g = Gen::new(size);
                while let Some((k, v)) = g.next() {
                    b.push(&k, v);
                    if b.len() >= 1000 {
                        b.flush(e.as_mut()).unwrap();
                    }
                }
                b.flush(e.as_mut()).unwrap();
                e.sync().unwrap();
                let bytes = e.size_bytes();
                (Arm::E(e), bytes)
            };
            let load_s = t.elapsed().as_secs_f64();
            let mut line = format!(
                "round {round} {arm:<22} load {load_s:6.1}s {:7.1} MB |",
                size_bytes as f64 / 1e6
            );
            for m in &MIXES {
                let n = if m.scan { ops / 10 } else { ops };
                let mut keys = KeyGen::new(m.dist, size, 0x9C5B ^ round as u64);
                let (_, mj0) = faults();
                let t = Instant::now();
                let mut bytes = 0usize;
                let mut misses = 0usize;
                for _ in 0..n {
                    db_key_into(keys.next(), &mut kb);
                    let b = match &mut store {
                        Arm::Z(z) => {
                            if m.scan {
                                z.range(&kb, SCAN, &mut out, &mut sink)
                            } else {
                                z.get(&kb, &mut out, &mut sink)
                            }
                        }
                        Arm::E(e) => {
                            if m.scan {
                                e.range(&kb, SCAN).unwrap()
                            } else {
                                e.get(&kb).unwrap()
                            }
                        }
                    };
                    misses += (b == 0) as usize;
                    bytes += b;
                }
                let el = t.elapsed().as_secs_f64();
                let (_, mj1) = faults();
                // The byte total is the check that the arms answered the
                // same question: every key exists, so the totals agree
                // across arms and the misses are zero.
                line.push_str(&format!(
                    " {}{:7.2} us/op {:6.3} majflt/op {:5.1} MB {} miss |",
                    m.name,
                    el * 1e6 / n as f64,
                    (mj1 - mj0) as f64 / n as f64,
                    bytes as f64 / 1e6,
                    misses
                ));
            }
            println!("{line}");
            drop(store);
            let _ = std::fs::remove_dir_all(&d);
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
