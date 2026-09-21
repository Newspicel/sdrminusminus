# DVB-T2 references

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

RF fixtures contain two frames with expected transport packets in matching `.ts` files:

| File | Transmission |
| --- | --- |
| `rf_2k_qpsk` | 2K, PP1, 1/4 guard, short 1/2 QPSK |
| `rf_8k_qpsk` | 8K, PP8, 1/128 guard, short 1/2 QPSK |
| `rf_8k_miso` | 8K MISO through two complex channels |
| `rf_32k_qpsk` | 32K, PP8, 1/128 guard |
| `rf_2k_lite` | T2-Lite, short 1/3, rotated 16-QAM |
| `rf_32k_media` | Normal 2/3, rotated 256-QAM, H.264 and MPEG audio |

`dvbt2_rf_reference.py` independently generates P1, L1-pre/post FEC, time and
frequency interleaving, pilots, OFDM and guard intervals. It reads P1, reserved-tone
and continual-pilot tables from the standard. Media comes from `fixtures/broadcast_audio`.

```sh
python3 crates/modem-test-support/scripts/dvbt2_rf_reference.py standard.txt fixtures/dvbt2
```

The RF generator requires NumPy. Tests compare every transport packet, exercise noise,
echoes, carrier offsets, a 2.048 MS/s source, and audio/video channel output.
RF-to-TS processing is checked for allocations. These are synthetic vectors, not
recordings or a receiver-conformance certification. GSE and multi-RF TFS are unsupported.
