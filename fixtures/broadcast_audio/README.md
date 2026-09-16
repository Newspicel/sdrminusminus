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
