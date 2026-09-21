# DVB-T2 payload references

These synthetic FEC-block cells are little-endian complex float32 pairs, after cell
interleaving and before time interleaving. They are not RF recordings.

| File | Frame | Rate | Constellation | Rotation |
| --- | --- | --- | --- | --- |
| `normal_2_3_qam256.f32` | Normal | 2/3 | 256-QAM | Yes |
| `normal_3_5_qam64.f32` | Normal | 3/5 | 64-QAM | No |
| `short_3_5_qam64.f32` | Short | 3/5 | 64-QAM | Yes |
| `lite_1_3_qam16.f32` | T2-Lite | 1/3 | 16-QAM | Yes |

The FEC-block index is 3. Decoded bit `i` equals `(173*i + i//7) % 31 < 15`.
`short_3_5_bch_errors.f32` adds errors at BCH positions 123 and 3456 before LDPC encoding.

The independent Python encoder follows [ETSI EN 302 755 V1.4.1](https://www.etsi.org/deliver/etsi_en/302700_302799/302755/01.04.01_60/en_302755v010401p.pdf),
clauses 5.2.4 and 6, annexes A, B and I. It reads LDPC tables from the standard,
without importing Rust code. The generator and fixtures are GPL-3.0-or-later.

Regenerate using the standard's `pdftotext -layout` output:

```sh
python3 crates/modem-test-support/scripts/dvbt2_reference.py standard.txt fixtures/dvbt2
```

The receiver currently provides payload decoding only. RF synchronization,
P2/L1 signalling, pilot equalization, PLP scheduling and channel integration
remain unimplemented. These fixtures do not establish full DVB-T2 reception.
