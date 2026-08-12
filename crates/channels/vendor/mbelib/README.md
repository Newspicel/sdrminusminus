# mbelib subset

This directory vendors the first-generation AMBE 3,600 × 2,400 decoder needed by D-STAR from
[szechyjs/mbelib](https://github.com/szechyjs/mbelib), commit
`9a04ed5c78176a9965f3d43f7aa1b1f5330e771f`. Only that decoder, its ECC support, shared
synthesis code and their headers are included. `dstar_wrapper.c` is local glue that owns codec
state and maps D-STAR's 72 serial bits into mbelib's code-vector matrix.

The upstream sources are ISC licensed; the complete notice is in `COPYRIGHT`.
