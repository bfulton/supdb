#!/usr/bin/env python3
"""Lever C: a block's snapshot keys cut by one cursor moved forward key by
key (`ZCursor::skip_until`, `Options::zpages_cut_cursor`, on), the shape
before (a fence seek a key) as `supdb-cache64tofitautozpagescutseek`.
Applied after lever A's commit; every replacement asserts its anchor once."""
def edit(path, pairs):
    s = open(path).read()
    for old, new in pairs:
        n = s.count(old)
        assert n == 1, (path, n, old[:90])
        s = s.replace(old, new)
    open(path, 'w').write(s)

Z = '/home/user/supdb/src/db/zfile.rs'
edit(Z, [
('''    /// The rank at the position: the key count at the end.
    #[cfg(test)]
    pub(super) fn rank(&self) -> usize {''',
 '''    /// The rank at the position: the key count at the end.
    pub(super) fn rank(&self) -> usize {'''),
('''    /// The bulk walk from the position, for the overlay walk
    /// (`Reader::scan_partitions` under `Options::zpages_walk_cursor`):''',
 '''    /// The position moved to the first record whose key is not below
    /// `until`, or to the end, and how that key compares with `until`:
    /// `Equal` for `until` itself, `Greater` for a key past it and at the
    /// end. The group is held across the records and across the caller's
    /// turns, so a batch of keys in key order cut against the partition
    /// opens one group a group crossed (the cuts of a block's snapshot keys,
    /// `SnapBounds::cuts_of` under `Options::zpages_cut_cursor`), where a
    /// seek a key opened the key's group from the fence each time, through
    /// the fence's search, the page's directory and the memo. The keys are
    /// read and nothing else: a record's values are not parsed. A record
    /// that does not parse is damage, and the rest of the walk is the raw
    /// form's, as `walk_until` leaves it.
    pub(super) fn skip_until(&mut self, until: &[u8]) -> KeyOrder {
        loop {
            match self.group {
                Group::None => return KeyOrder::Greater,
                Group::Raw => {
                    let Some(k) = self.raw.key_at(self.rank) else {
                        return KeyOrder::Greater;
                    };
                    match k.cmp(until) {
                        KeyOrder::Less => self.advance(),
                        order => return order,
                    }
                }
                _ => {
                    let (mut off, mut rank, end) = (self.off, self.rank, self.end);
                    let mut stop: Option<KeyOrder> = None;
                    let mut damaged = false;
                    {
                        let group = self.group();
                        while rank < end {
                            let Some((k, next)) = record_key_next(group, off) else {
                                damaged = true;
                                break;
                            };
                            match k.cmp(until) {
                                KeyOrder::Less => {
                                    off = next;
                                    rank += 1;
                                }
                                order => {
                                    stop = Some(order);
                                    break;
                                }
                            }
                        }
                    }
                    self.rank = rank;
                    if damaged {
                        self.zfile.fail(self.page);
                        self.fall();
                        continue;
                    }
                    if rank >= end {
                        self.next_group();
                        continue;
                    }
                    self.off = off;
                    self.place();
                    return stop.unwrap_or(KeyOrder::Greater);
                }
            }
        }
    }

    /// The bulk walk from the position, for the overlay walk
    /// (`Reader::scan_partitions` under `Options::zpages_walk_cursor`):'''),
])

D = '/home/user/supdb/src/db.rs'
edit(D, [
('''    #[doc(hidden)]
    pub zpages_trusted_views: bool,
''',
 '''    #[doc(hidden)]
    pub zpages_trusted_views: bool,
    /// The cuts of a block's snapshot keys against its partition -- where
    /// each unsealed key cuts the partition's walk, taken once at the
    /// block's first build or walk (`SnapBounds::cuts_of`) -- by one cursor
    /// through the form file moved forward key by key
    /// (`zfile::ZCursor::skip_until`): the keys are in key order and lie in
    /// the block's few groups, so the cursor opens one group a group
    /// crossed, where a seek a key searched the fence, read the page's
    /// directory, asked the memo and walked the group from its start for
    /// every key -- 5.9% of the E mix's main thread at five million keys
    /// under 640 MiB, the builds' largest part. Off is the seek a key, the
    /// shape before; `supdb-cache64tofitautozpagescutseek` is the arm. The
    /// raw form's cuts were already made this way (`OrdIndex::advance_below`).
    #[doc(hidden)]
    pub zpages_cut_cursor: bool,
'''),
('''            zpages_trusted_views: true,
''',
 '''            zpages_trusted_views: true,
            zpages_cut_cursor: true,
'''),
('''        let via_z = rd.zc(seg).is_some();
        for i in lo..hi {
            let (k, _) = snap
                .get(i)
                .ok_or_else(|| err("block cache: a snapshot bound did not resolve"))?;
            let same = if via_z {''',
 '''        // Through the form file with one cursor moved forward key by key
        // (`Options::zpages_cut_cursor`): the keys over the block are in
        // key order and lie in its few groups, so the cursor opens one
        // group a group crossed where the seek below opens the key's group
        // from the fence for every key.
        if let Some(z) = rd.zc(seg).filter(|_| rd.opts.zpages_cut_cursor) {
            let mut cur = zfile::ZCursor::at(&rd.zmemo, z, seg.blob.for_zfile(), r);
            for i in lo..hi {
                let (k, _) = snap
                    .get(i)
                    .ok_or_else(|| err("block cache: a snapshot bound did not resolve"))?;
                let at = cur.skip_until(k);
                let c = cur.rank().min(keys);
                let same = c < keys && at == Ordering::Equal;
                self.cuts[i].store(((c as u32) << 1) | same as u32, AtomicOrdering::Relaxed);
            }
            rd.zbump_by(|s| &s.zscangroups, cur.decoded);
            if cur.fell_back() {
                rd.zbump(|s| &s.zfallbacks);
            }
            drop(cur);
            rd.zmemo_take();
            self.ready[b].store(true, AtomicOrdering::Release);
            return Ok(&self.cuts);
        }
        let via_z = rd.zc(seg).is_some();
        for i in lo..hi {
            let (k, _) = snap
                .get(i)
                .ok_or_else(|| err("block cache: a snapshot bound did not resolve"))?;
            let same = if via_z {'''),
])
print("lever C engine edits applied")
