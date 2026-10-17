# Color

Tap the **color circle** at the right of the top bar to open the Color panel. Drag the circle onto the canvas to fill with it (see Pour Fill in Painting).

## The panel

The panel has four tabs:

- **Wheel** is the default picker. The outer ring is the hue. The thin ring inside it sets the brightness. The inner disc sets saturation and brightness together.
- **Square** has sliders and a square for the color's components, and a **Hex** field for typing a color such as `#3a7bd5`.
- **Swatches** holds your swatches (see below).
- **Schemes** picks colors that go with the current one: **Analogous**, **Complementary**, **Split**, **Triadic** and **Tetradic**.

Under the tabs, **History** keeps the last 10 colors you used. Tap one to use it again.

## Swatches

On the **Swatches** tab you can:

- **Import** a palette from a `.swatches`, `.aco` or `.ase` file;
- **Export** the current palette to one of those formats;
- **New from canvas** to take the colors that are in the picture.

Tap a swatch to use it. The panel tells you when a color is added to the palette, or is already in it.

## Color spaces

Each canvas has one color space, chosen when it is made:

- **sRGB** is the usual space for the web and most screens.
- **Display P3** is a wider space, with more vivid greens and reds on displays that can show them.

To change a canvas later, use Actions > Canvas > **Convert to Display P3** or **Convert to sRGB**. Every color is re-encoded into the new space, and the change is one undo step. Masks are not colors and are left alone.

Eskiz shows the picture in sRGB, and the operating system then matches that to your display. Exports carry the color profile of the canvas's color space, so a Display P3 picture is labeled as one. Exported pixels are not converted.
