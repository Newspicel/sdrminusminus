# DVB-S2X references

The receiver supports all 55 non-reserved additional normal and short-frame MODCODs in
EN 302 307-2 Table 17a, including 8/64/128/256APSK. The reserved PLS values in Table 17b
are not payload modes.

The constellation coordinates, column permutations and LDPC parity address matrices are
adapted from GNU Radio gr-dtv, copyright 2015–2019 Free Software Foundation, Inc., under
GPL-3.0-or-later. Source revision: `aee9fd3f79389c4282a98e8d62c8405c73fd91df`.
Full license text is included in THIRD_PARTY_NOTICES.md and the application notices.

`crates/modem-test-support/scripts/s2x_tables.py` downloads checksum-verified reference
sources and compiles their initialization expressions with clang++ to reproduce the Rust
tables. This compiler is used only by the developer tool; reception is pure Rust.

```
python3 crates/modem-test-support/scripts/s2x_tables.py --cache /tmp/s2x-reference --out crates/channels/src/datv/dvbs2
cargo fmt --all
```

Normative reference: [DVB BlueBook A083-2r4](https://dvb.org/wp-content/uploads/2022/02/A083-2r4_DVB-S2X_Draft-EN-302-307-2-v141_Feb_2024.pdf),
clauses 5.3–5.5 and Annexes B/C.

## Independent recordings

`pls*.sigmf-data` are synthetic complex signed 16-bit little-endian recordings at one
sample per symbol. `s2x_reference.py` independently implements the Python transmitter's
BBHEADER, PRBS, BCH, LDPC, interleaving and physical framing, using the checksum-pinned
GNU Radio mapping and parity matrices. They are not antenna recordings. Each has a fixed
0.37 radian phase offset and deterministic Gaussian noise; expected transport packets have
PID 0x123 and payload byte j of packet i equal to `(PLS code + i + j) % 256`.
The modes cover QPSK, 8APSK, 64APSK, 128APSK, 256APSK and short-frame 32APSK.

```
python3 crates/modem-test-support/scripts/s2x_reference.py --cache /tmp/s2x-reference --out fixtures/dvbs2x
```

`superframe0.sigmf-data` carries 72 complete repeated 256APSK PLFRAMEs in one Annex E
format 0 container, with SF pilots and default reference/payload codes. It uses cyclic
packet CRCs. The decoder retains the final transport packet until its CRC arrives in the
next frame; the standalone PLS recordings likewise yield one fewer packet than encoded.
