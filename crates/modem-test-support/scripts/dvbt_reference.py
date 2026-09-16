import hashlib
import json
from collections import deque
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "fixtures" / "dvbt"
N = 2048
G = 256
SYMBOLS = 160
CONTINUAL = [0, 48, 54, 87, 141, 156, 192, 201, 255, 279, 282, 333, 432, 450, 483, 525, 531, 618, 636, 714, 759, 765, 780, 804, 873, 888, 918, 939, 942, 969, 984, 1050, 1101, 1107, 1110, 1137, 1140, 1146, 1206, 1269, 1323, 1377, 1491, 1683, 1704]
TPS = [34, 50, 209, 346, 413, 569, 595, 688, 790, 901, 1073, 1219, 1262, 1286, 1469, 1594, 1687]


def multiply(a, b):
    result = 0
    while b:
        if b & 1:
            result ^= a
        a <<= 1
        if a & 256:
            a ^= 0x11d
        b >>= 1
    return result


def polynomial():
    result = [1]
    root = 1
    for _ in range(16):
        product = [0] * (len(result) + 1)
        for i, coefficient in enumerate(result):
            product[i] ^= coefficient
            product[i + 1] ^= multiply(coefficient, root)
        result = product
        root = multiply(root, 2)
    return result


def reed_solomon(data):
    generator = polynomial()
    remainder = list(data) + [0] * 16
    for i in range(len(data)):
        factor = remainder[i]
        for j, coefficient in enumerate(generator):
            remainder[i + j] ^= multiply(factor, coefficient)
    return list(data) + remainder[-16:]


def coded_stream(count):
    dispersal = 0xa9
    trellis = 0
    lines = [deque([0] * (i * 17)) for i in range(12)]
    result = []

    def prbs_byte():
        nonlocal dispersal
        byte = 0
        for _ in range(8):
            feedback = ((dispersal >> 14) ^ (dispersal >> 13)) & 1
            dispersal = ((dispersal << 1) | feedback) & 0x7fff
            byte = (byte << 1) | feedback
        return byte

    for packet in range(count):
        data = [0x47, 0x01, 0x23, 0x10 | (packet & 15)] + [(packet + i) & 255 for i in range(184)]
        if packet % 8 == 0:
            dispersal = 0xa9
            data[0] = 0xb8
        else:
            prbs_byte()
        for i in range(1, 188):
            data[i] ^= prbs_byte()
        for i, byte in enumerate(reed_solomon(data)):
            line = lines[i % 12]
            line.append(byte)
            byte = line.popleft()
            for bit in range(7, -1, -1):
                trellis = (trellis >> 1) | (((byte >> bit) & 1) << 6)
                result += [(trellis & 0o171).bit_count() & 1, (trellis & 0o133).bit_count() & 1]
    return np.array(result, dtype=np.uint8)


def tps(frame):
    data = f'{0x35ee if frame % 2 == 0 else 0xca11:016b}' + '011111' + f'{frame:02b}' + '00' + '000' + '000' + '000' + '10' + '00' + '01011010' + '000000'
    value = int(data, 2) << 14
    remainder = value
    for degree in range(66, 13, -1):
        if remainder & (1 << degree):
            remainder ^= 0x4377 << (degree - 14)
    return [0] + [int(bit) for bit in data + f'{remainder:014b}']


def permutation():
    register = 0
    result = []
    destinations = [4, 3, 9, 6, 2, 8, 1, 5, 7, 0]
    for i in range(N):
        if i < 2:
            register = 0
        elif i == 2:
            register = 1
        else:
            register = (register >> 1) | (((register ^ (register >> 3)) & 1) << 9)
        address = (i & 1) << 10
        for bit, destination in enumerate(destinations):
            address |= ((register >> bit) & 1) << destination
        if address < 1512:
            result.append(address)
    return result


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    coded = coded_stream((SYMBOLS * 3024 + 3263) // 3264)
    reference = []
    register = 2047
    for _ in range(1705):
        reference.append(1 - 2 * (register & 1))
        register = (register >> 1) | (((register ^ (register >> 2)) & 1) << 10)
    order = permutation()
    frames = []
    phase = 1
    for symbol in range(SYMBOLS):
        if symbol % 68 == 0:
            signalling = tps((symbol // 68) % 4)
            phase = 1
        elif signalling[symbol % 68]:
            phase = -phase
        block = coded[symbol * 3024:(symbol + 1) * 3024].reshape(12, 126, 2)
        block[:, :, 1] = np.roll(block[:, :, 1], -63, axis=1)
        words = block.reshape(1512, 2)
        interleaved = np.empty_like(words)
        if symbol % 2 == 0:
            interleaved[order] = words
        else:
            interleaved = words[order]
        pilots = set(CONTINUAL) | set(range(3 * (symbol % 4), 1705, 12))
        carriers = [k for k in range(1705) if k not in pilots and k not in TPS]
        spectrum = np.zeros(N, dtype=np.complex128)
        for i, carrier in enumerate(carriers):
            a, b = map(int, interleaved[i])
            spectrum[(carrier - 852) % N] = complex(1 - 2 * a, 1 - 2 * b) / np.sqrt(2)
        for k in pilots:
            spectrum[(k - 852) % N] = reference[k] * 4 / 3
        for k in TPS:
            spectrum[(k - 852) % N] = reference[k] * phase
        time = np.fft.ifft(spectrum) * np.sqrt(N)
        frames.append(np.concatenate((time[-G:], time)))
    iq = np.concatenate(frames)
    iq *= np.exp(2j * np.pi * 1750 * np.arange(len(iq)) / (64e6 / 7))
    packed = np.empty((len(iq), 2), dtype='<i2')
    packed[:, 0] = np.round(iq.real * 6000).astype('<i2')
    packed[:, 1] = np.round(iq.imag * 6000).astype('<i2')
    path = OUT / 'qpsk_2k_reference.sigmf-data'
    packed.tofile(path)
    metadata = {'global': {'core:datatype': 'ci16_le', 'core:sample_rate': 64e6 / 7, 'core:version': '1.2.6', 'core:description': 'Independent synthetic DVB-T 2K QPSK 1/2 guard 1/8, 1750 Hz carrier offset; PID 0x123 deterministic packet payload', 'core:sha512': hashlib.sha512(path.read_bytes()).hexdigest()}, 'captures': [{'core:sample_start': 0, 'core:frequency': 650e6}], 'annotations': [{'core:sample_start': 0, 'core:sample_count': len(iq)}]}
    path.with_suffix('.sigmf-meta').write_text(json.dumps(metadata, indent=2) + '\n')


if __name__ == '__main__':
    main()
