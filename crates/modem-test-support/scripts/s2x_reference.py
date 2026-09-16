import argparse
import cmath
import hashlib
import json
import math
import random
import re
import struct
from pathlib import Path


def crc8(data):
    crc = 0
    for byte in data:
        crc ^= byte
        for _ in range(8):
            crc = ((crc << 1) ^ (0xd5 if crc & 128 else 0)) & 255
    return crc


def binary(data):
    return [(byte >> shift) & 1 for byte in data for shift in range(7, -1, -1)]


def bch_generator(short):
    degree, primitive = (14, 0x402b) if short else (16, 0x1002d)
    order = (1 << degree) - 1
    powers = []
    logs = {}
    value = 1
    for i in range(order):
        powers.append(value)
        logs[value] = i
        value <<= 1
        if value >> degree:
            value ^= primitive

    def multiply(a, b):
        return powers[(logs[a] + logs[b]) % order] if a and b else 0

    roots = set()
    for i in range(1, 25):
        root = i
        while root not in roots:
            roots.add(root)
            root = (root * 2) % order
    coefficients = [1]
    for root in sorted(roots):
        next_coefficients = [0] * (len(coefficients) + 1)
        for i, coefficient in enumerate(coefficients):
            next_coefficients[i] ^= multiply(coefficient, powers[root])
            next_coefficients[i + 1] ^= coefficient
        coefficients = next_coefficients
    assert all(x in (0, 1) for x in coefficients)
    return sum(value << i for i, value in enumerate(coefficients))


def encode(mode, tables, seed, cyclic=False):
    n = 16200 if mode['short'] else 64800
    rows = tables[(mode['rate'], 'S' if mode['short'] else 'N')]
    k = len(rows) * 360
    generator = bch_generator(mode['short'])
    parity = generator.bit_length() - 1
    length = k - parity
    count = (length - 80) // 1504
    body = bytearray()
    carry = crc8(bytes([0x01, 0x23, 0x10 | ((count - 1) & 15)] + [(seed + count - 1 + j) & 255 for j in range(184)])) if cyclic else 0x47
    for i in range(count):
        packet = bytes([0x01, 0x23, 0x10 | (i & 15)] + [(seed + i + j) & 255 for j in range(184)])
        body += bytes([carry]) + packet
        carry = crc8(packet)
    header = struct.pack('>BBHHBH', 0xf0, 0, 1504, len(body) * 8, 0x47, 0)
    data = binary(header + bytes([crc8(header)]) + body)
    data += [0] * (length - len(data))
    state = 0xa9
    for i in range(length):
        bit = ((state >> 14) ^ (state >> 13)) & 1
        state = ((state << 1) | bit) & 0x7fff
        data[i] ^= bit
    remainder = 0
    mask = (1 << parity) - 1
    for bit in data:
        feedback = bit ^ (remainder >> (parity - 1))
        remainder = ((remainder << 1) ^ (generator if feedback else 0)) & mask
    data += [(remainder >> bit) & 1 for bit in range(parity - 1, -1, -1)]
    checks = [0] * (n - k)
    q = (n - k) // 360
    for i, bit in enumerate(data):
        if bit:
            for address in rows[i // 360]:
                checks[(address + (i % 360) * q) % len(checks)] ^= 1
    for i in range(1, len(checks)):
        checks[i] ^= checks[i - 1]
    data += checks
    width = mode['bits']
    if width != 2:
        height = (len(data) + width - 1) // width
        data += [0] * (width * height - len(data))
        data = [data[column * height + row] for row in range(height) for column in mode['order']]
    data += [1] * (mode['slots'] * 90 * width - len(data))
    constellation = [complex(*point) for point in mode['points']]
    payload = [constellation[int(''.join(map(str, data[i:i + width])), 2)] for i in range(0, len(data), width)]
    return physical(mode['code'], payload), count


def physical(code, payload):
    generators = [0x90ac2ddd, 0x55555555, 0x33333333, 0x0f0f0f0f, 0x00ff00ff, 0x0000ffff, 0xffffffff]
    scrambling = f'{0x719d83c953422dfa:064b}'
    word = 0
    for i, generator in enumerate(generators):
        if code & (1 << (7 - i)):
            word ^= generator
    coded = [((word >> (31 - i // 2)) & 1) ^ ((code & 1) * (i % 2)) for i in range(64)]
    coded = [value ^ int(bit) for value, bit in zip(coded, scrambling)]
    start = [int(x) for x in f'{0x18d2e82:026b}']
    header = [(1 - 2 * bit) * cmath.exp(1j * (math.pi / 4 + math.pi / 2 * (i % 2))) for i, bit in enumerate(start)]
    header += [1j * (1 - 2 * bit) * cmath.exp(1j * (math.pi / 4 + math.pi / 2 * (i % 2))) for i, bit in enumerate(coded)]
    x, y = 1, 0x3ffff
    for i, symbol in enumerate(payload):
        rotation = (((x & 0x8050).bit_count() ^ (y & 0xff60).bit_count()) & 1) * 2 + ((x ^ y) & 1)
        payload[i] = symbol * (1j ** rotation)
        x = (x >> 1) | (((x & 0x81).bit_count() & 1) << 17)
        y = (y >> 1) | (((y & 0x4a1).bit_count() & 1) << 17)
    return header + payload


def superframe(symbols):
    period = (1 << 20) - 1
    x, y = [1] + [0] * (period - 1), [1] * period
    for i in range(20, period):
        x[i] = x[i - 20] ^ x[i - 17]
        y[i] = y[i - 20] ^ y[i - 18] ^ y[i - 9] ^ y[i - 3]
    output = bytearray()
    cursor = 0
    for i in range(612540):
        if i < 720 or (i >= 1440 and (i - 1440) % 1476 < 36):
            shift = (i + 524288) % period
            value = cmath.exp(1j * math.pi / 4) * 1j ** (2 * (x[shift] ^ y[shift]) + (x[i] ^ y[i]))
        else:
            value = symbols[cursor % len(symbols)]
            cursor += 1
        value *= cmath.exp(0.19j)
        output += struct.pack('<hh', round(value.real * 16000), round(value.imag * 16000))
    return output


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--cache', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    modes = json.loads((args.cache / 'modes.json').read_text())
    source = (args.cache / 'ldpc.cc').read_text()
    tables = {}
    for match in re.finditer(r'ldpc_tab_(\d+_\d+)([NS])\s*\[\d+\]\s*\[\d+\]\s*=\s*\{(.*?)\};', source, re.S):
        rate, size, raw = match.groups()
        rows = []
        for row in re.findall(r'\{([^{}]+)\}', raw):
            values = list(map(int, re.findall(r'\d+', row)))
            rows.append(values[1:values[0] + 1])
        tables[(rate, size)] = rows
    args.out.mkdir(parents=True, exist_ok=True)
    randomizer = random.Random(324107)
    for code in [132, 138, 184, 200, 214, 248]:
        mode = next(mode for mode in modes if mode['code'] == code)
        symbols, packets = encode(mode, tables, code)
        recording = bytearray()
        for symbol in symbols:
            sample = symbol * cmath.exp(0.37j) + complex(randomizer.gauss(0, 0.0005), randomizer.gauss(0, 0.0005))
            recording += struct.pack('<hh', round(sample.real * 16000), round(sample.imag * 16000))
        path = args.out / f'pls{code}.sigmf-data'
        path.write_bytes(recording)
        metadata = {'global': {'core:datatype': 'ci16_le', 'core:sample_rate': 1000000, 'core:version': '1.2.6', 'core:description': f'Synthetic DVB-S2X PLS {code}, symbol-rate IQ, 0.37 rad rotation, Gaussian noise sigma 0.0005', 'core:sha512': hashlib.sha512(recording).hexdigest()}, 'captures': [{'core:sample_start': 0, 'core:frequency': 0}], 'annotations': [{'core:sample_start': 0, 'core:sample_count': len(symbols), 'core:comment': f'{packets} packets, PID 0x123, packet i payload byte j = ({code}+i+j)%256'}]}
        path.with_suffix('.sigmf-meta').write_text(json.dumps(metadata, indent=2) + '\n')
        print(code, len(symbols), packets)
        if code == 214:
            mode = dict(mode, code=215)
            symbols, _ = encode(mode, tables, 214, cyclic=True)
            recording = superframe(symbols)
            sf_path = args.out / 'superframe0.sigmf-data'
            sf_path.write_bytes(recording)
            metadata['global']['core:description'] = 'Synthetic Annex E format 0, 256APSK, default scrambling codes, SF pilots enabled, 0.19 rad phase'
            metadata['global']['core:sha512'] = hashlib.sha512(recording).hexdigest()
            metadata['annotations'][0]['core:sample_count'] = 612540
            sf_path.with_suffix('.sigmf-meta').write_text(json.dumps(metadata, indent=2) + '\n')


if __name__ == '__main__':
    main()
