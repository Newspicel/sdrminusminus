# MPEG Layer II audio reference vectors

Original synthetic tones encoded with FFmpeg's native MPEG Layer II encoder. These are
compressed audio fixtures, not antenna recordings. They exercise the shared DAB/DVB audio path.

- `tone_48k_mono.mp2`: 20 frames, 48 kHz, 64 kbit/s mono, 1 kHz tone.
- `tone_48k_mono.f32`: FFmpeg's decoded reference PCM, little-endian float32 mono.
- `tone_32k_stereo.mp2`: 10 frames, 32 kHz, 128 kbit/s stereo, left 700 Hz and right 1.3 kHz.

The mono fixture is also carried through the DAB and DVB test multiplex generators.
DAB test carriage checks the MSC-to-audio path; the elementary fixture lacks DAB-specific
ancillary data and is not a complete DAB audio encoder conformance vector.
The samples and fixtures are original project work under GPL-3.0-or-later.

Reproduce with:

```sh
ffmpeg -f lavfi -i 'sine=frequency=1000:sample_rate=48000:duration=0.48' -c:a mp2 -b:a 64k -ac 1 -f mp2 tone_48k_mono.mp2
ffmpeg -i tone_48k_mono.mp2 -c:a pcm_f32le -f f32le tone_48k_mono.f32
ffmpeg -f lavfi -i 'aevalsrc=0.2*sin(2*PI*700*t)|0.1*sin(2*PI*1300*t):s=32000:d=0.36' -c:a mp2 -b:a 128k -ac 2 -f mp2 tone_32k_stereo.mp2
```

## AAC, Dolby and video

These synthetic fixtures use FFmpeg 9.0.1. `tone.aac`, `tone.ac3` and `tone.eac3` contain
0.4 seconds of stereo audio: left 700 Hz at amplitude 0.2 and right 1300 Hz at amplitude 0.1.
AAC uses 96 kbit/s ADTS; Dolby streams use 192 kbit/s. The `pattern` files encode FFmpeg's
160 × 96 `testsrc2` at 25 frames/s. MPEG-2 has 12 frames; H.264 and HEVC have 10. All include
B pictures so tests exercise decoder reordering and fragmented input.

```sh
ffmpeg -f lavfi -i 'aevalsrc=0.2*sin(2*PI*700*t)|0.1*sin(2*PI*1300*t):s=48000:d=0.4' -c:a aac -b:a 96k -f adts tone.aac
ffmpeg -f lavfi -i 'aevalsrc=0.2*sin(2*PI*700*t)|0.1*sin(2*PI*1300*t):s=48000:d=0.4' -c:a ac3 -b:a 192k -f ac3 tone.ac3
ffmpeg -f lavfi -i 'aevalsrc=0.2*sin(2*PI*700*t)|0.1*sin(2*PI*1300*t):s=48000:d=0.4' -c:a eac3 -b:a 192k -f eac3 tone.eac3
ffmpeg -f lavfi -i testsrc2=size=160x96:rate=25 -t 0.48 -c:v mpeg2video -b:v 64k -maxrate 64k -bufsize 128k -g 6 -bf 2 -f mpeg2video pattern.m2v
ffmpeg -f lavfi -i testsrc2=size=160x96:rate=25 -t 0.4 -c:v libx264 -preset medium -g 6 -bf 2 -f h264 pattern.h264
ffmpeg -f lavfi -i testsrc2=size=160x96:rate=25 -t 0.4 -c:v libx265 -preset medium -x265-params log-level=error:pools=none -g 6 -bf 2 -f hevc pattern.hevc
```

## Independent DAB+ reference

`dab_*.aus` contains 30 AAC access units, each prefixed by its two-byte big-endian length.
`.asc` is the AudioSpecificConfig; `.pcm` is stereo float32 little-endian reference output.
The original fixture generator is `crates/modem-test-support/scripts/dab_audio_reference.c`.
It generates 960-sample AAC-LC, 960-sample HE-AAC, and HE-AAC v2 with parametric stereo.

The encoder is Opendigitalradio/fdk-aac, branch `dabplus2`, commit
`571316f329948dc464cc2be37b210f0b3e7816f7`. Add its omitted
`libMpegTPEnc/src/tpenc_dab.cpp` to the CMake source list before compiling the static library.
Build the fixture tool against its AACenc, AACdec and SYS include directories and static library:

```sh
cc crates/modem-test-support/scripts/dab_audio_reference.c -I<source>/libAACenc/include -I<source>/libAACdec/include -I<source>/libSYS/include <build>/libfdk-aac.a -lc++ -lm -o /tmp/dab-audio-reference
/tmp/dab-audio-reference fixtures/broadcast_audio/dab_lc_mono_48k 2 48000 1
/tmp/dab-audio-reference fixtures/broadcast_audio/dab_he_stereo_48k 5 48000 2
/tmp/dab-audio-reference fixtures/broadcast_audio/dab_he_ps_32k 29 32000 2
```

The checked-in PCM was decoded independently using upstream mstorsjo/fdk-aac v2.0.3,
commit `716f4394641d53f0d79c9ddac3fa93b03a49f278`. Compile the same tool against that library
and invoke it with only the fixture's basename to regenerate PCM. Convert the 32 kHz reference
with `ffmpeg -f f32le -ar 32000 -ac 2 -i dab_he_ps_32k.pcm -ar 48000 -f f32le dab_he_ps_32k_48k.pcm`.
FDK is a fixture-generation tool only, not a product dependency or bundled library.

Tests compare both channels after aligning polarity and codec delay, with stricter error
bounds for AAC-LC than for the independently reconstructed SBR/PS output. They also verify
sample counts and packet-fragmentation invariance. They are not AAC bit-exact conformance tests.

`slideshow.png` is an original 8 × 8 RGB image filled with (240, 60, 30), encoded using Python's
stdlib zlib and PNG chunk CRCs. The DAB generator carries it through CRC-protected MOT header
and body segments alongside the UTF-8 dynamic label `SDR-- live`, inside AAC data-stream elements.
