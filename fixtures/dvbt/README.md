# DVB-T receiver reference

`qpsk_2k_reference.sigmf-data` is an original synthetic IQ recording generated independently
of the Rust transmitter by `crates/modem-test-support/scripts/dvbt_reference.py` with NumPy.
It contains 160 OFDM symbols, 2K QPSK, code rate 1/2, guard 1/8, an 8 MHz channel and a
1750 Hz carrier offset. Samples are signed 16-bit little-endian I/Q at 64/7 MS/s.
This is a synthetic fixture, not an antenna recording.

Expected decoded transport packets have PID `0x123`, payload-only headers, and 184 payload
bytes increasing modulo 256. Payload byte zero advances once per transmitted packet; the
continuity counter is its low nibble. TPS reports cell byte `0x5a`.

The generator follows ETSI EN 300 744 V1.6.1 clauses 4.3–4.6, published as
[DVB BlueBook A012](https://dvb.org/wp-content/uploads/2019/12/a012_dvb-t_june_2015.pdf).
The generator and recording are original project work under GPL-3.0-or-later.
