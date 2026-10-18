# Troubleshooting

## Linux: the window does not open

On Linux, Eskiz checks for the system libraries its window needs before it opens. If one is missing, it prints the names of the packages to install and exits with status 2. On Debian and Ubuntu the packages are, for example:

    sudo apt install libxkbcommon-x11-0 libxkbcommon0 libx11-xcb1 libwayland-client0

Eskiz also needs OpenGL and Vulkan drivers to draw the window: `libgl1 libvulkan1 libgl1-mesa-dri mesa-vulkan-drivers` on Debian and Ubuntu, and `mesa-libGL vulkan-loader mesa-dri-drivers mesa-vulkan-drivers` on Fedora. Install the packages the message names, then start Eskiz again.

## The window opens, but nothing draws, or the graphics fail

Eskiz draws with the graphics card through wgpu (Metal, DirectX 12 or Vulkan). If that fails or panics while the window opens, Eskiz retries once with OpenGL, and it logs the adapter and backend it chose (see the Windows section above for where the log is). You can choose the path yourself with the **ESKIZ_RENDERER** environment variable:

- `gpu`, or not set: the default, with the GPU compositor.
- `cpu`: the same window, with the compositor on the processor. Use this if the picture looks wrong.
- `gl`: an OpenGL window, with the compositor on the processor.

Values are not case-sensitive. An unknown value means the default.

## The window freezes ("Not Responding")

Some graphics drivers stop answering. Eskiz limits every wait on the graphics card to under two seconds; if one runs out, it switches to the processor for the rest of the session and shows "Graphics driver is slow; switched to compatible mode". A watchdog also notices a window that does not finish a frame for 3 seconds: it writes a `STALL:` line to `startup.log` (with the phase it was in) and leaves a `stalled.marker` file. Close the frozen window (Task Manager > End task) and start Eskiz again: it skips the renderer that froze and tells you so. The order is automatic, DirectX 12, Vulkan, then OpenGL.

You can pick the renderer yourself:

- Start `eskiz.exe --renderer gl` (also `dx12`, `vulkan` or `auto`; `auto` forgets earlier freezes). On Windows, the zip has **Eskiz (compatibility mode).bat**, which does this for you.
- Or open Actions, then **Settings**, then **Graphics**, and turn on **Compatibility graphics (OpenGL)**. It applies the next time Eskiz starts.
- `ESKIZ_RENDERER=gl` does the same as `--renderer gl`; the command line wins.

Compatibility mode is slower on large canvases, but works on nearly every PC. Please send `startup.log` when you report a freeze.

## Windows: double-clicking eskiz.exe does nothing

Work through these in order.

1. **Extract the zip first.** Right-click the `.zip`, choose **Extract All**, and run `eskiz.exe` from the new folder. Running it from inside the zip window does not work.
2. **SmartScreen.** The download is not code-signed, so Windows may show "Windows protected your PC". Click **More info**, then **Run anyway**.
3. **Read the log.** Open `%LOCALAPPDATA%\Eskiz\logs` (paste it into the Explorer address bar). `startup.log` lists each start-up step, the graphics adapter and backend that were chosen, and "first frames drawn" when the window came up. `startup.prev.log` is the run before. If Eskiz crashed, `panic-<number>.txt` holds the message, the place in the code and a backtrace. When the start fails, Eskiz also shows a message box with the reason and this folder.
4. **Graphics.** If the previous start never drew its first frames (a graphics driver can end the process without any message), Eskiz notices the next time and tries a safer renderer: first DirectX 12 only, then Vulkan only, then OpenGL. Check `startup.log` for lines starting `renderer attempt`. You can also force it: set `ESKIZ_RENDERER=gl` (OpenGL) or `ESKIZ_RENDERER=cpu`. Updating the graphics driver usually fixes it for good.
5. **Terminal output.** Start `eskiz.exe --version`, `--selftest` or `--diagnose` from PowerShell or Command Prompt; the result prints in that window. Please send the `--diagnose` output and the files from the logs folder when you report a problem.

Eskiz is built with the C runtime inside, so the Visual C++ Redistributable is not needed.

## A pen tablet has no pressure (macOS)

On macOS, Eskiz reads tablet events directly: a Wacom-style tablet, or an iPad with an Apple Pencil used through Sidecar, gives pressure, tilt and barrel rotation, and the eraser end erases. A mouse or trackpad is not changed.

To check what the tablet sends, open Actions, then **Settings**, and look at the pressure curve at the bottom. Hover the pen over the window and press: the dot on the curve moves with the pressure and the line under the curve reads `Live: pen end, touching, pressure=0.62 altitude=.. azimuth=.. roll=..`. A dash means the device reports no such value. The status line next to Reset says "Pen detected (macOS tablet events)" once a pen has been seen.

If there is still no pressure, start Eskiz from a terminal with **ESKIZ_TRACE_STROKES=1**. Every pen stroke then prints `eskiz-stroke: pen pressure=...` lines. If they show `pressure=-` or the lines never appear, the tablet driver (or Sidecar) is sending plain mouse events; send us the lines. Windows uses Windows Ink and Wayland the tablet protocol; X11 has no pen support yet.

## Where the library is kept

Your drawings are kept in a library folder in your user data folder for Eskiz. To use another folder, set **ESKIZ_LIBRARY_DIR** to its path before starting Eskiz.

## A stroke disappeared

A stroke that has put ink on the canvas is never thrown away by Eskiz: it stays until you undo it. While you draw, trackpad pinch and scroll, a second contact on the trackpad, a window switch, a save or leaving the canvas do not cancel the stroke. If the pen-up is lost (for example the window loses focus mid-stroke) the stroke is finished where it was and is one undo step. Hold to Shape replaces a stroke on purpose; turn it off in Preferences if you pause a lot while drawing.

If you still see a stroke vanish, start Eskiz from a terminal with **ESKIZ_TRACE_STROKES=1**. Eskiz then writes one line to the terminal for each stroke that begins, is committed or is cancelled, with the reason (for example `commit reason=pointer released`, `commit reason=window lost focus`, `cancel reason=hold to shape replaces the stroke`), and a line for each gesture it ignored because a stroke was open. Attach the lines around the moment it happened to your bug report. The trace costs nothing when the variable is not set.

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
