# Recording and playback

| Node | Records | Wire from | Format |
|---|---|---|---|
| Recorder | The Device's full IQ | Device `iq` | SigMF |
| Baseband recorder | One channel's filtered IQ | Channel `baseband` | SigMF |
| Audio recorder | One channel's audio | Channel `audio` | 48 kHz 16-bit WAV |
| Time machine | IQ from before you pressed the button | Device `iq` | SigMF |
| Event output, Recordings | One WAV per call | Channel `events` | 8 kHz 16-bit WAV |

A SigMF recording is two files: samples in `.sigmf-data`, frequency, rate, and time in
`.sigmf-meta`. Keep them together.

**Record** on a recorder is a switch saved with the workspace. While it is on, the server records
whatever is wired in: it follows rewiring, keeps going with the browser closed, and starts again
after a restart.

Decoded messages are not recordings. For those, wire `events` to a **Decoder log**.

## Record IQ

Wire Device `iq` to a **Recorder**, press **Record**, then **Stop**. On a multi-lane radio the
wired port picks the lane. Wire GPS `position` to store the location. The sample rate is locked
while recording.

A clean server shutdown finishes open recordings. Killing the process can leave one incomplete.

## Time machine

Capture a signal after it happened:

1. Wire Device `iq` to **Time machine**, and GPS `position` if you have one.
2. Set **History**, 1 to 120 seconds (default 10), and press **Arm**.
3. Press **Capture** to save the buffer and keep recording live.
4. **Stop** ends the file and stays armed. **Disarm** frees the memory.

The buffer uses `seconds × sample rate × 8` bytes, up to 1 GiB. **Memory** shows it. The sample
rate is locked while armed. Retuning starts a new segment in the same recording.

## Record a channel

**Baseband recorder** keeps a channel's IQ after filtering and before squelch. The files are much
smaller than full Device IQ and can be played back like any other recording. Changing the mode or
the Device rate starts a new file. Removing the channel ends it.

**Audio recorder** keeps what reaches it, after squelch and any Audio FX it is wired behind.
Closed squelch writes silence so timing stays intact. Mode and rate changes do not stop it. The
file stays playable even if the server stops mid-recording.

Both recorders take several channels and write one file per wired input.

## Record each call

Wire a channel's `events` to an **Event output** set to **Recordings**. Every call becomes its own
WAV in the audio library, named by start time (UTC), mode, and frequency:

```text
20261001T223105Z_NFM_144.200MHz.wav
20260815T100000Z_DMR_451.125MHz_TG91.wav
```

What makes a call:

| Channel | A call is |
|---|---|
| Digital voice, DMR trunk | One call, from header to end |
| AM, NFM, SSB and other audio | One squelch opening. Gaps under 1.5 s stay in the call |

An analog channel with squelch off has no calls. Use the Audio recorder for continuous audio.
Calls longer than 10 minutes continue in a new file. Encrypted calls have no audio to save.
Several channels can share one Event output. Add an **Event filter** between them to skip short
calls or keep certain talkgroups. A [Spectrum monitor](scanning.md#monitor-a-band) wired here
saves its clips the same way.

## Record an array

**Rec** on an [Array](arrays.md) records every lane into one SigMF collection: a
`.sigmf-collection` file that ties one recording per lane together, with the geometry, tier and
noise source windows. **Stop** ends it. **Library → Recordings** lists the collection once, with
its lane count.

Press **Calibrate** while recording. Playback calibrates from the recorded noise windows.

## Play a recording

In **Library → Recordings**, press **Open as source**. A **Recording** node appears. Wire it to
channels and displays like a Device, then use play, pause, and seek to decode the same samples
again with different settings.

**Upload SigMF** adds a recording from your computer, as a `.sigmf` archive or a
`.sigmf-meta` and `.sigmf-data` pair. A downloaded array collection uploads whole.

## Play an array recording

Open a collection as a source. The **Recording** node gets one output per lane, `iq1` to `iq5` for
a five-lane array. Press **Make array**, or wire the outputs to an Array's lanes in order. Set the
Array's geometry as it was when recording. Wire a GPS to the Array's `position` and add a
processor, such as a [Direction finder](direction-finding.md). Each lane keeps its recorded
frequency, so the Array cannot retune. The Array calibrates on the recorded noise windows; when the
recording ends it keeps the last calibration.

## Tags and notes

In **Library → Recordings**, press the pencil to give a recording a name, comma-separated tags,
and a note. Search covers names, tags, and notes. Annotations live in the SigMF metadata, so they
travel with the files.

## Download

Download IQ as **.sigmf**, the original archive, or **.wav**, a stereo float WAV with I and Q as
channels. The WAV keeps only the centre frequency and start time from the metadata. An array
collection downloads as one SigMF archive with every lane, and has no WAV. A failed download
aborts instead of handing you a truncated file.

## Where files go

Recordings go to `sdrmm/recordings` in the platform data folder, with audio in `audio/`. Change it
with `--recordings-dir`. Containers use `/data/recordings`. The library rebuilds itself from the
SigMF files on disk.
