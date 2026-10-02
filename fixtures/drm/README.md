# DRM audio reference vectors

Synthetic tones encoded as 960-sample AAC for DRM. Each `.aus` holds 30 access units, each with a
two-byte big-endian length. `.asc` is the AudioSpecificConfig, `.pcm` the reference decode as
stereo float32 little-endian at the stream's output rate (mono is duplicated). The left
channel is a 700 Hz tone, the right 1.3 kHz.

| File | Codec | Output rate | Bitrate | DRM use |
|---|---|---|---|---|
| `drm_he_mono_24k` | HE-AAC mono | 24 kHz | 12 kbit/s | DRM30, 12 kHz core |
| `drm_he_ps_24k` | HE-AAC v2 (PS) | 24 kHz | 16 kbit/s | DRM30, 12 kHz core |
| `drm_lc_stereo_24k` | AAC-LC stereo | 24 kHz | 24 kbit/s | DRM30 |
| `drm_lc_mono_48k` | AAC-LC mono | 48 kHz | 48 kbit/s | DRM+ |

The encoder is Opendigitalradio/fdk-aac, branch `dabplus2` (it encodes 960-sample frames), with
`libMpegTPEnc/src/tpenc_dab.cpp` added to the CMake source list. Build the tool against its
AACenc, AACdec and SYS include directories and static library:

```sh
cc crates/modem-test-support/scripts/drm_audio_reference.c -I<source>/libAACenc/include -I<source>/libAACdec/include -I<source>/libSYS/include <build>/libfdk-aac.a -lstdc++ -lm -o /tmp/drm-audio-reference
/tmp/drm-audio-reference fixtures/drm/drm_he_mono_24k 5 24000 1 12000
/tmp/drm-audio-reference fixtures/drm/drm_he_ps_24k 29 24000 2 16000
/tmp/drm-audio-reference fixtures/drm/drm_lc_stereo_24k 2 24000 2 24000
/tmp/drm-audio-reference fixtures/drm/drm_lc_mono_48k 2 48000 1 48000
```

Only the `.pcm` files that tests read are kept.

## DRM frames

`.drm` holds the same access units in DRM form (CRC byte, ER-AAC with VCB11 and HCR, SBR data
reversed at the frame end), each with a two-byte length. The project's DRM encoder writes them:

```sh
cargo test -p sdrmm-channels --lib drm::aac::tests::regenerate -- --ignored
```

FDK's own DRM decoder (`TT_DRM`) checks them independently. Its output matched the `.pcm`
references bit-exactly for both AAC-LC files, and within 0.0014 for HE-AAC:

```sh
/tmp/drm-audio-reference drm fixtures/drm/drm_lc_stereo_24k 1300
/tmp/drm-audio-reference drm fixtures/drm/drm_lc_mono_48k 0500
/tmp/drm-audio-reference drm fixtures/drm/drm_he_mono_24k 2100
```

The hex argument is the SDC audio information from the audio coding field on. FDK is a
fixture tool only, not a product dependency. The tones and fixtures are original project work
under AGPL-3.0-or-later.
