import argparse
import re
import sys
from fractions import Fraction
from pathlib import Path

import dvbs2_spec

FILES = {"normal.rs": "N", "short.rs": "S", "medium.rs": "M"}


def rust_tables(folder):
    tables = {}
    for file, size in FILES.items():
        text = (folder / "tables" / file).read_text()
        for match in re.finditer(r"pub static R(\d+_\d+): \[&\[u16\]; \d+\] = \[(.*?)\n\];", text, re.S):
            rows = [[int(n) for n in re.findall(r"\d+", row)] for row in re.findall(r"&\[([^\]]*)\]", match.group(2))]
            tables[(match.group(1), size)] = rows
    return tables


def ends(rows):
    positions = set()
    total = 0
    for row in rows:
        total += len(row)
        positions.add(total)
    return positions


def kldpc(s2):
    result = {}
    for table, size in (("5a:", "N"), ("5b:", "S")):
        for row in s2.rows(table):
            if re.fullmatch(r"\d+/\d+", row[0]) and re.fullmatch(r"\d+ \d+", row[2]):
                result[(row[0].replace("/", "_"), size)] = int(row[2].replace(" ", ""))
    return result


def check_ldpc(s2, s2x, folder):
    errors = []
    spec = {**dvbs2_spec.ldpc_tables(s2x), **dvbs2_spec.ldpc_tables(s2)}
    lengths = kldpc(s2)
    for key, rows in rust_tables(folder).items():
        table = spec.get(key)
        if table is None:
            errors.append(f"LDPC {key}: not in ETSI text")
            continue
        flat = [value for values, _ in table.lines for value in values]
        if [value for row in rows for value in row] != flat:
            errors.append(f"LDPC {key}: addresses differ from Table {table.table}")
        if not dvbs2_spec.ldpc_breaks(table) <= ends(rows):
            errors.append(f"LDPC {key}: row boundaries differ from Table {table.table}")
        if key in lengths and len(rows) * 360 != lengths[key]:
            errors.append(f"LDPC {key}: {len(rows)} rows, kldpc {lengths[key]}")
    return errors


def rust_array(source, name):
    match = re.search(rf"const {name}: \[\w+; \d+\] = \[(.*?)\];", source, re.S)
    return [Fraction(value) for value in re.findall(r"-?\d+(?:\.\d+)?", match.group(1))]


def rust_ratios(source, function):
    body = re.search(rf"fn {function}\(rate: Rate\).*?\n}}", source, re.S).group(0)
    arms = re.search(r"let [^=]+ = match rate \{(.*?)\n    \};", body, re.S).group(1)
    ratios = {}
    for rate, pair, single in re.findall(r"(Rate::R\d+_\d+|_) => (?:\(([\d., ]+)\)|([\d.]+)),", arms):
        key = "_" if rate == "_" else rate.removeprefix("Rate::R").replace("_", "/")
        ratios[key] = [float(v) for v in (pair or single).split(",")]
    return ratios


def check_ratios(spec, rust, name):
    errors = []
    listed = {rate for rate in rust if rate != "_"}
    if listed - set(spec):
        errors.append(f"{name}: rates {sorted(listed - set(spec))} not in ETSI text")
    for rate, gammas in spec.items():
        if rust.get(rate, rust["_"]) != gammas:
            errors.append(f"{name} {rate}: {rust.get(rate, rust['_'])} != ETSI {gammas}")
    return errors


def check_labels(rings, source):
    errors = []
    units = {"QPSK_PHASES": 4, "PSK8_PHASES": 4, "APSK16_ANGLES": 12, "APSK32_ANGLES": 24}
    for name, family in (
        ("QPSK_PHASES", "QPSK"),
        ("PSK8_PHASES", "8PSK"),
        ("APSK16_ANGLES", "16APSK"),
        ("APSK32_ANGLES", "32APSK"),
    ):
        angles = [(value / units[name]) % 2 for value in rust_array(source, name)]
        if angles != [angle for _, angle in rings[family]]:
            errors.append(f"{family}: label angles differ from ETSI figure")
    for name, family in (("APSK16_RINGS", "16APSK"), ("APSK32_RINGS", "32APSK")):
        if rust_array(source, name) != [ring for ring, _ in rings[family]]:
            errors.append(f"{family}: label rings differ from ETSI figure")
    return errors


def check_interleaver(s2, source):
    columns = dvbs2_spec.s2_interleaver(s2)
    expected = {
        "(Modulation::Qpsk, _)": [],
        "(Modulation::Psk8, Rate::R3_5)": list(range(columns["8PSK"]))[::-1],
        "(Modulation::Psk8, _)": list(range(columns["8PSK"])),
        "(Modulation::Apsk16, _)": list(range(columns["16APSK"])),
        "(Modulation::Apsk32, _)": list(range(columns["32APSK"])),
    }
    body = re.search(r"fn column_order\(.*?\n}", source, re.S).group(0)
    errors = []
    for arm, order in expected.items():
        match = re.search(rf"{re.escape(arm)} => &\[([\d, ]*)\]", body)
        if match is None or [int(v) for v in re.findall(r"\d+", match.group(1))] != order:
            errors.append(f"interleaver {arm}: expected {order}")
    return errors


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cache", type=Path, required=True)
    parser.add_argument("--dvbs2", type=Path, required=True)
    args = parser.parse_args()
    s2 = dvbs2_spec.Standard(args.cache, "s2")
    s2x = dvbs2_spec.Standard(args.cache, "s2x")
    source = (args.dvbs2 / "frame.rs").read_text()
    errors = check_ldpc(s2, s2x, args.dvbs2)
    errors += check_ratios(dvbs2_spec.ratios(s2, "9:"), rust_ratios(source, "apsk16_radii"), "16APSK")
    errors += check_ratios(dvbs2_spec.ratios(s2, "10:"), rust_ratios(source, "apsk32_radii"), "32APSK")
    errors += check_labels(dvbs2_spec.s2_rings(s2), source)
    errors += check_interleaver(s2, source)
    for error in errors:
        print(error, file=sys.stderr)
    sys.exit(1 if errors else 0)


if __name__ == "__main__":
    main()
