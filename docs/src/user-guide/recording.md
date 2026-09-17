# Recording and playback

Choose a recorder for the signal you need to save:

| Node | Records | Format |
|---|---|---|
| Recorder | One device IQ lane | SigMF |
| Baseband recorder | Filtered IQ from individual channels | SigMF |
| Audio recorder | Processed channel audio | 48 kHz, 16-bit PCM WAV |
| Time machine | Recent device IQ plus live capture | SigMF |

SigMF stores samples in `.sigmf-data` and frequency, sample rate, timing, and annotations in
`.sigmf-meta`. Keep both files together.

## Record IQ

1. Connect Device `IQ` to **Recorder** `IQ`.
2. Start the radio and press **Record**.
3. Press **Stop** to finish the files.

For multi-lane radios, the connected port selects the lane. Connect GPS `position` to include
location metadata. The recorder shows elapsed time, bytes, and write errors.

A clean server shutdown finalises active recordings. Forcibly ending the process can leave an
incomplete capture.

## Record a channel's audio

Connect channel `audio` outputs to **Audio recorder**. Press **Record** beside each channel you
want to save, then **Stop** to finish its WAV file.

Each channel gets a separate file after squelch, filtering, noise reduction, and AGC. Closed
squelch writes silence to preserve timing. Mode and device-rate changes do not stop audio
recording; removing a channel does. Headers update during capture so interrupted files remain
playable through the last finalised data.

Audio and IQ recording can run independently at the same time.

## Record a channel's baseband

Connect channel `baseband` outputs to **Baseband recorder**. Start and stop each channel separately.

Files contain IQ after frequency translation and filtering, before squelch. They preserve the
channel frequency and sample rate and use less storage than full-device IQ. Completed files appear
in the IQ library for playback.

A channel rebuild, including a mode or device-rate change, finishes the recording. Removing the
channel also finishes it.

## The IQ time machine

Capture a signal after it happens:

1. Connect Device `IQ` to **Time machine**, with optional GPS `position`.
2. Set a buffer duration and press **Arm**.
3. Press **Capture** to save the buffer and continue recording live IQ.
4. Press **Stop** to finish and remain armed, or **Disarm** to release the buffer.

Memory use is `seconds × sample rate × 8` bytes, up to the server's 1 GiB limit. The display shows
the required memory and maximum duration.

Sample rate is locked while armed. Retuning starts a new SigMF capture segment. The first
timestamp belongs to the oldest buffered sample.

## Storage

Recordings default to `sdrmm/recordings` under the platform data directory. Override it with:

```sh
sdrmm --recordings-dir /srv/sdrmm/recordings
```

Containers use `/data/recordings`; persist `/data`. The library rebuilds its IQ index from valid
SigMF pairs on disk. Audio files live in the `audio` subdirectory.

## Tags and notes

In **Library → Recordings**, choose **Annotate** and enter comma-separated tags and a note.
Search matches names, tags, and notes; click a tag to search for it.

Annotations are stored in SigMF metadata as `sdrmm:tags` and `core:description`, so they survive
downloads and index rebuilds. Duplicate tags merge while keeping the first spelling.

## Download and export

Download IQ as the original SigMF archive or a stereo float WAV with I and Q as separate channels.
WAV preserves samples but only part of the capture metadata. **Channel audio** provides the audio
WAV downloads.

Downloads stream from disk. Failed exports abort instead of returning an apparently complete,
truncated file.

## Play a recording

Choose **Open as source** in **Library → Recordings**. Connect the new playback Device to
channels and displays as you would a radio.

Playback uses the capture's centre frequency and sample rate. Use play, pause, stop, and seek to
review the same samples with different decoder settings. Recording playback is available in
release builds.

## Decoder logs are separate

For messages, identifiers, and positions, connect channel `events` to **Decoder log**.
Add **Export** for CSV or JSON. Logs store decoded results in SQLite; IQ files store the signal
needed to decode again.
