#!/usr/bin/env python3
"""Build a Steam Deck panel EDID with extra refresh rates.

Usage: gen_deck_edid.py [--profile NAME] <original.bin> <output.bin> [rate ...]

<original.bin> is the panel's factory EDID. On the Deck itself that is
/sys/class/drm/card0-eDP-1/edid, which can be passed directly (sysfs reports
size 0 but reads fine), as long as no modified EDID is active (no
drm.edid_firmware / override). The output keeps all original blocks byte for
byte; only the extension count and checksum of block 0 change, and the new
DisplayID blocks are appended.

The panel is detected from the EDID's vendor/product (override with
--profile).  The extra modes use the same timings gamescope generates for that
panel (scripts/00-gamescope/displays/valve.steamdeck.{oled,lcd}.lua):

  oled-sdc  VLV 0x3003  Samsung OLED  fixed 133.2 MHz clock, VFP from Valve's table
  oled-boe  VLV 0x3004  BOE OLED      fixed 102.0 MHz clock, VFP from Valve's table
  lcd       VLV 0x3001  LCD           fixed 884x1322 total (h 40/4/40, v 30/4/8),
            VLV 0x3002                pixel clock scaled per rate (calc_max_clock)
            ANX 0x7539                (older LCD firmware ID, same timing as 0x3001)

The modes are emitted as DisplayID 1.3 Type I detailed timings in extra
extension blocks (an EDID DTD can only encode a 6-bit VFP, max 63).  The
original blocks are kept intact, so the native mode stays the preferred one.
"""
import sys

# gamescope OLED tables, index = refresh - 45
SDC_VFPS = [
    1321, 1264, 1209, 1157, 1106,
    1058, 993, 967, 925, 883, 829, 805, 768, 732, 698,
    665, 632, 601, 571, 542, 501, 486, 459, 433, 408,
    383, 360, 337, 314, 292, 271, 250, 230, 210, 191,
    173, 154, 137, 119, 102, 86, 70, 54, 38, 23,
    9,
]
BOE_VFPS = [
    1320, 1272, 1216, 1156, 1112,
    1064, 992, 972, 928, 888, 828, 808, 772, 736, 700,
    664, 636, 604, 572, 544, 500, 488, 460, 436, 408,
    384, 360, 336, 316, 292, 272, 252, 228, 212, 192,
    172, 152, 136, 120, 100, 84, 68, 52, 36, 20,
    8,
]
# rates gamescope offers on the OLED (others are deliberately skipped by Valve)
OLED_RATES = {45, 47, 48, 49, 50, 51, 53, 55, 56, 59, 60, 62, 64, 65, 66, 68,
              72, 73, 76, 77, 78, 80, 81, 82, 84, 85, 86, 87, 88}

# known EDID identities (vendor, product) -> native timing:
# (clock in 10 kHz, (hact, hfp, hsw, hbp), (vact, vfp, vsw, vbp))
PROFILES = {
    "oled-sdc": dict(
        products={("VLV", 0x3003): (13320, (800, 276, 8, 44), (1280, 9, 1, 22))},
        kind="vfp", vfps=SDC_VFPS, rates=OLED_RATES,
        default=[45, 48, 50, 55, 60, 66, 72, 76, 80, 85]),
    "oled-boe": dict(
        products={("VLV", 0x3004): (10200, (800, 18, 4, 36), (1280, 8, 2, 30))},
        kind="vfp", vfps=BOE_VFPS, rates=OLED_RATES,
        default=[45, 48, 50, 55, 60, 66, 72, 76, 80, 85]),
    "lcd": dict(
        products={("VLV", 0x3001): (6927, (800, 32, 20, 20), (1280, 16, 2, 26)),
                  ("VLV", 0x3002): (7011, (800, 40, 4, 40), (1280, 30, 4, 8)),
                  ("ANX", 0x7539): (6927, (800, 32, 20, 20), (1280, 16, 2, 26))},
        kind="clock", h=(800, 40, 4, 40), v=(1280, 30, 4, 8),
        # gamescope offers 40-60; lower rates are untested by Valve
        rates=set(range(30, 60)), tested=set(range(40, 61)),
        default=[30, 35, 40, 45, 48, 50, 55]),
}
PER_BLOCK = 5  # 3 + 5*20 = 103 bytes, max 121 fit in one 128-byte extension


def parse_dtd(d):
    clock = d[0] | d[1] << 8  # 10 kHz units
    hact = d[2] | (d[4] & 0xF0) << 4
    hbl = d[3] | (d[4] & 0x0F) << 8
    vact = d[5] | (d[7] & 0xF0) << 4
    vbl = d[6] | (d[7] & 0x0F) << 8
    hfp = d[8] | (d[11] & 0xC0) << 2
    hsw = d[9] | (d[11] & 0x30) << 4
    vfp = d[10] >> 4 | (d[11] & 0x0C) << 2
    vsw = d[10] & 0x0F | (d[11] & 0x03) << 4
    flags = d[17]
    assert flags & 0x18 == 0x18, "expected digital separate sync"
    return dict(clock=clock,
                h=(hact, hfp, hsw, hbl - hfp - hsw),
                v=(vact, vfp, vsw, vbl - vfp - vsw),
                hpos=bool(flags & 0x02), vpos=bool(flags & 0x04))


def refresh(clock, h, v):
    return clock * 1e4 / (sum(h) * sum(v))


def displayid_type1(clock, h, v, hpos, vpos):
    le16 = lambda x: [x & 0xFF, x >> 8]
    clk = clock - 1
    return bytes(
        [clk & 0xFF, clk >> 8 & 0xFF, clk >> 16, 0x08]  # flags: aspect undefined
        + le16(h[0] - 1) + le16(h[1] + h[2] + h[3] - 1)
        + le16((h[1] - 1) | (0x8000 if hpos else 0)) + le16(h[2] - 1)
        + le16(v[0] - 1) + le16(v[1] + v[2] + v[3] - 1)
        + le16((v[1] - 1) | (0x8000 if vpos else 0)) + le16(v[2] - 1)
    )


def displayid_ext(timings, prod_type, ext_count):
    payload = bytes([0x03, 0x00, 20 * len(timings)]) + b"".join(timings)
    # DisplayID 1.3 section header: version, length, product type, ext count
    section = bytes([0x13, len(payload), prod_type, ext_count]) + payload
    section += bytes([-sum(section) & 0xFF])
    blk = bytearray(bytes([0x70]) + section)
    blk += bytes(127 - len(blk))
    blk.append(-sum(blk) & 0xFF)
    return bytes(blk)


def make_mode(prof, base, r):
    """Mode for rate r, exactly as gamescope's dynamic_modegen builds it."""
    if prof["kind"] == "vfp":
        # adjust_front_porch: keep vsync width and back porch, set VFP
        vact, _, vsw, vbp = base["v"]
        return base["clock"], base["h"], (vact, prof["vfps"][r - 45], vsw, vbp)
    # set_h_timings/set_v_timings + calc_max_clock (kHz, rounded up),
    # then rounded up to the 10 kHz unit of a Type I timing
    h, v = prof["h"], prof["v"]
    khz = (sum(h) * sum(v) * r + 999) // 1000
    return -(-khz // 10), h, v


def main():
    args = sys.argv[1:]
    name = None
    if args[:1] == ["--profile"]:
        name, args = args[1], args[2:]
    src, dst, *rates = args

    edid = bytearray(open(src, "rb").read())
    nblocks = edid[0x7E] + 1
    assert edid[:8] == bytes.fromhex("00ffffffffffff00"), "not an EDID"
    edid = edid[:128 * nblocks]  # drop duplicated dumps (linuxhw)
    if any(edid[128 * i] == 0x70 for i in range(1, nblocks)):
        sys.exit("source already has DisplayID blocks: it is a generated EDID, "
                 "not the factory one (is drm.edid_firmware active?)")
    # vendor: three letters packed 5 bits each (A=1), big endian
    v = edid[8] << 8 | edid[9]
    ident = ("".join(chr(64 + (v >> s & 0x1F)) for s in (10, 5, 0)),
             edid[10] | edid[11] << 8)
    idstr = f"{ident[0]} 0x{ident[1]:04x}"

    if name is None:
        name = next((n for n, p in PROFILES.items()
                     if ident in p["products"]), None)
        if name is None:
            sys.exit(f"unknown panel ({idstr}); use --profile")
    prof = PROFILES[name]
    base = parse_dtd(edid[0x36:0x48])
    known = prof["products"].get(ident)
    if known:
        assert (base["clock"], base["h"], base["v"]) == known, \
            f"unexpected native timing for {idstr}"
    else:
        print(f"warning: {idstr} unknown to profile {name}, "
              "native timing not checked")

    native = round(refresh(base["clock"], base["h"], base["v"]))
    rates = [int(r) for r in rates] or prof["default"]
    print(f"profile {name}, {idstr}, native {native} Hz")
    timings = []
    for r in rates:
        if r not in prof["rates"]:
            sys.exit(f"{r} Hz is not supported by profile {name}"
                     f" (or is the native rate)")
        clock, h, v = make_mode(prof, base, r)
        note = "" if r in prof.get("tested", prof["rates"]) else \
            "  (untested by Valve)"
        print(f"{r:3d} Hz: clock {clock / 100:7.2f} MHz  "
              f"htotal {sum(h)}  vfp {v[1]:4d}  vtotal {sum(v)}  "
              f"-> {refresh(clock, h, v):.3f} Hz{note}")
        timings.append(displayid_type1(clock, h, v, base["hpos"], base["vpos"]))

    chunks = [timings[i:i + PER_BLOCK] for i in range(0, len(timings), PER_BLOCK)]
    # first section is the DisplayID base (product type 2 = display panel),
    # the rest are its extension sections (product type 0)
    exts = [displayid_ext(c, 0x02, len(chunks) - 1) if i == 0 else
            displayid_ext(c, 0x00, 0) for i, c in enumerate(chunks)]
    edid[0x7E] = nblocks - 1 + len(exts)
    edid[0x7F] = -sum(edid[:0x7F]) & 0xFF
    out = bytes(edid) + b"".join(exts)
    open(dst, "wb").write(out)
    print(f"wrote {dst} ({len(out)} bytes, {edid[0x7E]} extension blocks)")


if __name__ == "__main__":
    main()
