# Animation and timelapse

## Frames

Frames turns a document into an animation. Switch it on from Actions > Canvas with the **Frames** row. A timeline appears along the bottom edge. Each frame is built from the layers of the document.

- **Add frame** adds a layer above the stack, as a new frame.
- The **bottom layer** is drawn under every frame, and the **top layer** over every frame. They are the animation background and foreground, shared by all frames.
- The timeline plays the frames. Use Play and Pause, and choose the direction (**Forward**, **Reverse** or **Ping-pong**) and the mode (**Loop** or **One-shot**).
- The **Frame rate** sets how fast it plays.

**Onion skins** show the frames before and after the current one, faded, so you can judge the motion. Set how many frames show (**Onion frames**), how strong they are (**Onion opacity**) and how they blend (**Onion blend**).

Open **Frames, onion skins and playback** for the full settings.

## Exporting an animation

Open Actions > Share and choose one of the animation formats:

- **Animated GIF** keeps a transparent background if you ask for one, and **Dither GIF** smooths gradients in the palette.
- **Animated PNG (APNG)** can keep transparency.
- **MP4 animation** is not available in the 0.1 preview builds. The row says "MP4 export is not available in this build. Export a GIF, APNG or PNG sequence instead." Export an animated GIF or APNG, or a PNG sequence. (Builds made from source with the `mp4` feature can write H.264 video at a chosen quality; MP4 has no transparency.)

Exports run in the background. A message reports when the file is written.

## Timelapse

Eskiz can record how a picture is made. Each recording is kept with the document. Each committed change adds one frame to it.

- **Timelapse recording** turns recording on or off for the document. The **Timelapse, full length** and **Timelapse, 30 s summary** exports make a video of the whole session or a 30-second version of it. In the 0.1 preview builds there is no video encoder, so these rows are grayed out with the message above. Eskiz still records small pictures of your session, so nothing is lost; recording and in-app replay keep working.
- **Timelapse quality** is **Low**, **Standard**, **High** or **Studio**.
- **Timelapse resolution** is **1080p**, **2K** or **4K**.
- **Timelapse replay** plays the recording back inside the app, as a preview.

The Preferences tab sets whether new documents start with timelapse recording on.
