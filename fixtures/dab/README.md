# DAB transmission-mode reference waveforms

These frozen synthetic SigMF captures contain one complete transmission frame in each of
Modes II, III and IV at 2.048 MS/s. Repeat a frame to exercise continuous acquisition.
They were produced by the independent Python/NumPy modulator in
`crates/modem-test-support/scripts/dab_reference.py`, not by `channels::testgen`.
They are not antenna recordings. The MSC carries random data, so these fixtures verify FIC
reception and service discovery, not audio decoding.

Expected output in every mode:

- Ensemble `0x4a2c`, `Reference DAB`
- Service `0xc201`, `Reference audio`, DAB+, 96 kbit/s
- No failed FIB CRCs

Frame geometry, FIC puncturing, phase references and interleaving follow
[ETSI EN 300 401 V1.4.1](https://www.etsi.org/deliver/etsi_en/300400_300499/300401/01.04.01_60/en_300401v010401p.pdf),
clauses 11.2 and 14, including tables 38 and 40–47. Modes II–IV were removed from later editions.
The generator and captures are original project work under GPL-3.0-or-later.
Each metadata file includes the sample count and SHA-512 of its data file.
