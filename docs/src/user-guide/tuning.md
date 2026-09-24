# Tuning

Tune the decoder, not the radio. Every Device starts on **Auto**: set a channel's frequency and
the radio moves its window to cover it.

## Auto

The **Tuning** row on the Device node shows the mode.

On Auto, every way of tuning moves a decoder:

| You | What moves |
|---|---|
| Turn or type on the Device dial | The decoder |
| Click in the Scope | The decoder, to that frequency |
| Drag in the Scope | The decoder, with the pointer |
| Left / Right keys | The decoder, one step |

Which decoder: the selected one, or the only one wired to that radio. If several are wired and
none is selected, the radio switches to Manual.

With several decoders, the radio picks a window that covers as many as it can. The count on the
Device node, such as `2/3`, shows how many it hears.

## Manual

The radio stays where you put it. Channels outside its window stop until it covers them again.

Tuning the radio itself switches to Manual. A note appears with **Back to Auto**. You can also
switch on the **Tuning** row.

Use Manual to watch a fixed band in the Scope, or when no decoder is wired.

## Lock

A lock next to a dial stops tuning by hand. A locked channel is never picked as the decoder to
move.
