# Selection and transform

## Selection

Tap **Selection** in the top bar to switch Select mode on. A toolbar appears along the bottom edge. Choose a shape:

- **Automatic**: a magic wand. Tap an area to select the connected pixels of similar color. A banner lets you set the threshold.
- **Freehand**: draw around the area.
- **Polygon**: tap the corners.
- **Rectangle** and **Ellipse**: drag a box.

Each new selection replaces the old one. The **Add** and **Subtract** buttons combine the next selection with the current one instead. **Feather** softens the edge of the selection, up to 100 px. **Invert** selects everything outside the current selection. **Clear** removes the selection itself; the pixels stay where they are.

Use **Save** to keep a selection with the document and **Load** to bring it back. Saved selections survive a save and reload.

## Clipboard

- **Copy** copies the active layer's pixels inside the selection. With no selection it copies the whole layer.
- **Copy All** copies the whole picture as you see it, all layers together.
- **Cut** moves the selected pixels to the clipboard and removes them from the layer.
- **Paste** puts the copied pixels on a new layer and opens a Transform so you can place them.
- **Duplicate** copies the selection onto a new layer, without opening a Transform.

Cmd+C, Cmd+X and Cmd+V (Ctrl on Windows and Linux) do the same as Copy, Cut and Paste. The copy shortcut does not need the Selection toolbar open.

Copy, cut, paste and duplicate are each one undo step. You cannot use the clipboard while a transform is open.

## Transform

Tap **Transform** in the top bar to start a transform of the active layer. A box with handles appears. Choose a mode:

- **Freeform**: move, scale and rotate freely.
- **Uniform**: scale and rotate, keeping the proportions.
- **Distort**: move each corner on its own.
- **Warp**: drag control points of a grid to bend the pixels.

Other actions in the toolbar: **Flip H** and **Flip V** mirror the layer, **Rotate 45°** turns it, and **Fit** scales it to the canvas. **Snap 15°** makes rotation jump in 15-degree steps. The **Nearest**, **Bilinear** and **Bicubic** buttons choose how pixels are resampled when the layer moves or scales: Nearest keeps hard edges, and Bicubic is the smoothest.

To finish, tap **Transform** again, press **Enter**, or tap **Apply**. **Cancel** drops the changes. A transform is one undo step, named **Transform**.
