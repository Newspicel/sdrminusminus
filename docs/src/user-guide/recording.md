# Recording and playback

sdr-- records a device stream as a SigMF pair: metadata in `.sigmf-meta` and complex samples in
`.sigmf-data`. The format preserves the center frequency, sample rate, timing, and capture details
needed to process the IQ again.

## Record IQ

1. Add a Recorder node.
2. Wire a Device `IQ` output into the Recorder `IQ` input.
3. Start the radio, then press **Record**.
4. Press **Stop** to finalize the files.

For a multi-stream device, the source port chooses which stream is recorded. The Recorder face
shows elapsed time, bytes written, dropped samples, and write errors.

Stop the server cleanly when a recording is active. Graceful shutdown joins the writer and
finalizes the pair; forcibly killing the process can leave an incomplete capture.

## Record a channel's audio

An **Audio recorder** saves channel audio after squelch, filtering, noise reduction, and AGC.

1. Add an **Audio recorder**.
2. Connect one or more channel `audio` outputs to its `audio` input.
3. Press **Record** beside a channel to start its file, then **Stop** to finish it.

Each channel gets a separate 16-bit PCM WAV at 48 kHz. Closed squelch writes silence, preserving
the timing of quiet periods. Recording continues through mode and device sample-rate changes;
removing the channel finishes its file. WAV headers are updated during recording so an interrupted
file remains playable up to the last finalized data.

Audio and device IQ recording operate independently. You can run both and stop either one separately.

## Record a channel's baseband

A **Baseband recorder** saves channel IQ after frequency translation and filtering, at the channel's
sample rate. This uses less storage than recording the full device bandwidth.

1. Add a **Baseband recorder**.
2. Connect one or more channel `baseband` outputs to its `baseband` input.
3. Press **Record** beside a channel to start its SigMF pair, then **Stop** to finish it.

Each pair records the channel's absolute centre frequency: device centre plus channel offset.
The recording tap is before squelch, so closed squelch does not interrupt capture.

Completed pairs appear in the IQ recording library and can be opened as playback sources.
A change that rebuilds the channel, including a mode or device sample-rate change, finishes the
file because the recording cannot change sample rate mid-file. Removing the channel also finishes it.

## The IQ time machine

A **Time machine** keeps recent device IQ in memory. Use it to capture a signal after hearing it.

1. Add a **Time machine** and connect Device `IQ`. Optionally connect GPS `position`.
2. Set the buffer duration and press **Arm**.
3. Press **Capture** to write the buffered samples to a SigMF pair and continue recording live IQ.
4. Press **Stop** to finish the pair and stay armed, or **Disarm** to release the buffer.

Memory use is `seconds × sample rate × 8` bytes. The node shows the required memory and the
maximum duration allowed by the server's 1 GiB limit.

The sample rate is locked while armed. Retuning remains available and creates a new SigMF capture
segment. The recording's first timestamp belongs to the oldest buffered sample.

## Storage

The headless server stores recordings in the platform data directory by default, under
`sdrmm/recordings`. Set an explicit location with:

```sh
sdrmm --recordings-dir /srv/sdrmm/recordings
```

The container fixes this path at `/data/recordings`, so persist `/data` with a volume. The server
reconciles the database index with valid SigMF pairs on disk when listing recordings; the files
remain the source of truth.

## Tags and notes

Open **Library → Recordings** and press **Annotate**. Enter comma-separated tags and a note.
Duplicate tags are merged, keeping the first spelling.

Annotations are stored in `.sigmf-meta` as `sdrmm:tags` and `core:description`. They stay with
SigMF downloads and survive rebuilding the database index. Library search matches file names,
tags, and notes; clicking a tag searches for it.

## Download and export

Open **Library → Recordings** to inspect captures. A recording can be downloaded as:

- its original SigMF archive;
- a stereo floating-point WAV with I and Q as its two channels for tools such as HDSDR, SDR#, or
  Audacity. WAV keeps the samples but only part of the SigMF metadata.

Audio recordings appear under **Channel audio** in the same drawer and download as WAV files.
They are stored in the `audio` subdirectory of the recordings directory and listed directly from disk.

Large downloads include a content length and stream from disk. If an export fails, the response
is aborted instead of returning a silently truncated file.

## Play a recording

In **Library → Recordings**, choose **Open as source**. sdr-- adds a virtual playback Device to the
canvas. Wire it to channels and displays exactly as you would a live receiver.

Playback is pinned to the capture's center frequency and sample rate. The device transport lets
you play, pause, stop, and seek without changing the recording. Reopening the same capture is
useful for testing different channels, decoder settings, or graph layouts against identical IQ.

## Decoder logs are separate

IQ recording saves raw device samples. Decoder logs save structured output such as messages,
identifiers, and positions in SQLite. Wire decoder event outputs to a Decoder log and optionally
an Export node when you need CSV or JSON rather than raw RF samples.
