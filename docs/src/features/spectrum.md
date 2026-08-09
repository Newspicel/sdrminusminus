# Spectrum and waterfall

The FFT runs on the server. The browser receives quantized bins and draws them; it never sees
IQ.

## What the server computes

The DSP thread taps the corrected IQ stream before channelization and runs a windowed complex
FFT at 30 frames per second:

- **Complex FFT, not real-input.** IQ is complex, so the bins span the full `[0, fs)` and are
  fft-shifted so DC lands in the centre bin.
- **Hann window**, normalized by its coherent gain, so a full-scale complex tone at a bin
  centre reads about 0 dBFS. Levels are absolute and comparable between devices.
- **4096-point FFT.** The plan is built once and the hot path allocates nothing.
- **Max-hold decimation** to the subscriber's bin count. Averaging would hide a narrow
  carrier between bins; taking the peak of each group keeps it visible.
- **`u8` quantization** over an adaptive dB window — 80 dB below the frame peak by default.
  The window travels in the frame header, so the client maps bytes back to real dB.

Each frame is broadcast once and every subscriber decimates and quantizes it for itself, so
a phone asking for 1024 bins costs no extra FFT.

## What the client asks for

Subscriptions are per connection:

```json
{"type":"SubscribeSpectrum","data":{"device_set":0,"fps":20,"bins":2048}}
```

The server clamps both values (bins to 4096, `PLAN.md` §9). A phone can ask for 10 fps and
1024 bins while a desktop takes 30 fps and 4096 — same device set, same FFT, different
budgets. Frames arrive as binary WebSocket messages; see
[API and automation](../operating/api.md) for the header layout.

Backpressure is drop-oldest per connection. A slow client falls behind in frames, never in
the DSP.

## Rendering

The waterfall is a WebGL2 scrolling texture: one row per frame written into a ring, with the
colormap applied in the shader. The spectrum line is drawn from the same frame. Colormaps are
applied client-side, so changing one costs nothing on the server.

Zoom is a client-side crop today. A true zoom FFT belongs to the channel analyzer and is not
built yet.

Channel markers are overlaid on the spectrum: each hosted channel draws at its offset, click
selects it, and the hit area is at least 40 px on touch screens.

## Reading it

- **Overruns.** The device set carries a cumulative count of samples dropped at the capture
  ring. If it grows, the DSP thread is not keeping up: the spectrum has gaps even though the
  status still says `running`. Lower the sample rate or run fewer channels.
- **The DC spike.** Most direct-conversion receivers show a spike at the tuned frequency. It
  is the receiver, not the air. RTL-SDR's `offset_tune` (E4000 only) or simply tuning a few
  hundred kHz off and using a channel offset both dodge it.
- **Mirror images** at symmetric offsets are IQ imbalance. DC and IQ correction run before
  the tap; residual images mean the correction has not converged or the front end is
  overloaded.
- **A rising noise floor with gain** that does not improve the signal means you are past the
  point where the LNA helps. Watch a weak signal's height above the floor, not its absolute
  level.
