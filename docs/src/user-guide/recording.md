# Recording and playback

| Node | Records | Wire from | Format |
|---|---|---|---|
| Recorder | The Device's full IQ | Device `iq` | SigMF |
| Baseband recorder | One channel's filtered IQ | Channel `baseband` | SigMF |
| Audio recorder | One channel's audio | Channel `audio` | 48 kHz 16-bit WAV |
| Time machine | IQ from before you pressed the button | Device `iq` | SigMF |

A SigMF recording is two files: samples in `.sigmf-data`, frequency, rate, and time in
`.sigmf-meta`. Keep them together.

Decoded messages are not recordings. For those, wire `events` to a **Decoder log**.

## Record IQ

Wire Device `iq` to a **Recorder**, press **Record**, then **Stop**. On a multi-lane radio the
wired port picks the lane. Wire GPS `position` to store the location.

A clean server shutdown finishes open recordings. Killing the process can leave one incomplete.

## Record a channel

**Baseband recorder** keeps a channel's IQ after filtering and before squelch. The files are much
smaller than full Device IQ and can be played back like any other recording. Changing the mode or
the Device rate, or removing the channel, ends the file.

**Audio recorder** keeps what you hear, after squelch, filters, and AGC. Closed squelch writes
silence so timing stays intact. Mode and rate changes do not stop it. The file stays playable even
if the server stops mid-recording.

Both recorders take several channels. Start and stop each with its own button.

## Time machine

Capture a signal after it happened:

1. Wire Device `iq` to **Time machine**, and GPS `position` if you have one.
2. Set how many seconds to keep and press **Arm**.
3. Press **Capture** to save the buffer and keep recording live.
4. **Stop** ends the file and stays armed. **Disarm** frees the memory.

The buffer uses `seconds × sample rate × 8` bytes, up to 1 GiB. The node shows both. The sample
rate is locked while armed. Retuning starts a new segment in the same recording.

## Play a recording

In **Library → Recordings**, press **Open as source**. A **Recording** node appears. Wire it to
channels and displays like a Device, then use play, pause, and seek to decode the same samples
again with different settings.

**Upload SigMF** adds a recording from your computer, as a `.sigmf` archive or a
`.sigmf-meta` and `.sigmf-data` pair.

## Tags and notes

In **Library → Recordings**, choose **Annotate** to add comma-separated tags and a note. Search
covers names, tags, and notes. Annotations live in the SigMF metadata, so they travel with the
files.

## Download

Download IQ as the original SigMF archive or as a stereo float WAV with I and Q as channels. WAV
keeps the samples but not all metadata. A failed download aborts instead of handing you a
truncated file.

## Where files go

Recordings go to `sdrmm/recordings` in the platform data folder, with audio in `audio/`. Change it
with `--recordings-dir`. Containers use `/data/recordings`. The library rebuilds itself from the
SigMF files on disk.
