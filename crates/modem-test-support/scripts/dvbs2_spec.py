import hashlib
import math
import re
import struct
import subprocess
import urllib.request
from collections import namedtuple
from fractions import Fraction

DOCUMENTS = {
    "s2": (
        "https://www.etsi.org/deliver/etsi_en/302300_302399/30230701/01.04.01_60/en_30230701v010401p.pdf",
        "19077f80420eb17519fcf6f83e2847d7a877940b134c5306814b8c6f60d8475d",
    ),
    "s2x": (
        "https://www.etsi.org/deliver/etsi_en/302300_302399/30230702/01.04.01_60/en_30230702v010401p.pdf",
        "72b013ca8ca957c26a8e5fc70d491fc5c49065d25ff56ab8ae86087f35664fdd",
    ),
    "t2": (
        "https://www.etsi.org/deliver/etsi_en/302700_302799/302755/01.04.01_60/en_302755v010401p.pdf",
        "976656af03f8ccad658a81045bcca00c85098c6b889ebf5edb58d2a664e6bac4",
    ),
    "t": (
        "https://www.etsi.org/deliver/etsi_en/300700_300799/300744/01.06.02_60/en_300744v010602p.pdf",
        "db653577b021eb4f5fc67aa4fcc20a0fee99a303cd2b93d18027d070b8aa53f7",
    ),
}
SIZES = {"64 800": "N", "16 200": "S", "32 400": "M"}
LENGTHS = {"N": 64800, "S": 16200}
NUMBERS = re.compile(r"\s*\d+(?:\s+\d+)*\s*")
LdpcTable = namedtuple("LdpcTable", "table lines")
WORD = re.compile(r'<word xMin="([\d.]+)" yMin="([\d.]+)" xMax="([\d.]+)" yMax="([\d.]+)">([^<]*)</word>')


class Standard:
    def __init__(self, cache, name):
        url, digest = DOCUMENTS[name]
        cache.mkdir(parents=True, exist_ok=True)
        self.pdf = cache / f"{name}.pdf"
        if not self.pdf.exists():
            request = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
            with urllib.request.urlopen(request) as response:
                self.pdf.write_bytes(response.read())
        if hashlib.sha256(self.pdf.read_bytes()).hexdigest() != digest:
            raise ValueError(f"checksum mismatch: {self.pdf}")
        self.text = subprocess.check_output(["pdftotext", "-layout", str(self.pdf), "-"], text=True)
        self.pages = self.text.split("\f")

    def table(self, name, stop=r"Table |Figure |Annex |\d+(\.\d+)+\s+[A-Z]"):
        start = re.search(rf"^\s*Table {re.escape(name)}.*$", self.text, re.M)
        if start is None:
            raise ValueError(f"missing Table {name}")
        lines = []
        for line in self.text[start.end() :].splitlines():
            if re.match(rf"\s*({stop})", line) and not re.match(r"\s*Table \d+[a-z]? \(see", line):
                break
            lines.append(line)
        return lines

    def rows(self, name):
        return [re.split(r"\s{2,}", line.strip()) for line in self.table(name) if line.strip()]

    def page(self, caption):
        return next(i for i, text in enumerate(self.pages) if caption in text)

    def words(self, caption):
        return self.page_words(self.page(caption))

    def page_words(self, index):
        page = str(index + 1)
        output = subprocess.check_output(["pdftotext", "-f", page, "-l", page, "-bbox", str(self.pdf), "-"], text=True)
        return [(float(a), float(b), float(c), float(d), w) for a, b, c, d, w in WORD.findall(output)]


def single(value):
    return float(f"{struct.unpack('<f', struct.pack('<f', value))[0]:.9g}")


def number(text):
    return float(text.replace(",", ".").replace(" ", ""))


def fraction(text):
    text = text.replace(",", ".").replace(" ", "")
    if "+" in text[1:]:
        left, right = text.rsplit("+", 1)
        return fraction(left) + fraction(right)
    if "-" in text[1:]:
        left, right = text.rsplit("-", 1)
        return fraction(left) - fraction(right)
    return Fraction(text)


def ldpc_tables(standard, annexes="BC"):
    header = re.compile(
        rf"^\s*Table ([{annexes}]\.\d+): (?:Rate|LDPC code identifier:) (\d+/\d+) \([nN]\s*ldpc\s*= (\d+ \d+)\)", re.M
    )
    tables = {}
    for match in header.finditer(standard.text):
        section = standard.text[match.end() :].split("\n", 1)[1]
        end = re.search(rf"^\s*(Table [{annexes}]\.|Annex )", section, re.M)
        cells = [cell for page in section[: end.start() if end else None].split("\f") for cell in page_columns(page)]
        if cells:
            widths = {}
            for column, span, _ in cells:
                widths[column] = max(widths.get(column, 0), span)
            lines = [(values, span >= widths[column] - 7) for column, span, values in cells]
            key = (match.group(2).replace("/", "_"), SIZES[match.group(3)])
            if key in tables:
                raise ValueError(f"duplicate LDPC table {key}")
            tables[key] = LdpcTable(match.group(1), lines)
    return tables


def page_columns(page):
    cells = []
    for line in page.splitlines():
        if NUMBERS.fullmatch(line):
            for cell in re.finditer(r"\d+(?: \d+)*", line):
                cells.append((cell.start(), cell.end(), [int(n) for n in cell.group().split()]))
    starts = []
    for start in sorted({start for start, _, _ in cells}):
        if not starts or start - starts[-1][-1] > 5:
            starts.append([start])
        else:
            starts[-1].append(start)
    columns = [[cell for cell in cells if column[0] <= cell[0] <= column[-1]] for column in starts]
    return [(index, end - start, values) for index, column in enumerate(columns) for start, end, values in column]


def ldpc_rows(table):
    rows = []
    previous_full = False
    for values, full in table.lines:
        last = rows[-1] if rows else None
        if previous_full and last == sorted(last) and values == sorted(values) and values[0] > last[-1]:
            last.extend(values)
        else:
            rows.append(list(values))
        previous_full = full
    return rows


def ldpc_breaks(table):
    breaks = set()
    position = 0
    for values, full in table.lines:
        position += len(values)
        if not full:
            breaks.add(position)
    return breaks


def figure_labels(standard, caption, bits):
    words = [w for w in standard.words(caption) if re.fullmatch(rf"[01]{{{bits}}}", w[4])]
    if len(words) != 1 << bits:
        raise ValueError(f"{caption}: {len(words)} labels")
    centers = {w[4]: ((w[0] + w[2]) / 2, (w[1] + w[3]) / 2) for w in words}
    x0 = sum(x for x, _ in centers.values()) / len(centers)
    y0 = sum(y for _, y in centers.values()) / len(centers)
    return {label: (math.hypot(x - x0, y - y0), math.atan2(y0 - y, x - x0)) for label, (x, y) in centers.items()}


def snap(angle, count, offset):
    step = 2 * math.pi / count
    return Fraction(round((angle - offset) / step) % count * 2, count) + Fraction(offset / math.pi).limit_denominator(
        24
    )


def figure_rings(standard, caption, bits, rings):
    labels = figure_labels(standard, caption, bits)
    ordered = sorted(labels, key=lambda label: labels[label][0])
    result = {}
    start = 0
    for ring, (count, offset) in enumerate(rings):
        for label in ordered[start : start + count]:
            result[int(label, 2)] = (ring, snap(labels[label][1], count, offset))
        start += count
    if len(result) != 1 << bits:
        raise ValueError(f"{caption}: ring sizes")
    return [result[i] for i in range(1 << bits)]


def s2_rings(s2):
    return {
        "QPSK": figure_rings(s2, "Figure 9: Bit mapping into QPSK", 2, [(4, math.pi / 4)]),
        "8PSK": figure_rings(s2, "Figure 10: Bit mapping into 8PSK", 3, [(8, 0)]),
        "16APSK": figure_rings(s2, "Figure 11: 16APSK", 4, [(4, math.pi / 4), (12, math.pi / 12)]),
        "32APSK": figure_rings(s2, "Figure 12: 32APSK", 5, [(4, math.pi / 4), (12, math.pi / 12), (16, 0)]),
    }


def ratios(standard, name):
    result = {}
    for line in standard.table(name):
        match = re.fullmatch(r"\s*(\d+/\d+)\s+\d+,\d+((?:\s+\d+(?:,\d+)?)+)\s*", line)
        if match:
            result[match.group(1)] = [number(v) for v in match.group(2).split()]
    if not result:
        raise ValueError(f"no ratios in Table {name}")
    return result


def s2_interleaver(s2):
    columns = {}
    for row in s2.rows("8:"):
        if len(row) == 4 and row[0].endswith("PSK"):
            columns[row[0]] = int(row[3])
    if "MSB of BBHEADER is read out third" not in re.sub(r"\s+", " ", s2.text):
        raise ValueError("8PSK 3/5 read-out order not found")
    return columns


def label_rows(standard, name):
    result = []
    for row in standard.rows(name):
        if re.fullmatch(r"[01pqr]+", row[0]) and re.fullmatch(r"R\d", row[1]):
            result.append((row[0], int(row[1][1:]), [fraction(v) for v in row[2:]]))
    return result


def expand(template, columns):
    points = {}
    for column, angle in enumerate(columns):
        values = {"p": str(column >> 1), "q": str(column & 1)} if len(columns) == 4 else {"p": str(column)}
        label = "".join(values.get(c, c) for c in template)
        points[int(label, 2)] = angle
    return points


def ring_points(standard, name, count):
    points = {}
    for template, ring, columns in label_rows(standard, name):
        for index, angle in expand(template, columns).items():
            points[index] = (ring, angle)
    if len(points) != count:
        raise ValueError(f"Table {name}: {len(points)} points")
    return [points[i] for i in range(count)]


def complex_points(standard, name, column):
    points = {}
    for line in standard.table(name, stop=r"Table |Figure |Annex |\d+(\.\d+)+\s+[A-Z]|NOTE"):
        match = re.fullmatch(r"\s*([01]{4,8})((?:\s+-?\d+,\d+\s*[+-]\s*\d+,\d+i){2})\s*", line)
        if match:
            pairs = re.findall(r"(-?\d+,\d+)\s*([+-])\s*(\d+,\d+)i", match.group(2))
            real, sign, imaginary = pairs[column]
            points[int(match.group(1), 2)] = (number(real), number(sign + imaginary))
    return [points[i] for i in range(len(points))]


def apsk256_points(s2x):
    radii = {}
    for row in s2x.rows("15b:"):
        if len(row) == 2 and re.fullmatch(r"[01]{3}qpaaa", row[0]):
            radii[row[0][:3]] = int(row[1][1:])
    angles = {}
    for row in s2x.rows("15c:"):
        match = re.fullmatch(r"rrrqp([01]{3})", row[0])
        if match:
            base = fraction(re.search(r"=\s*(\d+)π/(\d+)", row[1]).expand(r"\1/\2"))
            angles[match.group(1)] = [base, 2 - base, 1 - base, 1 + base]
    points = []
    for index in range(256):
        label = f"{index:08b}"
        points.append((radii[label[:3]], angles[label[5:]][int(label[3]) + 2 * int(label[4])]))
    return points


def unit_energy(points, gammas):
    radii = [1.0] + list(gammas)
    scale = math.sqrt(len(points) / sum(radii[ring - 1] ** 2 for ring, _ in points))
    return [
        (
            scale * radii[ring - 1] * math.cos(float(angle) * math.pi),
            scale * radii[ring - 1] * math.sin(float(angle) * math.pi),
        )
        for ring, angle in points
    ]


def s2x_modes(s2, s2x):
    rings = s2_rings(s2)
    patterns = interleaver_patterns(s2x)
    modes = []
    for row in s2x.rows("17a:"):
        if len(row) == 4 and re.fullmatch(r"\d{3}", row[0]):
            code, _, name, kind = row
            family, rate = name.split()
            short = kind == "Short"
            points = s2x_points(s2x, rings, family, rate, short)
            bits = len(points).bit_length() - 1
            symbols = -(-LENGTHS["S" if short else "N"] // bits)
            modes.append(
                {
                    "code": int(code),
                    "short": short,
                    "rate": rate.replace("/", "_"),
                    "family": family,
                    "bits": bits,
                    "slots": -(-symbols // 90),
                    "order": patterns.get((family, rate, short), []),
                    "points": points,
                }
            )
    return modes


def interleaver_patterns(s2x):
    patterns = {}
    for table, short in (("9a:", False), ("9b:", True)):
        for row in s2x.rows(table):
            if len(row) == 2 and re.fullmatch(r"\d+", row[1]):
                family, rate = row[0].replace(",", "").replace(" APSK ", " ").split()
                patterns[(family, rate, short)] = [int(c) for c in row[1]]
    return patterns


def s2x_points(s2x, rings, family, rate, short):
    if family in ("QPSK", "8PSK"):
        return unit_energy([(1, angle) for _, angle in rings[family]], [])
    if family == "2+4+2APSK":
        return unit_energy(ring_points(s2x, "10a:", 8), ratios(s2x, "10b:")[rate])
    if family == "4+12APSK":
        gamma = ratios(s2x, "11b:" if short else "11a:")[rate]
        return unit_energy([(ring + 1, angle) for ring, angle in rings["16APSK"]], gamma)
    if family == "8+8APSK" and rate in ("18/30", "20/30"):
        return complex_points(s2x, "11e:", ["18/30", "20/30"].index(rate))
    if family == "8+8APSK":
        return unit_energy(ring_points(s2x, "11c:", 16), ratios(s2x, "11d:")[rate])
    if family == "4+12+16rbAPSK":
        return unit_energy(ring_points(s2x, "12c:", 32), ratios(s2x, "12b:" if short else "12a:")[rate])
    if family == "4+8+4+16APSK":
        return unit_energy(ring_points(s2x, "12d:", 32), ratios(s2x, "12e:")[rate])
    if family == "16+16+16+16APSK":
        return unit_energy(ring_points(s2x, "13a:", 64), ratios(s2x, "13b:")[rate])
    if family == "8+16+20+20APSK":
        return unit_energy(ring_points(s2x, "13c:", 64), ratios(s2x, "13d:")[rate])
    if family == "4+12+20+28APSK":
        return unit_energy(ring_points(s2x, "13e:", 64), ratios(s2x, "13f:")[rate])
    if family == "128APSK":
        return unit_energy(ring_points(s2x, "14b:", 128), ratios(s2x, "14a:")[rate])
    if family == "256APSK" and rate in ("20/30", "22/30"):
        return complex_points(s2x, "15d:", ["20/30", "22/30"].index(rate))
    if family == "256APSK":
        return unit_energy(apsk256_points(s2x), ratios(s2x, "15a:")[rate])
    raise ValueError(f"unknown constellation {family} {rate}")
