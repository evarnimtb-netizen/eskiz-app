# Troubleshooting

## Linux: the window does not open

On Linux, Eskiz checks for the system libraries its window needs before it opens. If one is missing, it prints the names of the packages to install and exits with status 2. On Debian and Ubuntu the packages are, for example:

    sudo apt install libxkbcommon-x11-0 libxkbcommon0 libx11-xcb1 libwayland-client0

Eskiz also needs OpenGL and Vulkan drivers to draw the window: `libgl1 libvulkan1 libgl1-mesa-dri mesa-vulkan-drivers` on Debian and Ubuntu, and `mesa-libGL vulkan-loader mesa-dri-drivers mesa-vulkan-drivers` on Fedora. Install the packages the message names, then start Eskiz again.

## The window opens, but nothing draws, or the graphics fail

Eskiz draws with the graphics card through wgpu (Metal, DirectX 12 or Vulkan). If that fails when the window opens, Eskiz retries once with OpenGL. You can choose the path yourself with the **ESKIZ_RENDERER** environment variable:

- `gpu`, or not set: the default, with the GPU compositor.
- `cpu`: the same window, with the compositor on the processor. Use this if the picture looks wrong.
- `gl`: an OpenGL window, with the compositor on the processor.

Values are not case-sensitive. An unknown value means the default.

## Where the library is kept

Your drawings are kept in a library folder in your user data folder for Eskiz. To use another folder, set **ESKIZ_LIBRARY_DIR** to its path before starting Eskiz.

## Lost work

Eskiz saves when you leave the canvas, when the app closes, and every 30 seconds while there are unsaved changes. Each save is written to a temporary file and then put in place, so a save that is interrupted leaves the last good copy intact. There is no separate crash-recovery prompt. If Eskiz stops unexpectedly, the most recent changes, up to about 30 seconds of work, may not be in the saved file.

## Some actions do not start

A message such as "Finish what you are doing first." means a stroke or an adjustment is still open. Finish it, or cancel it, and try again. A transform must be finished before you use the clipboard.

## A 3D model does not open

If a model fails its checks, a sheet lists the problems. Warnings let the model open and show a banner in the 3D view.

## Canvas size

A new canvas can be at most 8192 pixels on each side and 64 megapixels in area. Larger sizes are refused with a message.

## Undo does not go back far enough

The undo history keeps a limited number of steps and a limited amount of memory. Once it is full, the oldest steps are dropped. Save a version of the document when you reach a point you want to keep.

## MP4 export

The 0.1 preview builds do not include an MP4 encoder. The MP4 and timelapse export rows are grayed out and say "MP4 export is not available in this build. Export a GIF, APNG or PNG sequence instead." Nothing is wrong with your document or your computer. Export an animated GIF, an APNG or a PNG sequence.
