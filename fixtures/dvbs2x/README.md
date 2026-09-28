# DVB-S2X references

The receiver supports all 55 non-reserved additional normal and short-frame MODCODs in
EN 302 307-2 Table 17a, including 8/64/128/256APSK. The reserved PLS values in Table 17b
are not payload modes.

LDPC addresses, constellation points and interleaver patterns are generated from the ETSI
text: [EN 302 307-2 V1.4.1](https://www.etsi.org/deliver/etsi_en/302300_302399/30230702/01.04.01_60/en_30230702v010401p.pdf)
Tables 9a-17a and Annexes B/C, and [EN 302 307-1 V1.4.1](https://www.etsi.org/deliver/etsi_en/302300_302399/30230701/01.04.01_60/en_30230701v010401p.pdf)
Figures 9-11 and Annexes B/C. The scripts download the checksum-pinned PDFs and need
poppler's `pdftotext`. `--check` compares with the committed Rust; `dvbs2_spec_check.py`
checks the DVB-S2 tables, ring ratios, labels and interleavers. CI runs both checks.

```
python3 crates/modem-test-support/scripts/s2x_tables.py --cache /tmp/etsi --out crates/channels/src/datv/dvbs2
python3 crates/modem-test-support/scripts/s2x_tables.py --cache /tmp/etsi --out crates/channels/src/datv/dvbs2 --check
python3 crates/modem-test-support/scripts/dvbs2_spec_check.py --cache /tmp/etsi --dvbs2 crates/channels/src/datv/dvbs2
```

## Independent recordings

`pls*.sigmf-data` are synthetic complex signed 16-bit little-endian recordings at one
sample per symbol. `s2x_reference.py` independently implements the Python transmitter's
BBHEADER, PRBS, BCH, LDPC, interleaving and physical framing, using the tables parsed
from the ETSI text. They are not antenna recordings. Each has a fixed
0.37 radian phase offset and deterministic Gaussian noise; expected transport packets have
PID 0x123 and payload byte j of packet i equal to `(PLS code + i + j) % 256`.
The modes cover QPSK, 8APSK, 64APSK, 128APSK, 256APSK and short-frame 32APSK.

```
python3 crates/modem-test-support/scripts/s2x_reference.py --cache /tmp/etsi --out fixtures/dvbs2x
```

`superframe0.sigmf-data` carries 72 complete repeated 256APSK PLFRAMEs in one Annex E
format 0 container, with SF pilots and default reference/payload codes. It uses cyclic
packet CRCs. The decoder retains the final transport packet until its CRC arrives in the
next frame; the standalone PLS recordings likewise yield one fewer packet than encoded.
