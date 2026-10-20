Eskiz 0.1.0 for Windows

1. Extract the whole zip first (right-click the zip, Extract All...). Do not run eskiz.exe from
   inside the zip window.
2. Double-click eskiz.exe. If Windows says "Windows protected your PC" (SmartScreen), click
   "More info", then "Run anyway". The download is not code-signed.
   If nothing appears at all, Smart App Control (Windows 11) may be blocking it. It offers no
   "Run anyway": turn it off in Windows Security > App & browser control > Smart App Control
   settings (it cannot be turned back on without resetting Windows).
3. If nothing appears, or a message box names a problem, look in
   %LOCALAPPDATA%\Eskiz\logs  (paste that into the Explorer address bar).
   startup.log lists what happened; panic-*.txt has the details of a crash. Please send them.
4. In a terminal, eskiz.exe --version, --selftest and --diagnose print their result there.

If the window freezes ("Not Responding"), or the picture never appears
-------------------------------------------------------------------
Some graphics drivers hang. Eskiz watches itself: if the window does not answer for 3 seconds it
writes a STALL line (with the phase it was in) to startup.log and leaves a stalled.marker. Close
the frozen window (End task) and start Eskiz again: it skips the graphics renderer that froze and
shows a note saying so ("safe graphics mode"). The order is: DirectX 12, Vulkan, OpenGL,
Software. Eskiz stays on the safer renderer until you click "Try normal graphics" (in the note,
or in Settings > Graphics). A crash while starting is handled the same way.

You can also choose the renderer yourself, three ways:

  a) Double-click "Eskiz (compatibility mode).bat" in this folder. It starts eskiz.exe
     --renderer gl: OpenGL and the processor instead of the graphics card. Slower, but it works on
     nearly every PC.
  b) From a terminal:  eskiz.exe --renderer gl     (or dx12, vulkan, software, auto)
     "auto" tries everything again and forgets earlier stalls. ESKIZ_BACKEND=gl does the same.
     "software" needs no graphics driver at all (slowest).
  c) Inside Eskiz: Settings > Graphics: Automatic, DirectX 12, Vulkan, OpenGL (compatibility) or
     Software. It applies the next time Eskiz starts.

Please send %LOCALAPPDATA%\Eskiz\logs\startup.log (and startup.prev.log) with a bug report.
