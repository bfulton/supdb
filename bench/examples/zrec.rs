//! The per-record form, probed against the comparators before it is built
//! into the engine, and its two open choices -- the dictionary and the
//! value coding -- probed against each other.
//!
//! The compact form decodes a group of about fourteen records to read one
//! of them, so a point read pays a seven-kilobyte decode for a record of a
//! hundred and sixteen bytes, and a scan of fifty records decodes the whole
//! of every group it touches. Under the bound at five million keys the
//! decode is the largest cost the form's reads have left, and the cells the
//! engine loses there are E's scans and D's reads (`bench/scripts/pair.sh
//! lmdb-nordahead supdb-cache64tofitautozpages 5000000 --reps 3 --bound
//! 671088640 --passes read,read-threaded,scan,scan-threaded,mixes --ops-div
//! 10` at `dda952d`). This probe writes the same records as a form
//! whose records decode on their own -- a group of sixteen records, its
//! first key raw, each record's key front-coded against it (the shared
//! prefix's length and the suffix), each value compressed on its own with
//! LZ4, and a table of record offsets at the group's head -- and runs
//! read-only mixes against it, the current form (`supdb-nocachezpages`, the
//! form forced), LMDB with its readahead off and RocksDB, in one process,
//! arms interleaved within a round. A point read decodes one record; a scan
//! decodes exactly the records it emits.
//!
//! The form has shapes, one arm each, for the choices a build has to make:
//!
//! - `zrec`: every value LZ4-compressed against a dictionary of the
//!   segment's first values (sixty kilobytes of them).
//! - `zrec-sampled`: the dictionary sampled across the segment instead,
//!   every `size / 600`th value.
//! - `zrec-local`: no segment dictionary; a group's first record is stored
//!   raw and is the dictionary for the fifteen after it, so a group's own
//!   redundancy is what compresses it, as in the group form, with every
//!   record still decoding on its own (one raw record read as its
//!   dictionary).
//! - `zrec-hybrid`: both at once -- the segment's first values as the
//!   dictionary with the group's raw first record appended to it, so a
//!   value matches against the segment's redundancy and its group's.
//! - `zrec-nodict`: every value compressed alone, no dictionary.
//! - `zrec-raw`: values stored raw, only the keys coded.
//! - `zrec-group`: the group's sixteen records compressed as one block, the
//!   current form's shape at this grouping -- the control for the size
//!   and the decode the per-record shapes trade.
//!
//! A value that LZ4 does not shrink is stored raw under every shape (its
//! compressed length equal to its length says so), so no shape is larger
//! than `zrec-raw` by more than the header bytes.
//!
//! What it does not do: inserts (D's and E's 5%, which is why the mixes are
//! not named after the suite's), the controller, the block cache, any write
//! path. It is a probe of the read structure, and its numbers are the
//! mechanism's, not a row's.
//!
//!   cargo run --release --example zrec -- KEYS ROUNDS OPS ARM,ARM,... \
//!       [--bound BYTES] [--payload suite|drift]
//!
//! Each round: for each arm, load KEYS records, then OPS uniform point
//! reads (the suite's read pass), OPS Zipfian point reads (C's), OPS/10
//! Zipfian scans of fifty (E's scans without its inserts), each timed, with
//! the process's major faults around it and the bytes it answered, which
//! every arm must agree on; the store is dropped and removed before the
//! next arm. The payload is the suite's by default: sixteen-digit keys in
//! order and hundred-byte values half a shared motif and half entropy,
//! drawn as the suite's load draws them. `--payload drift` keeps the shape
//! and changes the motif every `DRIFT_EVERY` records, so a value shares its
//! motif with its neighbours in key order and with nothing far away: the
//! payload a segment dictionary cannot learn and a group's own redundancy
//! can. Under `--bound` the process enters a cgroup of its own first, as
//! `bench ab --bound` does; RocksDB as tuned does not survive 640 MiB at
//! five million keys, so the RocksDB there is `rocksdb-nosync`, the default
//! options, whose reads are a durable RocksDB's and whose sync policy only
//! touches the load.
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use lz4_flex::block::{
    compress_into, compress_into_with_dict, decompress_into, decompress_into_with_dict,
    get_maximum_output_size,
};
use memmap2::{Advice, Mmap};
use supdb_bench::bound::Bound;
use supdb_bench::engines::{self, Batch, Engine};
use supdb_bench::workload::{db_key_into, KeyDist, KeyGen, Payload, Rng};

/// Records a group; the key count at the probe's shape is sixteen bytes.
const GROUP: usize = 16;
const KEY: usize = 16;
const SCAN: usize = 50;
const VALUE: usize = 100;
/// The dictionary: sixty kilobytes of values, under LZ4's 64 KiB window
/// with room for the record compressed against it.
const DICT_BYTES: usize = 60 << 10;
/// The drifting payload's motif changes every this many records: twelve
/// groups share one, and a segment of a million has five thousand.
const DRIFT_EVERY: u64 = 200;
/// The motif's length, the suite's.
const MOTIF: usize = 56;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Dict {
    First,
    Sampled,
    Local,
    /// `First` with the group's first record appended: the dictionary is
    /// assembled per group, the segment's part once and the record copied
    /// behind it.
    Hybrid,
    None,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Coding {
    /// A value compressed on its own, against the shape's dictionary.
    Lz4,
    /// A value stored raw.
    Raw,
    /// The group's records compressed as one block.
    Group,
}

struct Shape {
    name: &'static str,
    coding: Coding,
    dict: Dict,
}

const SHAPES: [Shape; 7] = [
    Shape {
        name: "zrec",
        coding: Coding::Lz4,
        dict: Dict::First,
    },
    Shape {
        name: "zrec-sampled",
        coding: Coding::Lz4,
        dict: Dict::Sampled,
    },
    Shape {
        name: "zrec-local",
        coding: Coding::Lz4,
        dict: Dict::Local,
    },
    Shape {
        name: "zrec-hybrid",
        coding: Coding::Lz4,
        dict: Dict::Hybrid,
    },
    Shape {
        name: "zrec-nodict",
        coding: Coding::Lz4,
        dict: Dict::None,
    },
    Shape {
        name: "zrec-raw",
        coding: Coding::Raw,
        dict: Dict::None,
    },
    Shape {
        name: "zrec-group",
        coding: Coding::Group,
        dict: Dict::None,
    },
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PayloadKind {
    Suite,
    Drift,
}

/// The per-record form over one segment: the file's groups through a
/// mapping advised random, as the compact form's is, and the groups' first
/// keys and offsets in memory, as the compact form's fence is.
struct ZRec {
    map: Mmap,
    first: Vec<[u8; KEY]>,
    offset: Vec<u32>,
    total: usize,
    coding: Coding,
    dict_kind: Dict,
    dict: Vec<u8>,
    /// The hybrid shape's assembled dictionary: `dict` with the group's
    /// first record behind it, the record copied in at each group a read
    /// decodes from (`hybrid_of` names the group it holds).
    hybrid: std::cell::RefCell<(usize, Vec<u8>)>,
}

/// A record in a group under the per-record codings: the shared prefix
/// length with the group's first key, the suffix, the value's length and
/// its stored bytes -- compressed, or raw when `cval.len() == ulen`.
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

/// A record in a decoded group body under the group coding: the same key
/// coding, the value raw.
struct BRec<'a> {
    plen: usize,
    suffix: &'a [u8],
    val: &'a [u8],
}

fn brec_at(body: &[u8], off: usize) -> BRec<'_> {
    let plen = body[off] as usize;
    let slen = body[off + 1] as usize;
    let suffix = &body[off + 2..off + 2 + slen];
    let at = off + 2 + slen;
    let ulen = body[at] as usize;
    let val = &body[at + 1..at + 1 + ulen];
    BRec { plen, suffix, val }
}

/// The record's key (the group's first key to `plen`, then the suffix)
/// compared with `key`, without assembling it.
fn cmp_key(first: &[u8], plen: usize, suffix: &[u8], key: &[u8]) -> std::cmp::Ordering {
    use std::cmp::Ordering::*;
    let head = &first[..plen];
    let n = head.len().min(key.len());
    match head[..n].cmp(&key[..n]) {
        Equal => {}
        o => return o,
    }
    if key.len() < head.len() {
        return Greater;
    }
    suffix.cmp(&key[n..])
}

/// The buffers a read uses: a decoded value, a decoded group body (the
/// group coding), and the harness's sink, which every read copies its
/// value into as the suite's `consume` does.
struct Scratch {
    out: Vec<u8>,
    grp: Vec<u8>,
    sink: Vec<u8>,
}

impl ZRec {
    /// The form written from the generator's records, in key order, and
    /// mapped.
    fn build(path: &Path, size: u64, shape: &Shape, kind: PayloadKind) -> ZRec {
        let dict = dictionary(shape.dict, size, kind);
        let mut out = BufWriter::with_capacity(1 << 20, File::create(path).unwrap());
        let cap = (size as usize) / GROUP + 1;
        let mut first = Vec::with_capacity(cap);
        let mut offset = Vec::with_capacity(cap);
        let mut pos = 0u32;
        let mut cbuf = vec![0u8; get_maximum_output_size(GROUP * 2 * VALUE)];
        let mut body: Vec<u8> = Vec::with_capacity(GROUP * 2 * VALUE);
        // One group's records, copied out of the generator's pool.
        let mut gk = [[0u8; KEY]; GROUP];
        let mut gv: Vec<Vec<u8>> = (0..GROUP).map(|_| Vec::with_capacity(256)).collect();
        let mut g = Gen::new(size, kind);
        let mut total = 0usize;
        // The hybrid dictionary's buffer: the segment's part, then room for
        // the group's first record behind it.
        let mut hybrid: Vec<u8> = Vec::with_capacity(dict.len() + 256);
        hybrid.extend_from_slice(&dict);
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
            match shape.coding {
                Coding::Lz4 | Coding::Raw => {
                    for i in 0..n_in {
                        let (k, v) = (&gk[i], &gv[i]);
                        offs[i] = (KEY + 2 * GROUP + body.len()) as u16;
                        let plen = k.iter().zip(fk.iter()).take_while(|(a, b)| a == b).count();
                        // The stored value: compressed where that is shorter,
                        // raw otherwise, and raw for the local dictionary's
                        // first record, which is the dictionary.
                        let n = match (shape.coding, shape.dict) {
                            (Coding::Raw, _) => v.len(),
                            (_, Dict::Local | Dict::Hybrid) if i == 0 => v.len(),
                            (_, Dict::Local) => {
                                compress_into_with_dict(v, &mut cbuf, &gv[0]).unwrap()
                            }
                            (_, Dict::Hybrid) => {
                                hybrid.truncate(dict.len());
                                hybrid.extend_from_slice(&gv[0]);
                                compress_into_with_dict(v, &mut cbuf, &hybrid).unwrap()
                            }
                            (_, Dict::None) => compress_into(v, &mut cbuf).unwrap(),
                            _ => compress_into_with_dict(v, &mut cbuf, &dict).unwrap(),
                        };
                        let (clen, cval): (usize, &[u8]) = if n < v.len() {
                            (n, &cbuf[..n])
                        } else {
                            (v.len(), v)
                        };
                        assert!(
                            v.len() < 256 && clen < 256,
                            "a record's value fits a byte of length"
                        );
                        body.push(plen as u8);
                        body.push((k.len() - plen) as u8);
                        body.extend_from_slice(&k[plen..]);
                        body.push(v.len() as u8);
                        body.push(clen as u8);
                        body.extend_from_slice(cval);
                    }
                    out.write_all(&fk).unwrap();
                    for o in offs {
                        out.write_all(&o.to_le_bytes()).unwrap();
                    }
                    out.write_all(&body).unwrap();
                    pos += (KEY + 2 * GROUP + body.len()) as u32;
                }
                Coding::Group => {
                    // The body of raw records, the offsets into it, then the
                    // body as one block.
                    for i in 0..n_in {
                        let (k, v) = (&gk[i], &gv[i]);
                        offs[i] = body.len() as u16;
                        let plen = k.iter().zip(fk.iter()).take_while(|(a, b)| a == b).count();
                        body.push(plen as u8);
                        body.push((k.len() - plen) as u8);
                        body.extend_from_slice(&k[plen..]);
                        body.push(v.len() as u8);
                        body.extend_from_slice(v);
                    }
                    let n = compress_into(&body, &mut cbuf).unwrap();
                    let (clen, cbody): (usize, &[u8]) = if n < body.len() {
                        (n, &cbuf[..n])
                    } else {
                        (body.len(), &body)
                    };
                    assert!(body.len() < 65536 && clen < 65536);
                    out.write_all(&fk).unwrap();
                    for o in offs {
                        out.write_all(&o.to_le_bytes()).unwrap();
                    }
                    out.write_all(&(body.len() as u16).to_le_bytes()).unwrap();
                    out.write_all(&(clen as u16).to_le_bytes()).unwrap();
                    out.write_all(cbody).unwrap();
                    pos += (KEY + 2 * GROUP + 4 + clen) as u32;
                }
            }
        }
        out.flush().unwrap();
        let f = out.into_inner().unwrap();
        f.sync_all().unwrap();
        drop(f);
        // Mapped through a read-only handle: the writer's was write-only.
        let f = File::open(path).unwrap();
        let map = unsafe { Mmap::map(&f).unwrap() };
        map.advise(Advice::Random).unwrap();
        let hybrid = std::cell::RefCell::new((usize::MAX, hybrid));
        ZRec {
            map,
            first,
            offset,
            total,
            coding: shape.coding,
            dict_kind: shape.dict,
            dict,
            hybrid,
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

    /// Record `i`'s offset from the group's offset table: from the group's
    /// start under the per-record codings, into the decoded body under the
    /// group coding.
    fn rec_off(grp: &[u8], i: usize) -> usize {
        let at = KEY + 2 * i;
        u16::from_le_bytes([grp[at], grp[at + 1]]) as usize
    }

    /// A per-record coding's value decoded into `out`; its length.
    fn value_into(&self, g: usize, grp: &[u8], r: &Rec<'_>, out: &mut [u8]) -> usize {
        if r.cval.len() == r.ulen {
            out[..r.ulen].copy_from_slice(r.cval);
            return r.ulen;
        }
        match self.dict_kind {
            Dict::Local => {
                let r0 = rec_at(grp, Self::rec_off(grp, 0));
                decompress_into_with_dict(r.cval, &mut out[..r.ulen], r0.cval).unwrap()
            }
            Dict::Hybrid => {
                let mut h = self.hybrid.borrow_mut();
                if h.0 != g {
                    let r0 = rec_at(grp, Self::rec_off(grp, 0));
                    let base = self.dict.len();
                    h.1.truncate(base);
                    h.1.extend_from_slice(r0.cval);
                    h.0 = g;
                }
                decompress_into_with_dict(r.cval, &mut out[..r.ulen], &h.1).unwrap()
            }
            Dict::None => decompress_into(r.cval, &mut out[..r.ulen]).unwrap(),
            Dict::First | Dict::Sampled => {
                decompress_into_with_dict(r.cval, &mut out[..r.ulen], &self.dict).unwrap()
            }
        }
    }

    /// The group coding's body decoded into `into`.
    fn decode_group(grp: &[u8], into: &mut Vec<u8>) {
        let at = KEY + 2 * GROUP;
        let ulen = u16::from_le_bytes([grp[at], grp[at + 1]]) as usize;
        let clen = u16::from_le_bytes([grp[at + 2], grp[at + 3]]) as usize;
        let c = &grp[at + 4..at + 4 + clen];
        into.clear();
        into.resize(ulen, 0);
        if clen == ulen {
            into.copy_from_slice(c);
        } else {
            let n = decompress_into(c, &mut into[..]).unwrap();
            assert_eq!(n, ulen);
        }
    }

    /// The first record at or after `key` in group `g`, by binary search
    /// over the group's records; `count` for none.
    fn lower_bound(&self, g: usize, grp: &[u8], body: &[u8], key: &[u8]) -> usize {
        let first = &grp[..KEY];
        let (mut lo, mut hi) = (0usize, self.count_of(g));
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let off = Self::rec_off(grp, mid);
            let ord = match self.coding {
                Coding::Group => {
                    let r = brec_at(body, off);
                    cmp_key(first, r.plen, r.suffix, key)
                }
                _ => {
                    let r = rec_at(grp, off);
                    cmp_key(first, r.plen, r.suffix, key)
                }
            };
            if ord == std::cmp::Ordering::Less {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    }

    /// Whether record `i` of group `g` is `key`.
    fn is_key(&self, grp: &[u8], body: &[u8], i: usize, key: &[u8]) -> bool {
        let first = &grp[..KEY];
        let off = Self::rec_off(grp, i);
        let ord = match self.coding {
            Coding::Group => {
                let r = brec_at(body, off);
                cmp_key(first, r.plen, r.suffix, key)
            }
            _ => {
                let r = rec_at(grp, off);
                cmp_key(first, r.plen, r.suffix, key)
            }
        };
        ord == std::cmp::Ordering::Equal
    }

    /// A point read: the value's bytes, copied into the sink as the
    /// harness's reads are; zero for a key the form does not hold.
    fn get(&self, key: &[u8], s: &mut Scratch) -> usize {
        let g = self.group_of(key);
        let grp = self.group_bytes(g);
        if self.coding == Coding::Group {
            Self::decode_group(grp, &mut s.grp);
        }
        let i = self.lower_bound(g, grp, &s.grp, key);
        if i >= self.count_of(g) || !self.is_key(grp, &s.grp, i, key) {
            return 0;
        }
        let off = Self::rec_off(grp, i);
        let n = match self.coding {
            Coding::Group => {
                let r = brec_at(&s.grp, off);
                s.out[..r.val.len()].copy_from_slice(r.val);
                r.val.len()
            }
            _ => {
                let r = rec_at(grp, off);
                self.value_into(g, grp, &r, &mut s.out)
            }
        };
        s.sink.clear();
        s.sink.extend_from_slice(&s.out[..n]);
        n
    }

    /// A scan of `n` records from `from`: each key assembled, each value
    /// decoded and copied into the sink; the values' bytes, as the
    /// harness counts them.
    fn range(&self, from: &[u8], n: usize, s: &mut Scratch) -> usize {
        let mut g = self.group_of(from);
        let mut grp = self.group_bytes(g);
        let mut count = self.count_of(g);
        let mut first = &grp[..KEY];
        if self.coding == Coding::Group {
            Self::decode_group(grp, &mut s.grp);
        }
        let mut i = self.lower_bound(g, grp, &s.grp, from);
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
                if self.coding == Coding::Group {
                    Self::decode_group(grp, &mut s.grp);
                }
                i = 0;
                continue;
            }
            let off = Self::rec_off(grp, i);
            let vn = match self.coding {
                Coding::Group => {
                    let r = brec_at(&s.grp, off);
                    kb[..r.plen].copy_from_slice(&first[..r.plen]);
                    kb[r.plen..r.plen + r.suffix.len()].copy_from_slice(r.suffix);
                    std::hint::black_box(&kb[..r.plen + r.suffix.len()]);
                    s.sink.clear();
                    s.sink.extend_from_slice(r.val);
                    r.val.len()
                }
                _ => {
                    let r = rec_at(grp, off);
                    kb[..r.plen].copy_from_slice(&first[..r.plen]);
                    kb[r.plen..r.plen + r.suffix.len()].copy_from_slice(r.suffix);
                    std::hint::black_box(&kb[..r.plen + r.suffix.len()]);
                    let vn = self.value_into(g, grp, &r, &mut s.out);
                    s.sink.clear();
                    s.sink.extend_from_slice(&s.out[..vn]);
                    vn
                }
            };
            bytes += vn;
            seen += 1;
            i += 1;
        }
        bytes
    }
}

/// The shape's dictionary: the first values up to `DICT_BYTES`, or values
/// sampled across the segment, or nothing.
fn dictionary(kind: Dict, size: u64, payload: PayloadKind) -> Vec<u8> {
    let mut dict = Vec::with_capacity(DICT_BYTES);
    match kind {
        Dict::None | Dict::Local => {}
        Dict::First | Dict::Hybrid => {
            let mut g = Gen::new(size, payload);
            while let Some((_, v)) = g.next() {
                if dict.len() + v.len() > DICT_BYTES {
                    break;
                }
                dict.extend_from_slice(v);
            }
        }
        Dict::Sampled => {
            let samples = (DICT_BYTES / VALUE) as u64;
            let stride = (size / samples).max(1);
            let mut g = Gen::new(size, payload);
            let mut i = 0u64;
            while let Some((_, v)) = g.next() {
                if i.is_multiple_of(stride) && dict.len() + v.len() <= DICT_BYTES {
                    dict.extend_from_slice(v);
                }
                i += 1;
            }
        }
    }
    dict
}

/// Minor and major faults of the process so far, from `/proc/self/stat`.
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

/// The records in key order, one at a time: the key of `i` and a value,
/// so nothing of the store is held in the process beside the store. The
/// suite's payload draws a hundred-byte slice of the suite's pool, half a
/// shared motif and half entropy; the drifting payload lays the same halves
/// out with a motif of its own for every `DRIFT_EVERY` records.
struct Gen {
    kind: PayloadKind,
    payload: Payload,
    rng: Rng,
    i: u64,
    size: u64,
    buf: [u8; VALUE],
    motif: [u8; MOTIF],
    motif_of: u64,
}

impl Gen {
    fn new(size: u64, kind: PayloadKind) -> Gen {
        Gen {
            kind,
            payload: Payload::new(VALUE, 0.5, 0xE1),
            rng: Rng::new(0xE2),
            i: 0,
            size,
            buf: [0u8; VALUE],
            motif: [0u8; MOTIF],
            motif_of: u64::MAX,
        }
    }
    fn next(&mut self) -> Option<([u8; KEY], &[u8])> {
        if self.i >= self.size {
            return None;
        }
        let mut kb = [0u8; KEY];
        db_key_into(self.i, &mut kb);
        let i = self.i;
        self.i += 1;
        match self.kind {
            PayloadKind::Suite => Some((kb, self.payload.get(&mut self.rng))),
            PayloadKind::Drift => {
                let m = i / DRIFT_EVERY;
                if m != self.motif_of {
                    self.motif = drift_motif(m);
                    self.motif_of = m;
                }
                let phase = (i as usize * 7) % MOTIF;
                let comp = VALUE / 2;
                for j in 0..comp {
                    self.buf[j] = self.motif[(phase + j) % MOTIF];
                }
                for j in comp..VALUE {
                    self.buf[j] = (self.rng.next() & 0xff) as u8;
                }
                Some((kb, &self.buf[..]))
            }
        }
    }
}

/// Motif `m` of the drifting payload: printable bytes from a generator
/// seeded by `m`, the same every run.
fn drift_motif(m: u64) -> [u8; MOTIF] {
    let mut r = Rng::new(0xD21F ^ m.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let mut out = [0u8; MOTIF];
    for b in out.iter_mut() {
        *b = b' ' + (r.next() % 95) as u8;
    }
    out
}

/// One arm's store: a shape of the per-record form, or a suite engine by
/// name.
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
    let mut payload = PayloadKind::Suite;
    if let Some(i) = args.iter().position(|a| a == "--payload") {
        payload = match args[i + 1].as_str() {
            "suite" => PayloadKind::Suite,
            "drift" => PayloadKind::Drift,
            other => panic!("--payload suite|drift, not {other}"),
        };
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
        "{size} keys, {rounds} rounds, {ops} reads a mix and {} scans of {SCAN}, payload {}, bound {}",
        ops / 10,
        match payload {
            PayloadKind::Suite => "suite",
            PayloadKind::Drift => "drift",
        },
        entered
            .as_ref()
            .map_or("none".to_string(), |b| format!("{} bytes", b.limit()))
    );
    let mut s = Scratch {
        out: vec![0u8; 512],
        grp: Vec::with_capacity(GROUP * 2 * VALUE),
        sink: Vec::with_capacity(8 << 10),
    };
    let mut kb = [0u8; KEY];
    for round in 1..=rounds {
        for arm in &arms {
            let d = dir.join(arm);
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            let t = Instant::now();
            let (mut store, size_bytes) = if let Some(shape) = SHAPES.iter().find(|s| s.name == arm)
            {
                let z = ZRec::build(&d.join("zrec.bin"), size, shape, payload);
                let bytes = z.map.len() as u64;
                (Arm::Z(z), bytes)
            } else {
                let mut e = engines::open(arm, &d, 3).unwrap();
                let mut b = Batch::with_capacity(1000, VALUE);
                let mut g = Gen::new(size, payload);
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
                "round {round} {arm:<22} load {load_s:6.1}s {:7.1} MB {:6.1} B/rec |",
                size_bytes as f64 / 1e6,
                size_bytes as f64 / size as f64
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
                                z.range(&kb, SCAN, &mut s)
                            } else {
                                z.get(&kb, &mut s)
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
