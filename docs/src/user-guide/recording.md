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

An Audio recorder node records what a channel *sounds* like, after everything the channel does to
it — squelch, filters, noise reduction, AGC.

1. Add an Audio recorder node.
2. Wire one or more Channel `audio` outputs into its `audio` input.
3. Press **Record** beside a channel to start its own file, **Stop** to finish it.

Each wired channel writes its own 16-bit PCM WAV at 48 kHz, so a net followed across several
receivers stays separable afterwards. A closed squelch writes silence rather than nothing: the file
is a timeline, and a gap in it is as long as the quiet that made it. Recording survives a mode or
sample-rate change on the radio underneath, and removing the channel finishes the file rather than
abandoning it. A recording is finalized as it goes, so a server that is killed still leaves a file
that plays up to the last second it captured.

Channel audio and device IQ are independent: a radio can be recording its raw stream while one of
its channels records audio, and either can be started or stopped without the other.

## Storage

The headless server stores recordings in the platform data directory by default, under
`sdrmm/recordings`. Set an explicit location with:

```sh
sdrmm --recordings-dir /srv/sdrmm/recordings
```

The container fixes this path at `/data/recordings`, so persist `/data` with a volume. The server
reconciles the database index with valid SigMF pairs on disk when listing recordings; the files
remain the source of truth.

## Download and export

Open **Library → Recordings** to inspect captures. A recording can be downloaded as:

- its original SigMF archive;
- a stereo floating-point WAV with I and Q as its two channels for tools such as HDSDR, SDR#, or
  Audacity. WAV keeps the samples but only part of the SigMF metadata.

Channel audio is listed in the same drawer, under **Channel audio**, and downloads as the WAV it
was recorded as. Audio recordings live beside the IQ library, in an `audio` directory under the
recordings directory; there is no index behind them, because a WAV describes itself.

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
