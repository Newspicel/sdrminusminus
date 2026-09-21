import argparse
import math
import re
from pathlib import Path

import numpy as np
from dvbt2_reference import addresses, cell_interleave, encode, mapped_cells

PRE_SHORTEN = [7, 3, 6, 5, 2, 4, 1, 8, 0]
PRE_PUNCTURE = [
    27,
    13,
    29,
    32,
    5,
    0,
    11,
    21,
    33,
    20,
    25,
    28,
    18,
    35,
    8,
    3,
    9,
    31,
    22,
    24,
    7,
    14,
    17,
    4,
    2,
    26,
    16,
    34,
    19,
    10,
    12,
    23,
    1,
    6,
    30,
    15,
]
POST_SHORTEN = [18, 17, 16, 15, 14, 13, 12, 11, 4, 10, 9, 8, 3, 2, 7, 6, 5, 1, 19, 0]
POST_PUNCTURE = [
    6,
    4,
    18,
    9,
    13,
    8,
    15,
    20,
    5,
    17,
    2,
    24,
    10,
    22,
    12,
    3,
    16,
    23,
    1,
    14,
    0,
    21,
    19,
    7,
    11,
]
CONTINUAL = [
    116,
    255,
    285,
    430,
    518,
    546,
    601,
    646,
    744,
    1662,
    1893,
    1995,
    2322,
    3309,
    3351,
    3567,
    3813,
    4032,
    5568,
    5706,
    1022,
    1224,
    1302,
    1371,
    1495,
    2261,
    2551,
    2583,
    2649,
    2833,
    2925,
    3192,
    4266,
    5395,
    5710,
    5881,
    8164,
    10568,
    11069,
    11560,
    12631,
    12946,
    13954,
    16745,
    21494,
]
PERMUTATIONS = [[4, 3, 9, 6, 2, 8, 1, 5, 7, 0], [6, 9, 4, 8, 5, 1, 0, 7, 2, 3]]


PERMUTATIONS_8K = [
    [7, 1, 4, 2, 9, 6, 8, 10, 0, 3, 11, 5],
    [11, 4, 9, 3, 1, 2, 5, 0, 6, 7, 10, 8],
]


def bits(value, length):
    return [value >> shift & 1 for shift in reversed(range(length))]


def fields(values):
    return [bit for value, length in values for bit in bits(value, length)]


def checksum(word, width=32, polynomial=0x04C11DB7, initial=0xFFFFFFFF):
    state = initial
    for bit in word:
        feedback = bit ^ (state >> (width - 1))
        state = (state << 1) & ((1 << width) - 1)
        if feedback:
            state ^= polynomial
    return state


def append_crc(word):
    return word + bits(checksum(word), 32)


def l1_encode(word, pre, document, length):
    rows = addresses(document, "B.1" if pre else "B.2")
    size = len(rows) * 360 - 168
    shorten = PRE_SHORTEN if pre else POST_SHORTEN
    puncture = PRE_PUNCTURE if pre else POST_PUNCTURE
    omitted = set()
    missing = size - len(word)
    for group in shorten:
        positions = list(range(group * 360, min(size, (group + 1) * 360)))
        take = min(len(positions), missing)
        omitted.update(positions[len(positions) - take :])
        missing -= take
        if missing == 0:
            break
    padded = []
    source = iter(word)
    for index in range(size):
        padded.append(0 if index in omitted else next(source))
    information, parity = encode(padded, 16200, rows, 12, scramble=False)
    remove = len(word) + 16200 - size - length
    for index in range(remove):
        omitted.add(size + 168 + puncture[index // 360] + len(puncture) * (index % 360))
    return [
        complex(1 - 2 * bit)
        for index, bit in enumerate(information + parity)
        if index not in omitted
    ]


def l1(document, frame, blocks, fft=2048, miso=False, lite=False, media=False):
    p2_count = max(1, 16384 // fft)
    s2 = {2048: 0, 8192: 12, 32768: 14}[fft]
    guard = 3 if fft == 2048 else 4
    pattern = 0 if fft == 2048 else 7
    symbols = {2048: 16, 8192: 4, 32768: 2}[fft]
    post = fields([(1, 15), (1, 8), (0, 4), (0, 8), (0, 3), (474000000, 32)])
    post += fields(
        [
            (7, 8),
            (1, 3),
            (3, 5),
            (0, 1),
            (0, 3),
            (0, 8),
            (0, 8),
            (2 if media else 6 if lite else 0, 3),
            (3 if media else 1 if lite else 0, 3),
            (int(lite or media), 1),
            (int(media), 2),
            (blocks, 10),
            (1, 8),
            (1, 8),
            (0, 1),
            (0, 1),
            (0, 1),
            (0, 11),
            (2, 2),
            (0, 1),
            (0, 1),
        ]
    )
    post += fields(
        [
            (0, 32),
            (frame, 8),
            (0, 22),
            (0, 22),
            (0, 8),
            (0, 3),
            (0, 8),
            (7, 8),
            (20000 if fft == 32768 else 0, 22),
            (blocks, 10),
            (0, 8),
            (0, 8),
        ]
    )
    info = len(post)
    post = append_crc(post)
    multiple = max(2, p2_count)
    puncture = max(multiple - 1, (7032 - len(post)) * 6 // 5)
    length = math.ceil((len(post) + 9168 - puncture) / multiple) * multiple
    pre = fields(
        [
            (0, 8),
            (0, 1),
            (3 if lite else int(miso), 3),
            (s2, 4),
            (0, 1),
            (guard, 3),
            (0, 4),
            (0, 4),
            (0, 2),
            (0, 2),
            (length, 18),
            (info, 18),
            (pattern, 4),
            (0, 8),
            (1, 16),
            (2, 16),
            (3, 16),
            (2, 8),
            (symbols, 12),
            (0, 3),
            (0, 1),
            (1, 3),
            (0, 3),
            (2, 4),
            (0, 1),
            (0, 1),
            (0, 4),
        ]
    )
    assert len(pre) == 168
    return l1_encode(append_crc(pre), True, document, 1840), l1_encode(
        post, False, document, length
    )


def p1(document, s1=0, s2=0):
    section = document.split("Table 68: Distribution", 1)[1].split(
        "9.8.2.2           Modulation", 1
    )[0]
    carriers = []
    for line in section.splitlines():
        if len(line) > 32:
            values = line[32:].strip()
            if re.fullmatch(r"[0-9 ]+", values):
                carriers.extend(int(value) for value in values.split())
    assert len(carriers) == 384
    table = document.split("Table 69: S1 and S2 Modulation patterns", 1)[1].split(
        "The bit sequences", 1
    )[0]
    patterns = dict(
        re.findall(
            r"^\s*(?:S[12]\s+)?([01]{3,4})\s+([0-9A-F]+)\s*$", table, re.MULTILINE
        )
    )
    first = bytes.fromhex(patterns[f"{s1:03b}"])
    second = bytes.fromhex(patterns[f"{s2:04b}"])
    word = fields([(byte, 8) for byte in first + second + first])
    state = 0x4E46
    differential = 1
    spectrum = np.zeros(1024, dtype=np.complex128)
    for carrier, bit in zip(carriers, word, strict=True):
        differential *= 1 - 2 * bit
        prbs = (state ^ (state >> 1)) & 1
        state = (state >> 1) | (prbs << 14)
        spectrum[(carrier - 426) % 1024] = differential * (1 - 2 * prbs)
    symbol = np.fft.ifft(spectrum) * 1024 / math.sqrt(384)
    c = symbol[:542] * np.exp(2j * np.pi * np.arange(542) / 1024)
    b = symbol[542:] * np.exp(2j * np.pi * np.arange(1566, 2048) / 1024)
    return np.concatenate([c, symbol, b])


def media_transport(frame):
    def section(data):
        return data + checksum(fields([(byte, 8) for byte in data])).to_bytes(4, "big")

    pat = section(bytes.fromhex("00 b0 0d 00 01 c1 00 00 00 01 e1 00"))
    pmt = section(
        bytes.fromhex(
            "02 b0 17 00 01 c1 00 00 e1 01 f0 00 1b e1 01 f0 00 03 e1 02 f0 00"
        )
    )
    fixture = Path(__file__).resolve().parents[3] / "fixtures" / "broadcast_audio"
    streams = [(0, bytes([0]) + pat), (0x100, bytes([0]) + pmt)]
    for pid, stream, name in [
        (0x101, 0xE0, "pattern.h264"),
        (0x102, 0xC0, "tone_48k_mono.mp2"),
    ]:
        data = (fixture / name).read_bytes()
        pts = 90000 + frame * 90000
        stamp = bytes(
            [
                0x21 | ((pts >> 29) & 0x0E),
                (pts >> 22) & 255,
                1 | ((pts >> 14) & 0xFE),
                (pts >> 7) & 255,
                1 | ((pts << 1) & 0xFE),
            ]
        )
        pes = (
            bytes([0, 0, 1, stream])
            + (len(data) + 8).to_bytes(2, "big")
            + bytes([0x80, 0x80, 5])
            + stamp
            + data
        )
        streams.append((pid, pes))
    packets = []
    for pid, data in streams:
        count = math.ceil(len(data) / 184)
        for index, start in enumerate(range(0, len(data), 184)):
            chunk = data[start : start + 184]
            control = 0x10 if len(chunk) == 184 else 0x30
            header = bytes(
                [
                    0x47,
                    (pid >> 8) | (0x40 if index == 0 else 0),
                    pid & 255,
                    control | ((frame * count + index) % 16),
                ]
            )
            stuffing = b""
            if len(chunk) < 184:
                length = 183 - len(chunk)
                stuffing = bytes([length]) + (
                    bytes([0]) + bytes([255]) * (length - 1) if length else b""
                )
            packets.append(header + stuffing + chunk)
    assert len(packets) == 72
    return packets


def payload(document, frame, blocks, lite=False, media=False):
    encoded = []
    packets = []
    media_packets = media_transport(frame) if media else []
    for block in range(blocks):
        packet = bytes([0x47, 0x01, 0x23, 0x10 | (frame * blocks + block)]) + bytes(
            (i * 17 + block + frame) % 256 for i in range(184)
        )
        selected = media_packets[block * 24 : (block + 1) * 24] if media else [packet]
        packets.extend(selected)
        data = b"".join(p[1:] for p in selected)
        header = bytes([0xF0, 0, 0, 0]) + (len(data) * 8).to_bytes(2, "big") + bytes(3)
        hcrc = checksum(fields([(byte, 8) for byte in header]), 8, 0xD5, 0) ^ 1
        word = fields([(byte, 8) for byte in header + bytes([hcrc]) + data])
        word += [0] * ((43040 if media else 5232 if lite else 7032) - len(word))
        information, parity = encode(
            word,
            64800 if media else 16200,
            addresses(document, "A.3" if media else "B.8" if lite else "B.2"),
            10 if media else 12,
        )
        word = information + parity
        qpsk = [
            complex(1 - 2 * word[i], 1 - 2 * word[i + 1]) / math.sqrt(2)
            for i in range(0, len(word), 2)
        ]
        if lite:
            qpsk = mapped_cells(
                information,
                parity,
                4,
                [0, 0, 0, 1, 7, 20, 20, 21],
                [6, 0, 3, 4, 5, 2, 1, 7],
                math.radians(16.8),
            )
        if media:
            qpsk = mapped_cells(
                information,
                parity,
                8,
                [0, 2, 2, 2, 2, 3, 7, 15, 16, 20, 22, 22, 27, 27, 28, 32],
                [7, 2, 9, 0, 4, 6, 13, 3, 14, 10, 15, 5, 8, 12, 11, 1],
                math.atan(1 / 16),
            )
        cells = cell_interleave(qpsk, block)
        encoded.extend(cells)
    rows = (4050 if lite else 8100) // 5
    columns = 5 * blocks
    return [
        encoded[column * rows + row] for row in range(rows) for column in range(columns)
    ], b"".join(packets)


def permutation(count, parity, fft=2048):
    degree = int(math.log2(fft)) - 1
    taps = {2048: [0, 3], 8192: [0, 1, 4, 6], 32768: [0, 1, 2, 12]}[fft]
    order = {
        2048: PERMUTATIONS,
        8192: PERMUTATIONS_8K,
        32768: [[7, 13, 3, 4, 9, 2, 12, 11, 1, 8, 10, 0, 5, 6]] * 2,
    }[fft]
    result = []
    state = 0
    for i in range(fft):
        if i < 2:
            state = 0
        elif i == 2:
            state = 1
        else:
            state = (state >> 1) | (
                (sum(state >> t & 1 for t in taps) % 2) << (degree - 1)
            )
        address = (i % 2) * (fft // 2) + sum(
            ((state >> bit) & 1) << position
            for bit, position in enumerate(order[parity])
        )
        if address < count:
            result.append(address)
    return result


def p2_tones(document, fft):
    table = document.split("Table H.1: Reserved carrier indices for P2 symbol", 1)[
        1
    ].split("Table H.2:", 1)[0]
    values = []
    for line in table.splitlines():
        text = line[14:].strip()
        if text and re.fullmatch(r"[0-9, ]+", text):
            values.extend(int(n) for n in re.findall(r"\d+", text))
    counts = [10, 18, 36, 72, 144, 288]
    assert len(values) == sum(counts)
    mode = int(math.log2(fft)) - 10
    start = sum(counts[:mode])
    return values[start : start + counts[mode]]


def pp8_continual(document, fft):
    table = document.split("Table G.1:", 1)[1].split("Table G.2:", 1)[0]
    group = 0
    start = 0
    values = []
    for line in table.splitlines():
        if "PP8" in line:
            start = (line.index("PP7") + line.index("PP8")) // 2
        match = re.match(r"CP([1-6])", line)
        if match:
            group = int(match[1])
        if group == 0 or group > int(math.log2(fft)) - 9:
            continue
        text = line[start:].strip()
        if text and re.fullmatch(r"[0-9 ]+", text):
            values.extend(int(n) for n in text.split())
    assert len(values) == (47 if fft == 8192 else 175)
    return values


def waveform(document, frame, fft=2048, miso=False, lite=False, media=False):
    p2_count = max(1, 16384 // fft)
    carriers = {2048: 1705, 8192: 6817, 32768: 27265}[fft]
    guard = fft // 4 if fft == 2048 else fft // 128
    symbols = {2048: 16, 8192: 4, 32768: 2}[fft]
    tones = p2_tones(document, fft)
    continual = CONTINUAL if fft == 2048 else pp8_continual(document, fft)
    modulus = {2048: 1632, 8192: 6528, 32768: 32768}[fft]
    spacing = 3 if fft == 2048 else 6
    phases = 4 if fft == 2048 else 16
    blocks = 3 if media else 2
    pre, post = l1(document, frame, blocks, fft, miso, lite, media)
    payload_cells, packets = payload(document, frame, blocks, lite, media)
    source = iter(([1 + 0j] * 20000 if fft == 32768 else []) + payload_cells)
    pn = bits(0x4DC2AF7B, 32)
    state = 0x7FF
    prbs = []
    offset = {2048: 0, 8192: 48, 32768: 288}[fft]
    for _ in range(carriers + offset):
        prbs.append(state & 1)
        state = (state >> 1) | (((state ^ (state >> 2)) & 1) << 10)
    prbs = prbs[offset:]
    h1 = 0.7 + 0.2j if miso else 1 + 0j
    h2 = -0.1 + 0.3j if miso else 0j
    p2_amplitude = math.sqrt(37 if fft == 32768 and not miso else 31) / 5
    output = [
        p1(document, 3 if lite else int(miso), {2048: 0, 8192: 12, 32768: 14}[fft])
        * (h1 + h2)
    ]
    for symbol in range(p2_count + symbols):
        is_p2 = symbol < p2_count
        closing = fft == 2048 and symbol == p2_count + symbols - 1
        scattered_amp = 4 / 3 if fft == 2048 else 7 / 3
        if is_p2:
            pilots = dict.fromkeys(
                range(0, carriers, 6 if fft == 32768 and not miso else 3), p2_amplitude
            )
            if miso:
                pilots.update(
                    dict.fromkeys([1, 2, carriers - 2, carriers - 3], p2_amplitude)
                )
                for tone in tones:
                    adjacent = tone + 1 if tone % 3 == 1 else tone - 1
                    if adjacent not in tones:
                        pilots[adjacent] = p2_amplitude
        elif closing:
            pilots = dict.fromkeys(range(0, carriers, spacing), scattered_amp)
        else:
            pilots = dict.fromkeys(
                (k % modulus for k in continual), 4 / 3 if fft == 2048 else 8 / 3
            )
            pilots.update(
                dict.fromkeys(
                    range(spacing * (symbol % phases), carriers, spacing * phases),
                    scattered_amp,
                )
            )
        pilots[0] = pilots[carriers - 1] = p2_amplitude if is_p2 else scattered_amp
        reserved = set(tones) if is_p2 else set()
        available = [
            k for k in range(carriers) if k not in pilots and k not in reserved
        ]
        data = pre[symbol::p2_count] + post[symbol::p2_count] if is_p2 else []
        active = 804 if closing else len(available)
        data.extend(next(source, 1 + 0j) for _ in range(active - len(data)))
        data.extend(0j for _ in range(len(available) - active))
        addresses = permutation(len(available), symbol % 2, fft)
        if fft == 32768 and symbol % 2 == 0:
            values = [0j] * len(data)
            for i, address in enumerate(addresses):
                values[address] = data[i]
        else:
            values = [data[address] for address in addresses]
        spectrum = np.zeros(fft, dtype=np.complex128)
        for i, k in enumerate(available):
            second = (
                -values[i + 1].conjugate() if i % 2 == 0 else values[i - 1].conjugate()
            )
            spectrum[(k - carriers // 2) % fft] = h1 * values[i] + h2 * second
        for k, amplitude in pilots.items():
            if is_p2:
                inverted = k % 3 == 0 and k // 3 % 2 == 1
            elif k == 0 or k == carriers - 1:
                inverted = symbol % 2 == 1
            else:
                inverted = k % spacing == 0 and k // spacing % 2 == 1
            spectrum[(k - carriers // 2) % fft] = (
                amplitude
                * (1 - 2 * (prbs[k] ^ pn[symbol]))
                * (h1 + (-h2 if inverted else h2))
            )
        time = np.fft.ifft(spectrum) * fft / math.sqrt(carriers)
        output.append(np.concatenate([time[-guard:], time]))
    assert next(source, None) is None
    return np.concatenate(output), packets


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("standard_text", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    document = args.standard_text.read_text()
    args.output.mkdir(parents=True, exist_ok=True)
    for fft, miso, lite, media in [
        (2048, False, False, False),
        (8192, False, False, False),
        (8192, True, False, False),
        (32768, False, False, False),
        (2048, False, True, False),
        (32768, False, False, True),
    ]:
        frames = [
            waveform(document, frame, fft, miso, lite, media) for frame in range(2)
        ]
        samples = np.concatenate(
            [np.zeros(73), *(frame[0] for frame in frames), np.zeros(8192)]
        )
        name = f"rf_{fft // 1024}k_" + (
            "media" if media else "lite" if lite else "miso" if miso else "qpsk"
        )
        samples.astype("<c8").tofile(args.output / (name + ".f32"))
        (args.output / (name + ".ts")).write_bytes(
            b"".join(frame[1] for frame in frames)
        )
        print(name, len(samples), "samples")


if __name__ == "__main__":
    main()
