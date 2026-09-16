import binascii
import hashlib
import json
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[3] / "fixtures" / "dab"
H = [
    [0, 2, 0, 0, 0, 0, 1, 1, 2, 0, 0, 0, 2, 2, 1, 1] * 2,
    [0, 3, 2, 3, 0, 1, 3, 0, 2, 1, 2, 3, 2, 3, 3, 0] * 2,
    [0, 0, 0, 2, 0, 2, 1, 3, 2, 2, 0, 2, 2, 0, 1, 3] * 2,
    [0, 1, 2, 1, 0, 3, 3, 2, 2, 3, 2, 1, 2, 1, 3, 2] * 2,
]
PHASE = {
    "ii": [(0, 2), (1, 3), (2, 2), (3, 2), (0, 1), (1, 2), (2, 0), (1, 2), (0, 2), (3, 1), (2, 0), (1, 3)],
    "iii": [(0, 2), (1, 3), (2, 0), (3, 2), (2, 2), (1, 2)],
    "iv": [(0, 0), (1, 1), (2, 1), (3, 2), (0, 2), (1, 2), (2, 0), (3, 3), (0, 3), (1, 1), (2, 3), (3, 2), (0, 0), (3, 1), (2, 0), (1, 2), (0, 0), (3, 1), (2, 2), (1, 2), (0, 2), (3, 1), (2, 3), (1, 0)],
}


def fib(*figures):
    body = b"".join(bytes([kind * 32 + len(data)]) + data for kind, data in figures)
    assert len(body) <= 30
    body = body.ljust(30, b"\xff")
    return body + (binascii.crc_hqx(body, 0xffff) ^ 0xffff).to_bytes(2, "big")


def label(extension, identifier, text):
    return bytes([extension]) + identifier.to_bytes(2, "big") + text.encode().ljust(16, b" ") + b"\xff\xff"


def encode(fibs):
    source = np.unpackbits(np.frombuffer(b"".join(fibs), dtype=np.uint8)).tolist()
    register = 511
    for at in range(len(source)):
        bit = ((register >> 8) ^ (register >> 4)) & 1
        register = ((register << 1) | bit) & 511
        source[at] ^= bit
    state = [0] * 6
    output = []
    generators = [0o133, 0o171, 0o145, 0o133]
    for bit in source + [0] * 6:
        window = [bit] + state
        output.extend(sum(window[index] for index in range(7) if poly & (1 << (6 - index))) % 2 for poly in generators)
        state = window[:6]
    full = 29 if len(fibs) == 4 else 21
    pi16 = [1, 1, 1, 0] * 8
    pi15 = [1, 1, 1, 0] * 7 + [1, 1, 0, 0]
    mask = pi16 * (4 * full) + pi15 * 12 + [1, 1, 0, 0] * 6
    assert len(output) == len(mask)
    return [value for value, keep in zip(output, mask) if keep]


def render(name, useful, guard, null, symbols, cifs):
    carriers = useful * 3 // 4
    half = carriers // 2
    permutation = []
    at = 0
    for _ in range(useful - 1):
        at = (13 * at + useful // 4 - 1) % useful
        if useful // 8 <= at <= useful * 7 // 8 and at != useful // 2:
            permutation.append((at - useful // 2) % useful)
    reference = np.zeros(useful, dtype=np.complex128)
    for index, carrier in enumerate(list(range(-half, 0)) + list(range(1, half + 1))):
        table, offset = PHASE[name][index // 32]
        reference[carrier % useful] = (1j) ** (H[table][index % 32] + offset)
    information = [
        fib((0, bytes.fromhex("00 4a2c 00 00 00")), (0, bytes.fromhex("01 0400 8848")), (0, bytes.fromhex("02 c201 01 3f 06"))),
        fib((1, label(0, 0x4a2c, "Reference DAB"))),
        fib((1, label(1, 0xc201, "Reference audio"))),
    ]
    if name == "iii":
        information.append(fib())
    fic = encode(information) * cifs
    rng = np.random.default_rng(300401)
    bits = fic + rng.integers(0, 2, cifs * 55296).tolist()
    assert len(bits) == (symbols - 1) * carriers * 2
    waveform = [np.zeros(null, dtype=np.complex128)]
    previous = reference[permutation]
    for symbol in range(symbols):
        if symbol:
            chunk = np.array(bits[(symbol - 1) * carriers * 2:symbol * carriers * 2])
            previous *= ((1 - 2 * chunk[:carriers]) + 1j * (1 - 2 * chunk[carriers:])) / np.sqrt(2)
        bins = np.zeros(useful, dtype=np.complex128)
        bins[permutation] = previous
        time = np.fft.ifft(bins) * np.sqrt(useful)
        waveform.extend([time[-guard:], time])
    iq = np.concatenate(waveform).astype("<c8")
    data = iq.tobytes()
    stem = ROOT / f"mode_{name}_reference_2m048"
    stem.with_suffix(".sigmf-data").write_bytes(data)
    metadata = {
        "global": {"core:datatype": "cf32_le", "core:sample_rate": 2048000, "core:version": "1.2.5", "core:license": "https://www.gnu.org/licenses/gpl-3.0.html", "core:description": f"Frozen independent Python/NumPy synthetic DAB Mode {name.upper()} frame; EN 300 401 V1.4.1 clauses 11.2 and 14. Not an off-air recording. MSC is random data.", "core:sha512": hashlib.sha512(data).hexdigest()},
        "captures": [{"core:sample_start": 0, "core:frequency": 220352000}],
        "annotations": [{"core:sample_start": 0, "core:sample_count": len(iq), "core:label": "Expected ensemble 0x4a2c Reference DAB; service 0xc201 Reference audio; 96 kbit/s DAB+"}],
    }
    stem.with_suffix(".sigmf-meta").write_text(json.dumps(metadata, indent=2) + "\n")


if __name__ == "__main__":
    ROOT.mkdir(parents=True, exist_ok=True)
    for args in [("ii", 512, 126, 664, 76, 1), ("iii", 256, 63, 345, 153, 1), ("iv", 1024, 252, 1328, 76, 2)]:
        render(*args)
