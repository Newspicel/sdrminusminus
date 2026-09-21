import argparse
import cmath
import math
import re
import struct
from pathlib import Path

NORMAL_BCH = [
    [0, 2, 3, 5, 16],
    [0, 1, 4, 5, 6, 8, 16],
    [0, 2, 3, 4, 5, 7, 8, 9, 10, 11, 16],
    [0, 2, 4, 6, 9, 11, 12, 14, 16],
    [0, 1, 2, 3, 5, 8, 9, 10, 11, 12, 16],
    [0, 2, 4, 5, 7, 8, 9, 10, 12, 13, 14, 15, 16],
    [0, 2, 5, 6, 8, 9, 10, 11, 13, 15, 16],
    [0, 1, 2, 5, 6, 8, 9, 12, 13, 14, 16],
    [0, 5, 7, 9, 10, 11, 16],
    [0, 1, 2, 5, 7, 8, 10, 12, 13, 14, 16],
    [0, 2, 3, 5, 9, 11, 12, 13, 16],
    [0, 1, 5, 6, 7, 9, 11, 12, 16],
]
SHORT_BCH = [
    [0, 1, 3, 5, 14],
    [0, 6, 8, 11, 14],
    [0, 1, 2, 6, 9, 10, 14],
    [0, 4, 7, 8, 10, 12, 14],
    [0, 2, 4, 6, 8, 9, 11, 13, 14],
    [0, 3, 7, 8, 9, 13, 14],
    [0, 2, 5, 6, 7, 10, 11, 13, 14],
    [0, 5, 8, 9, 10, 11, 14],
    [0, 1, 2, 3, 9, 10, 14],
    [0, 3, 6, 9, 11, 12, 14],
    [0, 4, 11, 12, 14],
    [0, 1, 2, 3, 5, 6, 7, 8, 10, 13, 14],
]


def addresses(document, table):
    start = document.index(f"Table {table}:")
    section = document[start:].splitlines()[1:]
    columns = [[], [], []]
    for line in section:
        if re.search(r"Table [AB]\.\d+:|Annex [BC] \(normative\)", line):
            break
        if re.fullmatch(r"\s*\d+(?:\s+\d+)+\s*", line):
            for col, value in enumerate(re.split(r" {2,}", line.strip())):
                columns[col].append([int(n) for n in value.split()])
    return [row for column in columns for row in column]


def polynomial_product(left, right):
    result = 0
    while right:
        if right & 1:
            result ^= left
        left <<= 1
        right >>= 1
    return result


def encode(message, length, rows, correct, bch_errors=()):
    generator = 1
    factors = SHORT_BCH if length == 16200 else NORMAL_BCH
    for factor in factors[:correct]:
        generator = polynomial_product(generator, sum(1 << i for i in factor))
    parity_length = generator.bit_length() - 1
    scrambled = []
    state = 0x4A80
    for bit in message:
        prbs = (state ^ (state >> 1)) & 1
        state = (state >> 1) | (prbs << 14)
        scrambled.append(bit ^ prbs)
    remainder = 0
    for bit in scrambled:
        feedback = bit ^ (remainder >> (parity_length - 1))
        remainder <<= 1
        if feedback:
            remainder ^= generator
        remainder &= (1 << parity_length) - 1
    information = scrambled + [
        (remainder >> i) & 1 for i in reversed(range(parity_length))
    ]
    assert len(information) == 360 * len(rows)
    for index in bch_errors:
        information[index] ^= 1
    parity = [0] * (length - len(information))
    q = len(parity) // 360
    for i, bit in enumerate(information):
        if bit:
            for address in rows[i // 360]:
                parity[(address + (i % 360) * q) % len(parity)] ^= 1
    for i in range(1, len(parity)):
        parity[i] ^= parity[i - 1]
    return information, parity


def mapped_cells(information, parity, bits, twist, mux, rotation):
    q = len(parity) // 360
    ordered = information + [parity[q * s + t] for t in range(q) for s in range(360)]
    columns = len(twist)
    rows = len(ordered) // columns
    matrix = [[0] * columns for _ in range(rows)]
    for i, bit in enumerate(ordered):
        c = i // rows
        matrix[(i % rows + twist[c]) % rows][mux[c]] = bit
    serial = [bit for row in matrix for bit in row]
    cells = []
    for i in range(0, len(serial), bits):
        axes = []
        for axis in range(2):
            gray = serial[i + axis : i + bits : 2]
            magnitude = 1
            for position in reversed(range(1, len(gray))):
                magnitude = (
                    2 ** (len(gray) - position) + (1 - 2 * gray[position]) * magnitude
                )
            axes.append((1 - 2 * gray[0]) * magnitude)
        cells.append(
            complex(*axes) / math.sqrt(2 * (2**bits - 1) / 3) * cmath.exp(1j * rotation)
        )
    if rotation:
        cells = [
            complex(value.real, cells[i - 1].imag) for i, value in enumerate(cells)
        ]
    return cells


def cell_interleave(cells, block):
    degree = (len(cells) - 1).bit_length()
    taps = {
        11: [0, 3],
        12: [0, 2],
        13: [0, 1, 4, 6],
        14: [0, 1, 4, 5, 9, 11],
        15: [0, 1, 2, 12],
    }[degree]
    state = 0
    permutation = []
    for i in range(2**degree):
        if i < 2:
            state = 0
        elif i == 2:
            state = 1
        else:
            feedback = sum((state >> tap) & 1 for tap in taps) % 2
            state = (state >> 1) | (feedback << (degree - 2))
        address = state | ((i % 2) << (degree - 1))
        if address < len(cells):
            permutation.append(address)
    shifts = [int(f"{i:0{degree}b}"[::-1], 2) for i in range(2**degree)]
    shift = [v for v in shifts if v < len(cells)][block]
    output = [0j] * len(cells)
    for i, address in enumerate(permutation):
        output[(address + shift) % len(cells)] = cells[i]
    return output


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("standard_text", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    document = args.standard_text.read_text()
    args.output.mkdir(parents=True, exist_ok=True)
    cases = [
        (
            "normal_2_3_qam256",
            "A.3",
            64800,
            10,
            8,
            [0, 2, 2, 2, 2, 3, 7, 15, 16, 20, 22, 22, 27, 27, 28, 32],
            [7, 2, 9, 0, 4, 6, 13, 3, 14, 10, 15, 5, 8, 12, 11, 1],
            math.atan(1 / 16),
        ),
        (
            "normal_3_5_qam64",
            "A.2",
            64800,
            12,
            6,
            [0, 0, 2, 2, 3, 4, 4, 5, 5, 7, 8, 9],
            [2, 7, 6, 9, 0, 3, 1, 8, 4, 11, 5, 10],
            0,
        ),
        (
            "short_3_5_qam64",
            "B.3",
            16200,
            12,
            6,
            [0, 0, 0, 2, 2, 2, 3, 3, 3, 6, 7, 7],
            [11, 7, 3, 10, 6, 2, 9, 5, 1, 8, 4, 0],
            math.radians(8.6),
        ),
        (
            "lite_1_3_qam16",
            "B.8",
            16200,
            12,
            4,
            [0, 0, 0, 1, 7, 20, 20, 21],
            [6, 0, 3, 4, 5, 2, 1, 7],
            math.radians(16.8),
        ),
    ]
    for name, table, length, correct, bits, twist, mux, rotation in cases:
        rows = addresses(document, table)
        message_length = 360 * len(rows) - correct * (14 if length == 16200 else 16)
        message = [int((i * 173 + i // 7) % 31 < 15) for i in range(message_length)]
        information, parity = encode(message, length, rows, correct)
        cells = cell_interleave(
            mapped_cells(information, parity, bits, twist, mux, rotation), 3
        )
        args.output.joinpath(name + ".f32").write_bytes(
            b"".join(struct.pack("<ff", p.real, p.imag) for p in cells)
        )
        print(name, len(cells), message_length)
        if name == "short_3_5_qam64":
            information, parity = encode(message, length, rows, correct, (123, 3456))
            cells = cell_interleave(
                mapped_cells(information, parity, bits, twist, mux, rotation), 3
            )
            args.output.joinpath("short_3_5_bch_errors.f32").write_bytes(
                b"".join(struct.pack("<ff", p.real, p.imag) for p in cells)
            )


if __name__ == "__main__":
    main()
