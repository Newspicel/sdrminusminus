# Hardware tests

These tests need real radios, so they are ignored by default and CI never runs them. Run them on
an idle radio that no other program holds.

## Capture health

Measures loss on real radios, from USB through DSP to publication:

```sh
SDRMM_CAPTURE_DRIVER=hackrf SDRMM_CAPTURE_RATE=8000000 SDRMM_CAPTURE_SECONDS=30 \
  cargo test -p sdrmm-engine --lib --no-default-features --features rtlsdr,hackrf \
  connected_radio_capture_health -- --ignored --nocapture
```

`SDRMM_CAPTURE_DRIVER` is `hackrf` (default), `rtlsdr`, or `both`. Default rates are 20 MS/s for
HackRF and 2.4 MS/s for RTL-SDR; the default length is 10 s. The test fails on any loss unless told
otherwise.

| Variable | Does |
|---|---|
| `SDRMM_CAPTURE_CHANNELS=8` | Channels per radio, default 4 |
| `SDRMM_CAPTURE_SPREAD_HZ=2000000` | Spread channels over this span instead of 25 kHz steps |
| `SDRMM_CAPTURE_MIXED=1` | Cycle NFM, WFM, AM, and SSB |
| `SDRMM_CAPTURE_RETUNE=1` | Retune channels every 5 s |
| `SDRMM_CAPTURE_DEVICE_RETUNE=1` | Retune radios every 5 s |
| `SDRMM_CAPTURE_ALTERNATE_RATE=10000000` | With device retunes, alternate to this rate |
| `SDRMM_CAPTURE_RTL_RATE=3200000` | Override the RTL-SDR rate only |
| `SDRMM_CAPTURE_CPU_THREADS=4` | Add CPU load threads |
| `SDRMM_CAPTURE_RECORD=1` | Record IQ and verify sample counts |
| `SDRMM_CAPTURE_HISTORY=1` | Capture history, then record live, and verify |
| `SDRMM_CAPTURE_HISTORY_SECONDS=6` | History length, default 1 s |
| `SDRMM_CAPTURE_TRANSPORT_SECONDS=5` | Raw USB test length per radio, default up to 30 s; 0 skips it |
| `SDRMM_CAPTURE_ALLOW_DROPS=1` | Measure overload instead of failing |

Software counters miss some USB losses. For RTL-SDR, also check the hardware byte counter:

```sh
SDRMM_RTL_TEST_RATE=3200000 SDRMM_RTL_TEST_SECONDS=60 \
  cargo test -p sdrmm-device-rtlsdr --lib hardware_counter_stream_is_continuous -- --ignored --nocapture
```

Defaults are 2.4 MS/s for 30 s. `SDRMM_RTL_TEST_SERIAL` picks a dongle; otherwise the first one
outside a KrakenSDR is used. The 8-bit counter cannot see losses of exact multiples of 256 bytes.

## KrakenSDR

```sh
cargo test -p sdrmm-device-rtlsdr kraken_ -- --ignored --nocapture --test-threads=1
cargo test -p sdrmm-engine --test array_hardware -- --ignored --nocapture --test-threads=1
SDRMM_BENCH_ASSERT=mac cargo test -p sdrmm-engine --release \
  --test radar_hardware benchmark_radar -- --ignored --nocapture
SDRMM_RADAR_FM_HZ=<local FM in Hz> cargo test -p sdrmm-engine --release \
  --test radar_hardware kraken_fm_live -- --ignored --nocapture
```

Add `--features probe` to the array tests to check that no processor sees the noise source.
The radar benchmarks use synthetic FM and DAB signals and need no radio. `SDRMM_BENCH_ASSERT=pi5`
checks the Raspberry Pi 5 budget. Raw numbers land in `target/hardware/*.csv`.

`kraken_df_known_bearing` needs antennas and a transmitter:

| Variable | Value |
|---|---|
| `SDRMM_DF_TX_HZ` | Transmitter frequency |
| `SDRMM_DF_BEARINGS` | Bearings relative to element 1, at least 3, none within 10° of its axis or mirrored about it, like `40,130,250` |
| `SDRMM_DF_RADIUS_M` | Array radius, default 0.35 |
| `SDRMM_DF_WINDING` | `clockwise` (default) or `counter_clockwise` |
| `SDRMM_DF_GAIN_DB`, `SDRMM_DF_BANDWIDTH_HZ` | Gain, default 30; band, default 20 kHz |

It asks you to move the transmitter, takes 60 reports per bearing, and passes when every error is
under 5° or 2 sigma, the RMS error lies within 0.5 to 2 times the mean sigma, and at least 90% of
reports count one source.

### Measured

KrakenSDR 1000 to 1004, no antennas, 2.4 MS/s, Apple M4 Max.

Share of I/Q values at full scale with the noise source on, lane 1:

| MHz | 0 dB | 8.7 dB | 19.7 dB | 29.7 dB | 49.6 dB |
|---:|---:|---:|---:|---:|---:|
| 30 | 0 | 0.032 | 0.133 | 0.164 | 0.171 |
| 100 | 0 | 0.046 | 0.137 | 0.164 | 0.171 |
| 433.92 | 0 | 0.045 | 0.129 | 0.159 | 0.168 |
| 868 | 0 | 0.044 | 0.129 | 0.157 | 0.160 |
| 1090 | 0 | 0 | 0.022 | 0.098 | 0.116 |
| 1300 | 0 | 0 | 0.006 | 0.055 | 0.071 |
| 1700 | 0 | 0 | 0 | 0.004 | 0.007 |

Noise source solves over all 29 gain steps, lanes 2 to 5 against lane 1:

| MHz | Lowest coherence | Lowest pair coherence | Lowest purity | Most clipped samples | Phase from 0 dB |
|---:|---:|---:|---:|---:|---|
| 100 | 0.83 | 0.82 | 0.95 | 0.58 | −8.9° to +3.8° |
| 433.92 | 0.84 | 0.83 | 0.95 | 0.36 | −9.2° to +2.1° |
| 868 | 0.74 | 0.71 | 0.92 | 0.34 | −9.4° to +2.4° |
| 1090 | 0.91 | 0.91 | 0.97 | 0.28 | −9.3° to +2.5° |
| 1300 | 0.75 | 0.76 | 0.91 | 0.22 | −9.2° to +2.4° |
| 1700 | 0.68 | 0.67 | 0.89 | 0.06 | −9.8° to +1.9° |

Noise solves therefore accept lane coherence from 0.5, pair coherence from 0.5, and bin purity
from 0.8. They accept up to 75% clipped samples and add 2° to the calibration sigma.

| Check | Result |
|---|---|
| Start spread, 20 starts at 1.024, 2.4, 2.56 MS/s | 6.5 to 7.8 ms, about 1.7 ms per lane. Coarse search reaches 256 to 437 ms. |
| Lane 3 failed on purpose | Every lane reports `Rearmed` and streams again after 76 to 80 ms, offsets moved by up to 10 ms. |
| 2.88 MS/s | Refused: `KrakenSDR runs at most 2.56 MS/s` |
| Noise source off | Back at the noise floor within 1,024 samples |
| Noise source on exit | Off after a drop, a drop while streaming, a panic, and a server stop |
| Array, 433.92 MHz, 30 dB | Locked and solved in 2.3 s. 18 checks in 3 minutes: delay within 0.007 samples, phase within 0.9°. |
| Retune 100 to 433.92 to 868 MHz, gain 20 to 40 dB | Stale after 0.44 s, solved after 0.69 to 0.71 s |
| Processors during noise | None saw a noise block |
| Direction finder on noise only | 357 reports, 0 sources |

Source count on real tuners, 0 to 29.7 dB. Receiver noise alone: the largest eigenvalue sits at most
1 dB over the rest, dominance counts 0, MDL up to 4. Noise source in a 37.5 or 300 kHz band: the
second eigenvalue is 14 to 24 dB down, dominance counts 1, MDL 4. Over the full uncalibrated
2.4 MHz it is 5 to 11 dB down and dominance counts 2. Source counting therefore uses 6 dB and
12 dB thresholds.

Passive radar, median of 10 CPIs, M4 Max shared with other builds:

| | FM | Target | DAB | Target |
|---|---:|---:|---:|---:|
| Front, share of a core | 0.05 | 0.10 | 0.11 | 0.15 |
| CPI, one thread | 6.3 ms | 25 ms | 62 ms | 120 ms |
| CPI, crew of 3 | 5.2 ms | | 40 ms | 50 ms |
| CPI, Auto | 5.3 ms on the CPU | | 23 ms on the GPU | 30 ms |

The DAB CPI on the GPU stays under 30 ms at a load average of 20, but other apps busy on the GPU
can still push it past. Its GPU CAF alone takes 9 to 10 ms.

Not measured yet: bearings against a transmitter at known bearings, radar on a live FM station
(both need antennas), and a Raspberry Pi 5.
