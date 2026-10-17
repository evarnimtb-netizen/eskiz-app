# Import and export

## Importing

| What | How | Result |
|---|---|---|
| PNG, JPEG or WebP picture | Drop it on the window, or Actions > Add > Import | A new layer. Pictures larger than the canvas are scaled to fit. |
| PSD or `.procreate` file | Drop it on the Gallery, or Actions > Add > Import | Its layers, with groups flattened into one list |
| `.glb`, `.gltf` or `.obj` model | Drop it on the Gallery | A 3D document (see 3D painting) |
| `.brush`, `.brushset` or `.abr` | Drop it on the window, or the Brush Library's **+** | Brushes in the Imported set |
| `.ttf`, `.otf` or `.ttc` font | Actions > Add > **Import font** | A font for text layers |
| `.swatches`, `.aco` or `.ase` palette | The Swatches tab of the Color panel | A palette |

Pictures and layered files are added to the canvas you have open. To open a `.procreate` file in its own canvas, use Actions > Add > **Open a .procreate file**. That replaces the current drawing, so save first if you want to keep it.

If a file cannot be read, a message says so, for example "Could not import" or "No brush could be imported from that file."

## Exporting

Open Actions > **Share**. Choose a format from the list. Each one writes a file next to the name you typed in **Save to**. The default folder is your home folder, in an `eskiz_export` name.

| Format | What you get |
|---|---|
| **Layered .procreate** | The whole document with every layer, for Eskiz and other compatible apps. |
| **Photoshop PSD** | Every layer kept. |
| **PDF, flattened** | One page with the picture. Turn on **PDF with one page per layer** to get one page per layer. |
| **PNG**, **JPEG**, **TIFF**, **WebP** | One flattened picture. |
| **Each layer as a PNG in a folder** | One PNG per layer. |

The options above the list:

- **Scale** in percent, and **DPI** for the file's resolution.
- **Transparent background** keeps the alpha channel in PNG, TIFF, WebP and PDF. JPEG cannot hold transparency, so it is flattened onto the paper color.
- **JPEG quality** from 0 to 100.

Animations are exported from the same Share list (see Animation and timelapse). 3D documents show their own list with GLB and Texture PNGs (see 3D painting).

Export runs in the background, and the panel shows "Writing" while it works. Every format that can hold a color profile carries the profile of the canvas's color space (see Color).
