"""Minimal SFNT surgery, for building the malformed fonts a scanner's
fallback paths only run for.

fontTools would be the obvious tool and is not always installed; everything
here is done with `struct` so the harnesses have no dependency beyond python3.

A table is replaced by appending the new bytes at the end of the file and
repointing its directory entry, which keeps every other table's offset valid.
"""

import struct


def tables(data):
    """Every table as {tag: (offset, length)}."""
    count = struct.unpack(">H", data[4:6])[0]
    out = {}
    for i in range(count):
        at = 12 + i * 16
        tag, _checksum, offset, length = struct.unpack(">4sIII", data[at:at + 16])
        out[tag.decode("latin1")] = (offset, length)
    return out


def _checksum(block):
    block = block + b"\0" * ((4 - len(block) % 4) % 4)
    total = 0
    for i in range(0, len(block), 4):
        total = (total + struct.unpack(">I", block[i:i + 4])[0]) & 0xFFFFFFFF
    return total


def replace_table(data, tag, new):
    """`data` with `tag`'s contents replaced, appended at the end."""
    count = struct.unpack(">H", data[4:6])[0]
    tag = tag.encode("latin1") if isinstance(tag, str) else tag
    out = bytearray(data)
    out += b"\0" * ((4 - len(out) % 4) % 4)
    offset = len(out)
    out += new + b"\0" * ((4 - len(new) % 4) % 4)
    for i in range(count):
        at = 12 + i * 16
        if bytes(out[at:at + 4]) == tag:
            struct.pack_into(">III", out, at + 4, _checksum(new), offset, len(new))
            return bytes(out)
    raise KeyError(f"no {tag!r} table")


def drop_table(data, tag):
    """`data` without `tag`'s directory entry.

    The table's bytes stay where they are and become dead space; every other
    entry keeps the offset it had, which is the point of doing it this way.
    """
    count = struct.unpack(">H", data[4:6])[0]
    tag = tag.encode("latin1") if isinstance(tag, str) else tag
    entries = [data[12 + i * 16:12 + (i + 1) * 16] for i in range(count)]
    kept = [e for e in entries if e[:4] != tag]
    if len(kept) == count:
        raise KeyError(f"no {tag!r} table")
    head = bytearray(data[:12])
    struct.pack_into(">H", head, 4, len(kept))
    # searchRange/entrySelector/rangeShift are advisory; FreeType recomputes.
    body = b"".join(kept)
    rest = data[12 + count * 16:]
    return bytes(head) + body + b"\0" * 16 + rest


def patch_u16(data, tag, offset, value):
    """Set one big-endian u16 inside a table."""
    at, _length = tables(data)[tag]
    out = bytearray(data)
    struct.pack_into(">H", out, at + offset, value)
    return bytes(out)


def read_u16(data, tag, offset):
    at, _length = tables(data)[tag]
    return struct.unpack(">H", data[at + offset:at + offset + 2])[0]


def name_table(records):
    """A format 0 `name` table from [(platform, encoding, language, id, text)]."""
    storage = b""
    entries = []
    for platform, encoding, language, name_id, text in records:
        raw = text.encode("utf-16-be" if platform == 3 else "latin1")
        entries.append(struct.pack(">HHHHHH", platform, encoding, language,
                                   name_id, len(raw), len(storage)))
        storage += raw
    header = struct.pack(">HHH", 0, len(entries), 6 + 12 * len(entries))
    return header + b"".join(entries) + storage


def set_names(data, records):
    return replace_table(data, "name", name_table(records))


# Field offsets, from the OpenType spec.
OS2_VERSION = 0
OS2_WEIGHT_CLASS = 4
OS2_WIDTH_CLASS = 6
OS2_FS_SELECTION = 62
HEAD_MAC_STYLE = 44


def fvar(axes, instances):
    """An `fvar` table.

    `axes` is [(tag, min, default, max, name_id)] in points/user units;
    `instances` is [(subfamily_name_id, [coord, ...])].
    """
    count = len(axes)
    # majorVersion, minorVersion, axesArrayOffset, reserved, axisCount,
    # axisSize, instanceCount, instanceSize. An instance record is the
    # subfamily name id, its flags, and one Fixed per axis.
    header = struct.pack(">HHHHHHHH", 1, 0, 16, 2, count, 20,
                         len(instances), 4 + 4 * count)
    body = b""
    for tag, lo, default, hi, name_id in axes:
        body += struct.pack(">4siiiHH", tag.encode("latin1"),
                            int(lo * 65536), int(default * 65536), int(hi * 65536),
                            0, name_id)
    for subfamily, coords in instances:
        body += struct.pack(">HH", subfamily, 0)
        body += b"".join(struct.pack(">i", int(c * 65536)) for c in coords)
    return header + body


def add_table(data, tag, new):
    """`data` with `tag` added as a new directory entry.

    The directory grows by one entry, which moves everything after it, so each
    surviving entry's offset is bumped -- unlike `replace_table`, which can
    leave them all alone.
    """
    count = struct.unpack(">H", data[4:6])[0]
    tag = tag.encode("latin1") if isinstance(tag, str) else tag
    entries = [bytearray(data[12 + i * 16:12 + (i + 1) * 16]) for i in range(count)]
    shift = 16
    for entry in entries:
        offset = struct.unpack(">I", entry[8:12])[0]
        struct.pack_into(">I", entry, 8, offset + shift)
    head = bytearray(data[:12])
    struct.pack_into(">H", head, 4, count + 1)
    rest = data[12 + count * 16:]
    out = bytearray(bytes(head) + b"".join(bytes(e) for e in entries) + b"\0" * 16 + rest)
    out += b"\0" * ((4 - len(out) % 4) % 4)
    offset = len(out)
    out += new + b"\0" * ((4 - len(new) % 4) % 4)
    at = 12 + count * 16
    struct.pack_into(">4sIII", out, at, tag, _checksum(new), offset, len(new))
    return bytes(out)
