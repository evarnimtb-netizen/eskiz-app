# Painting

## Tools

Three tools sit in the top bar: **Brush** (B), **Smudge** (S) and **Eraser** (E). Each tool keeps its own brush, size and opacity.

## Size and opacity

The sidebar sliders set the size and opacity of the current tool. The size slider runs from 1 to 400 pixels on a logarithmic scale and shows a percentage. The opacity slider is linear. The **[** and **]** keys make the brush about 5 percent smaller or larger each time you press them.

The **Brush cursor** preference draws the outline of the brush under the pen (see Preferences).

## Brush Library

Tap the active tool again, or long-press any tool, to open the Brush Library. It lists:

- eight built-in sets: **Graphite**, **Inkwell**, **Pigment**, **Mist**, **Surfaces**, **Char**, **Washes** and **Nibs**;
- **Imported**, the brushes you brought in;
- **My Brushes**, the brushes you saved from the Brush Bench.

Tap a brush to use it. Tap it again, or tap **Edit**, to open it in the Brush Bench.

## Brush Bench

The Brush Bench shows a brush's settings, such as its tip, grain and stroke behavior, next to a drawing pad where you can try the brush.

- Built-in and imported brushes are never changed. **Save** makes your own copy in My Brushes.
- For a brush in My Brushes, **Save** writes your changes to it.
- **Duplicate** copies the brush into My Brushes and opens the copy.
- **Reset** puts the brush back as it was when you opened or last saved it.
- **Done** closes the Studio without saving.
- In My Brushes, **Delete** asks you to tap again to confirm.

Curves in the Studio are edited by dragging their points. Tap the plot to add a point, and double-tap a point to remove it.

## Importing brushes

Drop a `.brush`, `.brushset` or `.abr` file onto the window. You can also open the Brush Library, tap **+** and type the path. Imported brushes go into the Imported set. Eskiz maps the settings it supports (size, spacing, smoothing, tapers, and the tip and grain images). Other settings in a file are kept but not used yet.

## Painting

One finger paints with the current tool, unless **Disable touch painting** is on. A pen paints with its pressure. Pressure goes through the pressure curve in Preferences. A stroke appears as you draw it and becomes one undo step, named **Stroke**, when you lift the pen.

## Eyedropper

Pick a color from the canvas in any of three ways:

- hold one finger still for 0.4 seconds (the delay is set in Gesture controls, from 0.1 to 1.5 seconds);
- hold the **Pick Color** button in the sidebar for the same delay;
- on a desktop, hold **Alt** (Option on macOS) and click or drag.

A loupe appears: the color under the pointer is shown above, and the color you had before is shown below. Let go to take the new color.

## Hold to Shape

Hold the pen still for 0.65 seconds at the end of a stroke and Hold to Shape snaps the stroke to a clean shape: a line, arc, ellipse, circle, polyline, rectangle or triangle. Moving the pen while it is still down scales and rotates the shape.

After you lift the pen, **Edit Shape** shows the shape's control points. Drag a point to reshape it. Tap anywhere else to finish. The history keeps only the final shape, as one Stroke step.

The hold time is set in Gesture controls (0.2 to 1.5 seconds). Hold to Shape can be turned off in Preferences.

## Pour Fill

Drag the color circle from the top bar onto the canvas to fill a region with the current color.

1. **Carry.** The color circle follows your finger. Release over the canvas to fill the region under the pointer, at the default threshold of 50 percent. Release anywhere else to cancel.
2. **Dwell.** Rest the pointer on the canvas for 0.3 seconds to fix the drop point. The fill then previews on the canvas. Drag left or right to change the threshold: moving 2 px changes it by 1 percent, and dragging right makes the fill looser. Release to commit one undo step, named **Pour Fill**.

The fill reads the **active layer** only, so the region must be closed on the layer you are filling. On an empty layer the fill covers the whole layer.

## Smudge

The **Smudge** tool (S) picks up the color under each dab and carries it along. Its strength follows the opacity slider.

## Eraser

The **Eraser** (E) removes paint in the shape of the current brush, at the opacity you set.

## Wet mixing

Some brushes are wet. The Pigment and Washes sets include wet brushes. A wet brush picks up the color already on the canvas, mixes it with its own paint, and softens the edges of the stroke as it moves. The Smudge tool uses the same pickup and carry. Erasing never mixes.

## Undo and redo

- Two-finger tap, or Cmd+Z (Ctrl+Z on Windows and Linux), undoes.
- Three-finger tap, or Cmd+Shift+Z, redoes.
- Holding two fingers keeps undoing.

The undo history keeps a limited number of steps and a limited amount of memory. The oldest steps go first.
