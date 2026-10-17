# Getting started

Eskiz is a painting app for touch, pen and mouse. This chapter covers the Gallery, making a new canvas and the canvas screen. The other chapters are listed on the left.

## The Gallery

The Gallery shows your artwork as cards with a thumbnail, a title and a size. Tap a card to open it. Tap **+** to start a new canvas.

Files can come in from the Gallery's **Import** button, or you can drop them onto the window:

- pictures (PNG, JPEG or WebP) and layered files (PSD or `.procreate`), which open as new artwork;
- 3D models (`.glb`, `.gltf` or `.obj`), which open for painting in 3D (see 3D painting).

With the Gallery open, **Select** lets you pick several cards at once. **Sort** orders the cards. Artwork can be grouped into a **stack**: choose **Make a stack**, then **Add to stack** for each card. **Delete stack** removes the stack and every artwork in it, and this cannot be undone.

## New canvas

Tap **+** in the Gallery and choose a size:

| Size | Pixels |
|---|---|
| Screen size | The size of your window |
| Square 2048 | 2048 x 2048 |
| 4K | 3840 x 2160 |
| A4 300 dpi | 2480 x 3508 |

Or type a **custom** width and height. Each side must be a whole number from 1 to 8192, and the area can be at most 64 megapixels.

Then choose:

- the **color space**, sRGB or Display P3 (see Color);
- the **color depth**, 8-bit or 16-bit per channel. 8-bit is the usual choice. 16-bit keeps smooth gradients free of banding, at twice the memory.

Tap **Create**. Canvases with no name are called Untitled, Untitled 2, and so on.

You can change these later from Actions > Canvas: **Convert to Display P3** (or sRGB), and **Convert to 16-bit** (or 8-bit). Converting down to 8-bit dithers the pixels so gradients do not band. Each conversion is one undo step.

## The canvas screen

The top bar runs along the top of the window.

- **Left side:** **Gallery** returns to the Gallery. **Actions** opens the Actions panel. **Adjustments** opens the list of adjustments. **Selection** turns Select mode on. **Transform** starts a Transform on the active layer.
- **Right side**, from left to right: **Brush**, **Smudge** and **Eraser** choose the tool. **Layers** opens the Layers panel. The **color circle** opens the Color panel, and dragging it onto the canvas fills a region (Pour Fill).

Tap a tool that is already active, or long-press any tool, to open the Brush Library for it.

The **sidebar** is the floating capsule at the edge of the canvas. From top to bottom it holds the size slider, the **Pick Color** button (hold it to pick a color with the eyedropper), the opacity slider, undo and redo. Drag its grip to move it. The Right-hand interface preference puts it on the right edge.

To hide the interface and see more of the canvas, press **F**, or tap with four fingers. Do the same again to bring it back.

Short messages appear at the bottom of the screen to confirm actions, such as "Undo Stroke", "Fit to screen" or the brush size.

## Welcome tour

The first time you start Eskiz, a short tour opens over the Gallery. It offers a sample canvas called "Sample: first steps" and then three hints on the canvas: paint a stroke, undo it, and change the brush size. Each hint waits for you to do the action, and every step can be skipped with its **Skip** button or the Esc key. The sample is an ordinary artwork, so you can delete it from the Gallery like any other.

To see the tour again, open **Actions**, then **Help**, then **Welcome tour**. The **Settings** tab also has **Show welcome again on next launch**.

## Saving

Eskiz saves your work for you. It saves when you leave the canvas, when the app closes, and every 30 seconds while there are unsaved changes. Each save is written to a temporary file first and then put in place, so an interrupted save does not damage the last good copy. See Troubleshooting if work goes missing.
