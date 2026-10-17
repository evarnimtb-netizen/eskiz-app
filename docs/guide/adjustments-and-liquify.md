# Adjustments and Liquify

Tap **Adjustments** in the top bar to open the list. Choose an adjustment to start it on the active layer. The list has 16 adjustments:

Opacity, Gaussian blur, Motion blur, Perspective blur, Sharpen, Noise, Chromatic aberration, Glitch, Halftone, Bloom, Hue, saturation and brightness, Color balance, Curves, Gradient map, Recolor and Clone.

## Working in an adjustment

While an adjustment is open, a banner at the top shows its name, its parameters and the **Layer** or **Pencil** switch.

- **Layer** applies the effect to the whole layer. Drag left or right anywhere on the canvas to change the selected value.
- **Pencil** applies the effect only where you paint. Drag on the canvas to paint the area the effect applies to.

The canvas shows a live preview. Finish with **Apply**, by tapping the Adjustments button again, or by pressing **Enter**. Press **Esc** to cancel. Each applied adjustment is one undo step, named for the adjustment.

Some adjustments have extra controls:

- **Curves** has a curve editor. Click an empty spot to add a point, drag a point to move it, and right-click a point to remove it. **Reset curve** puts the curve back to a straight line.
- **Gradient map** lets you choose the gradient stops.
- **Clone** needs a source. Tap **Set source**, then drag on the canvas to paint from the source. Drag the source handle to move the source.
- **Recolor** takes the color under a tap, or the current color with **Use current color**, as the new color.

## Liquify

Choose **Liquify** from the Adjustments list to warp the active layer with strokes. The layer is shown warped as you go.

Pick a mode from the chips:

- **Push** moves the pixels along your stroke.
- **Twirl Right** and **Twirl Left** rotate the pixels around the stroke.
- **Pinch** pulls pixels toward the middle of the stroke, and **Expand** pushes them outward.
- **Crystals** is a pinch with random jitter, which breaks up the pixels.
- **Edge** gathers the pixels onto the line of the stroke.
- **Reconstruct** removes the warp where you paint, so the pixels return to their original places.

The sliders set the **Size** of the brush, the **Pressure** of the stroke, the **Distortion** (random jitter in the warp), and the **Momentum** (how far the pixels keep moving after the stroke ends; 0 by default). **Adjust** sets how strongly the warp made so far is applied, and the change is baked in when you let go of the slider. **Reset** clears the warp so far.

**Done** commits the warp as one undo step, named **Liquify**. **Cancel** discards it.
