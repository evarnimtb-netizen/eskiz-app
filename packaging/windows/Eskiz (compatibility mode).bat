@echo off
rem Starts Eskiz with the OpenGL renderer and the CPU compositor. Use it if the normal start
rem freezes ("Not Responding") or shows a black window. Same as: eskiz.exe --renderer gl
start "" "%~dp0eskiz.exe" --renderer gl %*
