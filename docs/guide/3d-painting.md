# 3D painting

Eskiz can paint directly on a 3D model. Open the model's card in the Gallery: import a `.glb`, `.gltf`, `.obj`, `.usdz`, `.usda` or `.usdc` file with the Gallery's **Import** button, or drop the file onto the Gallery. Eskiz reads the model in the background and checks it.

If a check fails, a sheet lists the problems and the model does not open. Warnings open the model and show a banner in the 3D view.

## The 3D view

- **Paint** with one finger or the pen, as on the flat canvas. Each stroke paints on the model's surface, as it is seen from the camera.
- **Orbit** the camera with two fingers, or with right-drag on a mouse.
- **Pan** with the middle mouse button, or with Space and drag.
- **Zoom** with a pinch, or with the scroll wheel.

The camera does not move during a stroke. While it moves, the view draws at half resolution to keep it smooth.

Undo and redo in the sidebar follow the strokes you make on the model.

## Texture sets

A model's surfaces are grouped into **texture sets**, listed under **TEXTURE SETS** in the 3D panel. Each set has its own maps:

- **Color**, the paint you put on the surface;
- **Normal**, **Metallic**, **Roughness** and **Ambient occlusion**, which the model uses for lighting;
- **Composite (derived)**, which Eskiz builds from the maps for the view.

For each set you can switch it **Visible**, **Locked** (so it cannot be painted), and change its opacity.

## Saving and exporting

A 3D document is saved in your library as a folder. It holds the model and the map images, and autosaves in the same way as a flat document.

From Actions > Share, choose **GLB** to export the model with its maps in one `.glb` file, or **Texture PNGs** to write one PNG per map per texture set into a folder.

Choose **USDZ** to export the model and its maps as a `.usdz` package that other 3D tools and viewers can open. USDZ files can also be imported (see the start of this chapter).
