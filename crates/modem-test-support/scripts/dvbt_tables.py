import argparse
import filecmp
import re
import subprocess
import sys
import tempfile
from pathlib import Path

import dvbs2_spec

RUSTFMT_CONFIG = Path(__file__).resolve().parents[3] / "rustfmt.toml"
FFT_SIZES = ["1K", "2K", "4K", "8K", "16K", "32K"]
PATTERNS = [f"PP{n}" for n in range(1, 9)]


def integers(text):
    return [int(n) for n in re.findall(r"\d+", text)]


def body(standard, name):
    return [line for line in standard.table(name) if "ETSI" not in line]


def permutations(standard, name, groups):
    values = []
    for line in body(standard, name):
        if "π" in line or re.search(r"Modulation|Code rate|Order of", line):
            continue
        line = re.sub(r"\d+-QAM|\d+/\d+\s+\d+|\d+/\d+", " ", line)
        values += integers(line)
    rows = [values[i : i + groups] for i in range(0, len(values), groups)]
    for row in rows:
        if sorted(row) != list(range(groups)):
            raise ValueError(f"Table {name} row {row} is not a permutation")
    return rows


def padding_orders(standard, name):
    groups = set()
    for line in body(standard, name):
        match = re.search(r"\d+/\d+\s+(\d+)", line)
        if match:
            groups.add(int(match.group(1)))
    if len(groups) != 1:
        raise ValueError(f"Table {name}: Ngroup {groups}")
    return permutations(standard, name, groups.pop())


def puncturing_orders(standard, name):
    groups = re.search(r"Qldpc\s*=\s*(\d+)", " ".join(body(standard, name))).group(1)
    return permutations(standard, name, int(groups))


def demux(standard, name):
    result = {}
    lines = body(standard, name)
    for index, line in enumerate(lines):
        match = re.search(r"Modulation format\s+(.+?)\s*$", line)
        if not match:
            continue
        inputs = next(integers(text) for text in lines[index + 1 :] if "di mod" in text)
        start = next(i for i in range(index + 1, len(lines)) if "Output bit-number" in lines[i])
        outputs = next(integers(text) for text in lines[start + 1 :] if text.strip())
        if inputs != list(range(len(outputs))) or sorted(outputs) != inputs:
            raise ValueError(f"Table {name} {match.group(1)}: {inputs} {outputs}")
        result[match.group(1)] = outputs
    return result


def pn_sequence(standard):
    digits = "".join(line.strip() for line in body(standard, "56:") if re.fullmatch(r"\s*[0-9A-F]+\s*", line))
    if len(digits) * 4 != 2624:
        raise ValueError(f"Table 56: {len(digits) * 4} chips")
    return list(bytes.fromhex(digits))


def p1_carriers(standard):
    values = []
    for line in body(standard, "68:"):
        line = re.sub(r"kP1\(\d+\)\.\.kP1\(\d+\)|CSSS\d", " ", line)
        if re.fullmatch(r"[\d\s]+", line):
            values += integers(line)
    if len(values) != 384 or values != sorted(set(values)):
        raise ValueError("Table 68: carrier list")
    return values


def modulation_patterns(standard):
    patterns = {"S1": {}, "S2": {}}
    for line in standard.table("69:"):
        match = re.fullmatch(r"\s*(?:S[12]\s+)?([01]{3,4})\s+([0-9A-F]+)\s*", line)
        if match:
            field = "S1" if len(match.group(1)) == 3 else "S2"
            patterns[field][int(match.group(1), 2)] = list(bytes.fromhex(match.group(2)))
    if [len(patterns["S1"]), len(patterns["S2"])] != [8, 16]:
        raise ValueError("Table 69: pattern count")
    return [patterns["S1"][i] for i in range(8)], [patterns["S2"][i] for i in range(16)]


def reserved_carriers(standard, name):
    text = " ".join(body(standard, name))
    labels = re.findall(r"(\d+K) \((\d+)\)", text)
    values = integers(re.sub(r"\d+K \(\d+\)", " ", text))
    if [label for label, _ in labels] != FFT_SIZES or len(values) != sum(int(n) for _, n in labels):
        raise ValueError(f"Table {name}: {labels} {len(values)}")
    result = []
    for _, count in labels:
        result.append(values[: int(count)])
        values = values[int(count) :]
        if result[-1] != sorted(set(result[-1])):
            raise ValueError(f"Table {name}: unsorted")
    return result


def rows(words):
    result = []
    for word in sorted(words, key=lambda w: w[1]):
        if result and word[1] - result[-1][0][1] <= 3:
            result[-1].append(word)
        else:
            result.append([word])
    return [sorted(row) for row in result]


def pattern_table(standard, caption, header, label, end=None):
    first = standard.page(caption)
    last = standard.page(end) if end else first
    blocks = []
    for index in range(first, last + 1):
        words = standard.page_words(index)
        start = max((w[1] for w in words if index == first and w[4] == caption.split()[1]), default=0)
        top = next(w[1] for w in words if w[4] == header and w[1] > start)
        columns = sorted(w for w in words if w[4] in PATTERNS and abs(w[1] - top) < 2)
        marks = {"ETSI", end.split()[1]} if end and index == last else {"ETSI"}
        bottom = min((w[1] for w in words if w[1] > top and w[4] in marks), default=float("inf"))
        for row in rows([w for w in words if top + 2 < w[1] < bottom]):
            for word in row:
                if word[2] < columns[0][0] - 10:
                    if re.match(label, word[4]):
                        blocks.append(["", {}])
                    blocks[-1][0] += word[4] + " "
                elif word[4].isdigit():
                    column = min(columns, key=lambda c: abs(c[0] - word[0]))[4]
                    blocks[-1][1].setdefault(column, []).append(int(word[4]))
    return [(name.strip(), [cells.get(p, []) for p in PATTERNS]) for name, cells in blocks]


def continual_pilot_groups(standard):
    blocks = pattern_table(standard, "Table G.1: Continual", "Group", r"CP", "Table G.2:")
    names = [re.match(r"CP (\d)", name).group(1) for name, _ in blocks]
    if names != [str(g) for g in range(1, 7)]:
        raise ValueError(f"Table G.1: groups {names}")
    return [columns for _, columns in blocks]


def extended_continual_pilots(standard):
    blocks = pattern_table(standard, "Table G.2: Locations", "FFT", r"\d+K$")
    if [name for name, _ in blocks] != FFT_SIZES[3:]:
        raise ValueError(f"Table G.2: sizes {[name for name, _ in blocks]}")
    return [columns for _, columns in blocks]


def numbers(row):
    merged = []
    for word in row:
        if merged and re.fullmatch(r"\d{3}", word[4]) and word[0] - merged[-1][2] < 4:
            merged[-1] = (merged[-1][0], word[1], word[2], word[3], merged[-1][4] + word[4])
        else:
            merged.append(word)
    return [((w[0] + w[2]) / 2, int(w[4])) for w in merged]


def header_center(words, head):
    mode = min((w for w in words if w[4] == "mode" and w[0] > head[0] and abs(w[1] - head[1]) < 2), key=lambda w: w[0])
    return (head[0] + mode[2]) / 2


def mode_split(standard, caption):
    words = standard.words(caption)
    title = next(w for w in words if w[4] == caption.split()[1])
    heads = {w[4]: w for w in words if w[4] in ("2K", "8K") and w[1] > title[1]}
    centers = [header_center(words, heads[mode]) for mode in ("2K", "8K")]
    lines = []
    for row in rows([w for w in words if w[1] > heads["2K"][1] + 2]):
        if not all(re.fullmatch(r"\d+", w[4]) for w in row):
            break
        lines.append(numbers(row))
    clusters = []
    for x in sorted(x for line in lines for x, _ in line):
        if clusters and x - clusters[-1][-1] < 4:
            clusters[-1].append(x)
        else:
            clusters.append([x])
    positions = [sum(c) / len(c) for c in clusters]

    def cost(split):
        left, right = positions[:split], positions[split:]
        return abs((left[0] + left[-1]) / 2 - centers[0]) + abs((right[0] + right[-1]) / 2 - centers[1])

    split = min(range(1, len(positions)), key=cost)
    boundary = (positions[split - 1] + positions[split]) / 2
    modes = [[v for line in lines for x, v in line if (x > boundary) == right] for right in (False, True)]
    for mode in modes:
        if mode != sorted(set(mode)):
            raise ValueError(f"{caption}: unsorted")
    return modes


def rust_list(values):
    return "[" + ", ".join(map(str, values)) + "]"


def rust_bytes(values):
    return "[" + ", ".join(f"0x{v:02X}" for v in values) + "]"


def rust_slices(values):
    return "[" + ", ".join("&" + rust_list(v) for v in values) + "]"


def constant(name, kind, value):
    return f"pub const {name}: {kind} = {value};\n"


def flat(rows):
    return [value for row in rows for value in row]


def load(cache):
    t2 = dvbs2_spec.Standard(cache, "t2")
    t = dvbs2_spec.Standard(cache, "t")
    satellite = dvbs2_spec.ldpc_tables(dvbs2_spec.Standard(cache, "s2"))
    ldpc = {}
    for (rate, size), table in sorted(dvbs2_spec.ldpc_tables(t2, "AB").items()):
        rows = [values for values, _ in table.lines]
        if (rate, size) in satellite and flat(rows) == flat(v for v, _ in satellite[(rate, size)].lines):
            continue
        numerator, denominator = map(int, rate.split("_"))
        if len(rows) * 360 * denominator != dvbs2_spec.LENGTHS[size] * numerator:
            raise ValueError(f"Table {table.table}: {len(rows)} rows")
        ldpc[f"{'NORMAL' if size == 'N' else 'SHORT'}_R{rate}"] = rows
    return t2, t, ldpc


def files(t2, t, ldpc):
    groups = continual_pilot_groups(t2)
    extended = extended_continual_pilots(t2)
    s1, s2 = modulation_patterns(t2)
    demuxes = demux(t2, "13(a):")
    pre_padding = padding_orders(t2, "40:")
    post_padding = padding_orders(t2, "41:")
    pre_puncturing = puncturing_orders(t2, "42:")
    post_puncturing = puncturing_orders(t2, "43:")
    continual_2k, continual_8k = mode_split(t, "Table 7: Carrier indices")
    tps_2k, tps_8k = mode_split(t, "Table 8: Carrier indices")
    ldpc_file = ""
    for name, table in ldpc.items():
        rows = "".join("&" + rust_list(row) + ", " for row in table)
        ldpc_file += f"pub static {name}: [&[u16]; {len(table)}] = [{rows}];\n"
    return {
        "t2/en302755/mod.rs": "".join(f"pub mod {m};\n" for m in ("demux", "l1", "ldpc", "p1", "papr", "pilots")),
        "t2/en302755/demux.rs": constant("QAM16", "[usize; 8]", rust_list(demuxes["16-QAM"]))
        + constant("QAM64", "[usize; 12]", rust_list(demuxes["64-QAM"])),
        "t2/en302755/l1.rs": constant("PRE_PADDING_ORDER", "[usize; 9]", rust_list(pre_padding[0]))
        + constant("POST_PADDING_ORDER", "[[usize; 20]; 3]", rust_list(map(rust_list, post_padding)))
        + constant("PRE_PUNCTURING_ORDER", "[usize; 36]", rust_list(pre_puncturing[0]))
        + constant("POST_PUNCTURING_ORDER", "[[usize; 25]; 3]", rust_list(map(rust_list, post_puncturing))),
        "t2/en302755/ldpc.rs": ldpc_file,
        "t2/en302755/p1.rs": constant("ACTIVE_CARRIERS", "[usize; 384]", rust_list(p1_carriers(t2)))
        + constant("CSS_S1", "[[u8; 8]; 8]", rust_list(map(rust_bytes, s1)))
        + constant("CSS_S2", "[[u8; 32]; 16]", rust_list(map(rust_bytes, s2))),
        "t2/en302755/papr.rs": constant(
            "P2_RESERVED_CARRIERS", "[&[usize]; 6]", rust_slices(reserved_carriers(t2, "H.1:"))
        )
        + constant("RESERVED_CARRIERS", "[&[usize]; 6]", rust_slices(reserved_carriers(t2, "H.2:"))),
        "t2/en302755/pilots.rs": constant("PN_SEQUENCE", "[u8; 328]", rust_bytes(pn_sequence(t2)))
        + constant("CONTINUAL_PILOT_GROUPS", "[[&[usize]; 8]; 6]", rust_list(map(rust_slices, groups)))
        + constant("EXTENDED_CONTINUAL_PILOTS", "[[&[usize]; 8]; 3]", rust_list(map(rust_slices, extended))),
        "en300744.rs": constant("CONTINUAL_PILOTS_2K", f"[usize; {len(continual_2k)}]", rust_list(continual_2k))
        + constant("CONTINUAL_PILOTS_8K", f"[usize; {len(continual_8k)}]", rust_list(continual_8k))
        + constant("TPS_CARRIERS_2K", f"[usize; {len(tps_2k)}]", rust_list(tps_2k))
        + constant("TPS_CARRIERS_8K", f"[usize; {len(tps_8k)}]", rust_list(tps_8k)),
    }


def generate(root, sources):
    for path, text in sources.items():
        (root / path).parent.mkdir(parents=True, exist_ok=True)
        (root / path).write_text(text)
    paths = [str(root / path) for path in sources]
    subprocess.run(["rustfmt", "--edition", "2024", "--config-path", str(RUSTFMT_CONFIG), *paths], check=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cache", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    sources = files(*load(args.cache))
    if not args.check:
        generate(args.out, sources)
        return
    with tempfile.TemporaryDirectory(prefix="dvbt-tables-") as temporary:
        generate(Path(temporary), sources)
        mismatched = [
            path
            for path in sources
            if not (args.out / path).exists() or not filecmp.cmp(Path(temporary) / path, args.out / path, shallow=False)
        ]
    for path in mismatched:
        print(f"differs from ETSI EN 302 755 / EN 300 744: {path}", file=sys.stderr)
    sys.exit(1 if mismatched else 0)


if __name__ == "__main__":
    main()
