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

## Record a channel's baseband

A Baseband recorder node records what a channel *receives*: its own IQ, down-converted to the
channel's center, filtered to the channel's width, and at the channel's own sample rate — a few
tens of kHz rather than the radio's megahertz.

1. Add a Baseband recorder node.
2. Wire one or more Channel `baseband` outputs into its `baseband` input.
3. Press **Record** beside a channel to start its own pair, **Stop** to finish it.

Each wired channel writes its own SigMF pair, centred on the channel (the radio's center plus the
channel offset), which is what lets a 12.5 kHz channel be kept for hours where the wideband stream
would fill a disk in minutes. The tap sits before the squelch, so a closed gate still records —
the file is what arrived at the channel, not what got through it.

The finished pair lands in the same library as a device recording and can be reopened as a
playback source. Because SigMF cannot change sample rate mid-file, anything that rebuilds the
channel — a mode change, or a rate change on the radio underneath — finishes the file rather than
splicing a second rate into it; removing the channel does the same.

## The IQ time machine

A Time machine node holds the last few seconds of a radio's IQ in memory so a signal can be
recorded *after* it has already been heard.

1. Add a Time machine node.
2. Wire a Device `IQ` output into it, and optionally a GPS `position` output.
3. Set how many seconds to hold, then press **Arm**. The rolling buffer starts filling.
4. Press **Capture** when something interesting has just gone past. The buffered seconds are
   written to a new SigMF pair, and live samples keep appending to it.
5. Press **Stop** to finalize that pair while staying armed, or **Disarm** to release the memory.

The window costs `seconds × sample rate × 8` bytes of memory, which the face shows; the server
refuses a window above 1 GiB and names the number of seconds that fits at the current rate. The
sample rate is locked while the buffer is armed, because a buffer measured in samples cannot
change what a sample means underneath. Retuning stays available: a retune inside the window lands
in the capture as its own SigMF capture segment, and the first segment is stamped with the wall
clock of the *oldest held sample*, not the moment the button was pressed.

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

Open **Library → Recordings** and press **Annotate** on a capture to give it tags and a note —
what was on the air, and what to remember about it. Tags are comma separated; a repeat of one
already on the recording is folded into the first spelling.

Both live in the recording's own `.sigmf-meta`, as `core:description` and `sdrmm:tags`, not only
in the server's index. An annotated capture keeps its tags when it is downloaded as a SigMF
archive, when the database is thrown away and rebuilt from the files, and when it is read by
another tool. The search box above the library filters on file name, tags and note together, and
clicking a tag searches for it.

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
