# verify.py SHAPE STORED KEYS PLAIN
# Reads a file the probe left (E5_KEEP=1, E5_KEYS, E5_PLAIN) the way each
# candidate's recovery rule reads it after a clean stop, and requires the
# plaintext it rebuilds to be the plaintext the probe handed down, byte for
# byte. It checks that every page, record, frame and tag page the probe wrote
# is where the design puts it and sealed as the design seals it; it is not a
# crash test (the model, work/fix/model, is that).
import struct, sys
from cryptography.hazmat.primitives.ciphers.aead import AESGCM

PG = 4096
shape, stored_p, keys_p, plain_p = sys.argv[1:5]
stored = open(stored_p, 'rb').read()
keys = open(keys_p, 'rb').read()
want = open(plain_p, 'rb').read()


def data_off(p):
    return (1 << 20) + (p >> 16) * (258 << 20) + (2 << 20) + (p & 0xFFFF) * PG


def tagpage_off(g):
    p = g * 128
    return (1 << 20) + (p >> 16) * (258 << 20) + (g & 511) * PG


def page_dec(aes, fid, p, ctr, ct, tag):
    iv = b'\x10' + struct.pack('<Q', p)[:7] + struct.pack('<I', ctr)
    aad = fid + struct.pack('<QIH', p, ctr, 0)
    return aes.decrypt(iv, ct + tag, aad)


def adj():
    fid, sk = keys[:16], keys[16:48]
    aes = AESGCM(sk)
    recs = []
    cands = [8192 + i * 8192 for i in range(8)]
    # data-area candidates: every page whose clear prefix has the magic
    base = (1 << 20) + (2 << 20)
    for off in range(base, len(stored), PG):
        if stored[off:off + 4] == b'PFR1':
            cands.append(off)
    for off in cands:
        h = stored[off:off + 32]
        if h[:4] != b'PFR1':
            continue
        pages, = struct.unpack_from('<H', h, 6)
        seq, = struct.unpack_from('<Q', h, 8)
        blen, at = struct.unpack_from('<II', h, 20)
        if at != off // PG:
            continue
        iv = b'\x11' + struct.pack('<Q', seq) + b'\0\0\0'
        try:
            body = aes.decrypt(iv, stored[off + 32:off + 32 + blen + 16], fid + h)
        except Exception:
            continue
        recs.append((seq, off, h[5], body))
    recs.sort(reverse=True)
    seq, off, kind, b = recs[0]
    o = 0
    L, sh, F, cg, cctr = struct.unpack_from('<QQQII', b, o); o += 32
    cmac = b[o:o + 16]; o += 16
    nopen, = struct.unpack_from('<I', b, o); o += 4
    opens = [b[o + 20 * i:o + 20 * i + 20] for i in range(nopen)]; o += 20 * nopen
    npend, = struct.unpack_from('<I', b, o); o += 4
    pend = {}
    for i in range(npend):
        p, = struct.unpack_from('<Q', b, o)
        pend[p] = b[o + 8:o + 8 + PG]; o += 8 + PG
    tl, = struct.unpack_from('<H', b, o); o += 2
    tail = b[o:o + tl]; o += tl
    T = L // PG
    assert F == T and tl == L % PG
    chain_g = cg if cg != 0xFFFFFFFF else -1
    # walk the tag chain back from the record's (g, ctr, MAC)
    gtags = {}
    mac, ctr = cmac, cctr
    for g in range(chain_g, -1, -1):
        tp = stored[tagpage_off(g):tagpage_off(g) + PG]
        assert tp[PG - 16:] == mac, ('tag page mac', g)
        prev_mac = stored[tagpage_off(g - 1) + PG - 16:tagpage_off(g - 1) + PG] if g > 0 else b'\0' * 16
        iv = b'\x12' + struct.pack('<Q', g)[:7] + struct.pack('<I', ctr)
        aad = fid + struct.pack('<QI', g, ctr) + prev_mac
        pt = aes.decrypt(iv, tp[:PG - 16] + tp[PG - 16:], aad)
        for i in range(128):
            gtags[g * 128 + i] = pt[i * 24:i * 24 + 20]
        mac, ctr = prev_mac, struct.unpack_from('<I', pt, 128 * 24)[0]
    oi = 0
    for p in range((chain_g + 1) * 128, T):
        if p not in pend:
            gtags[p] = opens[oi]; oi += 1
    assert oi == nopen
    out = bytearray()
    for p in range(T):
        if p in pend:
            out += pend[p]
            continue
        e = gtags[p]
        ctrp, = struct.unpack_from('<I', e, 16)
        out += page_dec(aes, fid, p, ctrp, stored[data_off(p):data_off(p) + PG], e[:16])
    out += tail
    return bytes(out), f"records={len(recs)} newest seq={seq} kind={kind} at={'ring' if off < (1 << 20) else 'data'} L={L} T={T} pend={npend} open={nopen} chain_g={chain_g}"


def jrp():
    fid, sk = keys[:16], keys[16:48]
    aes = AESGCM(sk)
    rps = {}
    for slot in range(256, len(stored) // PG + 1):
        off = slot * PG
        h = stored[off:off + 32]
        if h[:8] != b'SUPDBRP1':
            continue
        nrp, = struct.unpack_from('<H', h, 8)
        seq, = struct.unpack_from('<Q', h, 11)
        att, blen = struct.unpack_from('<II', h, 19)
        iv = b'\x14' + struct.pack('<Q', seq) + b'\0\0\0'
        aad = fid + b'rp' + struct.pack('<Q', slot) + h
        try:
            body = aes.decrypt(iv, stored[off + 32:off + 32 + blen + 16], aad)
        except Exception:
            continue
        rps[seq] = (slot, nrp, body)
    out = bytearray()
    last_seq, L = -1, 0
    H = 0
    tail = b''
    for seq in sorted(rps):
        slot, nrp, b = rps[seq]
        L2, home, first, ctr0 = struct.unpack_from('<QQQI', b, 0)
        np_ = home - first
        tags = [b[28 + 16 * k:44 + 16 * k] for k in range(np_)]
        o = 28 + 16 * np_
        dead, prev = struct.unpack_from('<QQ', b, o); o += 16
        novr, = struct.unpack_from('<I', b, o); o += 4
        tl, = struct.unpack_from('<H', b, o); o += 2
        assert first == H, ('rp chain', seq, first, H)
        dead_before = dead - nrp
        for k in range(np_):
            p = first + k
            s = 256 + p + dead_before
            out[p * PG:(p + 1) * PG] = page_dec(aes, fid, p, ctr0 + k, stored[s * PG:(s + 1) * PG], tags[k])
        assert slot == 256 + home + dead_before
        H = home
        del out[H * PG:]
        tail = b[o:o + tl]
        out += tail
        L = L2
        last_seq = seq
    # ring frames with seq past the newest RP, contiguous, each at the current length
    frames = {}
    i = PG
    while True:
        i = stored.find(b'\x46\x4a\x01', i, 1 << 20)
        if i < 0:
            break
        h = stored[i:i + 16]
        seq, blen = struct.unpack_from('<QI', h, 3)
        iv = b'\x11' + struct.pack('<Q', seq) + b'\0\0\0'
        try:
            body = aes.decrypt(iv, stored[i + 16:i + 16 + blen + 16], fid + b'fr' + h)
            frames[seq] = body
        except Exception:
            pass
        i += 1
    s = last_seq + 1
    nf = 0
    while s in frames:
        frm, = struct.unpack_from('<Q', frames[s], 0)
        assert frm == L, ('frame from', s, frm, L)
        out += frames[s][8:]
        L += len(frames[s]) - 8
        s += 1
        nf += 1
    return bytes(out), f"rps={len(rps)} frames_found={len(frames)} applied={nf} L={L} H={H}"


def swal(kind):
    key, fid, store_id, prefix, tag0 = keys[:32], keys[32:48], keys[48:64], keys[64:72], keys[72:88]
    aes = AESGCM(key)
    hn = {'D': 68, 'J': 84, 'S': 72}[kind]
    o, prev, out, n = hn, tag0, bytearray(), 0
    while o + 32 <= len(stored):
        ln, = struct.unpack_from('<I', stored, o)
        if ln == 0:
            break
        if kind == 'J':
            head = stored[o:o + 15]
            seq, = struct.unpack_from('<Q', head, 4)
            iv = b'\x13' + struct.pack('<Q', seq) + b'\0\0\0'
            aad = fid + head + prev
            hl = 15
        else:
            iv = stored[o + 4:o + 16]
            hl = 16
            aad = struct.pack('<QQI', 7, o, ln) if kind == 'D' else b'pfs' + store_id + fid + struct.pack('<QI', o, ln) + prev
        ct = stored[o + hl:o + hl + ln + 16]
        out += aes.decrypt(iv, ct, aad)
        prev = ct[-16:]
        o += hl + ln + 16
        n += 1
    return bytes(out), f"records={n}"


k = shape[0]
got, info = adj() if k in 'OT' else jrp() if k == 'R' else swal('D' if k == 'D' else k)
ok = got == want
print(f"{shape} {'OK' if ok else 'MISMATCH'} plain={len(want)} rebuilt={len(got)} {info}")
if not ok:
    for i in range(min(len(got), len(want))):
        if got[i] != want[i]:
            print('first difference at', i, 'page', i // PG)
            break
    sys.exit(1)
