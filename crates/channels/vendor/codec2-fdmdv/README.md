# Codec2 FDMDV subset

This directory vendors the FDMDV modem used by FreeDV 1600 from
[drowe67/codec2](https://github.com/drowe67/codec2), commit
`310777b1c6f1af0bc7c72f5b32f80f6fd9136962`. It contains `fdmdv.c`, its generated
filter tables, and the bundled Kiss FFT implementation. Codec2 speech synthesis is not
duplicated here; the channel uses the existing pure-Rust `codec2` crate.

`freedv_wrapper.c` is local glue that fixes the FreeDV 1600 carrier count, keeps the upstream
modem state opaque to Rust, and returns one allocation-free demodulation result.

The upstream FDMDV sources are LGPL-2.1-only; the complete license is in `COPYING`. Kiss FFT is
BSD-3-Clause as stated in its source headers.
