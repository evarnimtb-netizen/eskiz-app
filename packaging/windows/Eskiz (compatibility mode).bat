@echo off
rem Starts Eskiz with the OpenGL renderer, the CPU compositor and no pen tablet input. Use it if the normal start
rem freezes ("Not Responding") or shows a black window. Same as: eskiz.exe --renderer gl --no-tablet
start "" "%~dp0eskiz.exe" --renderer gl --no-tablet %*
