# Panel plugin command reference

Generated from this PhotoCraft build's `extension.commands` and CLI command descriptions.
The current PhotoCraft Plugin API; **46** permitted command ids.
The running host remains authoritative. Start with the [handbook](plugin-development.md).

## How to use this reference

Declare each command id in `permissions.commands`, then return it with an object-valued
`params`. Installation rejects unsupported grants. Runtime authorization and each command's
target, lock, selection, mode and parameter checks still apply. A listed id is not a promise
that it is currently enabled. Parameter descriptions below use the engine's human-readable
notation; they are not literal JSON examples or JSON Schemas. `?` means optional, `=` marks
a documented default, and alternatives/ranges describe accepted values. JSON uses ordinary
objects/arrays/strings/numbers/booleans, without those notation operators.

The guest cannot call discovery methods, inspect arbitrary documents, retrieve command
results, interpolate earlier results into later commands, or invoke global `plugin.run`.
Use `plugin.processImage` for its own version-2 packaged native worker or WASM filters;
the host binds the invoking installation, so panel parameters do not supply `id`.
Each returned command keeps normal history behavior; batches are not atomic.

Use [`plugin.runTask`](plugin-native-tasks.md) for explicit maintenance in the same installation's
native worker without a document. Like image processing, it must be the sole command in an action.

Image processing requires the invoking plugin's compatible installed backend; a grant
alone does not supply one. See [independent image processing](plugin-image-processing.md)
and [package formats](plugin-native-packages.md). The direct CLI description may list
an explicit owner `id`; a panel's generic image request is scoped automatically.

## Command descriptions

### `edit.undo`

Undo

```text
{}
```

### `edit.redo`

Redo

```text
{}
```

### `edit.fill`

Fill…

```text
{"contents":"foreground|background|color|contentAware|pattern|history|black|gray|white"="color","color":"#rrggbb|[r,g,b,a]"=foreground (contents=color),"pattern":id|name (contents=pattern),"scale":%=100,"angle":deg,"state":index? (contents=history; default the oldest state),"colorAdaptation":bool=true (contents=contentAware),"mode":"normal|multiply|…"="normal","opacity":0..100=100,"preserveTransparency":bool=false,"target":"pixels"|"mask"|"quickMask"|{"channel":i}?}
```

### `edit.stroke`

Stroke…

```text
{"width":1..250=1,"color":"#rrggbb|[r,g,b,a]"=foreground,"location":"inside|center|outside"="center","mode":"any layer blend mode"="normal","opacity":0..100=100,"preserveTransparency":bool=false}
```

### `edit.clear`

Clear

```text
{} (a targeted channel or Quick Mask is cleared alone)
```

### `image.imageSize`

Image Size…

```text
{"width":px,"height":px,"percent":%? (both sides; an explicit side wins),"resolution":ppi,"keepPrintSize":bool=false (with only a resolution: resample the pixels to keep the print size),"resample":"automatic|preserveDetails|bicubicSmoother|bicubicSharper|bicubicAutomatic|bicubic|nearest|bilinear|lanczos|none"="bicubic"} → {width,height} (the document's size after the command; "none" changes the resolution only)
```

### `plugin.processImage`

Process Visible Image with Plugin

```text
{"id":text,"processor":text,"options":object={},"width":u32,"height":u32,"preview":bool=true} (uses this installed plugin's packaged processor; result: flattened 8-bit sRGB)
```

### `plugin.runTask`

Run Native Plugin Task

```text
{"id":text,"options":object={}} (uses this installed plugin's own native worker; no document required)
```

### `image.canvasSize`

Canvas Size…

```text
{"width":px,"height":px,"relative":bool=false,"anchor":"topLeft|top|topRight|left|center|right|bottomLeft|bottom|bottomRight"="center","extensionColor":"background|foreground|white|black|transparent|#rrggbb"="background"}
```

### `image.crop`

Crop

```text
{"x":px,"y":px,"width":px,"height":px,"angle":-3600..3600=0,"deleteCroppedPixels":bool=true,"contentAware":bool=false,"targetWidth":px,"targetHeight":px,"resolution":ppi,"resample":"bicubic|bilinear|nearest|lanczos|preserveDetails"="bicubic"} → {x,y,width,height,documentWidth,documentHeight,resolution} (targetWidth/targetHeight, both or neither: the cropped area is resampled to exactly that size, as Image Size, in the same undo step; resolution: the document's new ppi; contentAware: the Crop tool's Fill › Content-Aware Fill, the area a rotated or extending crop uncovers filled from its surroundings on the Background, else the active pixel layer)
```

### `image.trim`

Trim…

```text
{"basedOn":"transparent|topLeft|bottomRight"="transparent","top":bool=true,"bottom":bool=true,"left":bool=true,"right":bool=true}
```

### `image.duplicate`

Duplicate…

```text
{"name":str,"mergedOnly":bool=false}
```

### `image.adjustments.brightnessContrast`

Brightness/Contrast…

```text
{"brightness":-150..150=0,"contrast":-50..100=0,"legacy":bool=false}
```

### `image.adjustments.levels`

Levels…

```text
{"inBlack":0..253=0,"gamma":0.01..9.99=1,"inWhite":2..255=255,"outBlack":0..255=0,"outWhite":0..255=255,"red":json,"green":json,"blue":json,"eyedropper":json} (top level = composite; per channel {"inBlack","gamma","inWhite","outBlack","outWhite"} under red/green/blue, gray, cyan/magenta/yellow/black or lightness/a/b; eyedropper = the dialog's Set Black/Neutral Gray/White Point picker {"point":"black|gray|white","at":[x,y],"size":1..101=1} or {"point",…,"color":[r,g,b] 0..1}: black/white set each channel's input black/white to the sample, gray each channel's gamma so the sample turns neutral at its luma; RGB and Grayscale (no gray) only, applied over the given levels)
```

### `image.adjustments.curves`

Curves…

```text
{"points":json,"red":json,"green":json,"blue":json,"pencil":bool=false,"smooth":0..10=0,"eyedropper":json} (curves as [[in,out],…] in 0..255, 2..19 points: points = composite; red/green/blue, gray, cyan/magenta/yellow/black or lightness/a/b per channel; pencil = drawn curves (Draw to modify the curve): each curve is 256 output levels in 0..255 (point curves given are drawn in, and turning pencil off fits points to the drawing); smooth = Smooth passes over the drawn curves; eyedropper = the dialog's Set Black/Neutral Gray/White Point picker {"point":"black|gray|white","at":[x,y],"size":1..101=1} or {"point",…,"color":[r,g,b] 0..1}, RGB and Grayscale (no gray) only, applied over the given curves)
```

### `image.adjustments.exposure`

Exposure…

```text
{"exposure":-20..20=0,"offset":-0.5..0.5=0,"gamma":0.01..9.99=1}
```

### `image.adjustments.vibrance`

Vibrance…

```text
{"vibrance":-100..100=0,"saturation":-100..100=0}
```

### `image.adjustments.hueSaturation`

Hue/Saturation…

```text
{"hue":-180..180=0,"saturation":-100..100=0,"lightness":-100..100=0,"colorize":bool=false,"reds":json,"yellows":json,"greens":json,"cyans":json,"blues":json,"magentas":json} (per range {"hue","saturation","lightness","range":[4 hue degrees]}; colorize: hue 0..360, saturation 0..100)
```

### `image.adjustments.colorBalance`

Color Balance…

```text
{"shadows":json,"midtones":json,"highlights":json,"preserveLuminosity":bool=true} (each tone [cyan-red, magenta-green, yellow-blue] in -100..100)
```

### `image.adjustments.blackWhite`

Black & White…

```text
{"reds":-200..300=40,"yellows":-200..300=60,"greens":-200..300=40,"cyans":-200..300=60,"blues":-200..300=20,"magentas":-200..300=80,"tint":bool=false,"tintColor":"#rrggbb"}
```

### `image.adjustments.photoFilter`

Photo Filter…

```text
{"filter":"warming85|warmingLBA|warming81|cooling80|coolingLBB|cooling82|red|orange|yellow|green|cyan|blue|violet|magenta|sepia|deepRed|deepBlue|deepEmerald|deepYellow|underwater","color":"#rrggbb","density":0..100=25,"preserveLuminosity":bool=true}
```

### `image.adjustments.channelMixer`

Channel Mixer…

```text
{"red":json,"green":json,"blue":json,"gray":json,"monochrome":bool=false} (per output channel [red %, green %, blue %, constant %] in -200..200; gray = the monochrome mix)
```

### `image.adjustments.invert`

Invert

```text
{}
```

### `image.adjustments.posterize`

Posterize…

```text
{"levels":2..255=4}
```

### `image.adjustments.threshold`

Threshold…

```text
{"level":1..255=128}
```

### `image.adjustments.desaturate`

Desaturate

```text
{}
```

### `image.adjustments.equalize`

Equalize

```text
{}
```

### `layer.new.layer`

Layer…

```text
{"name":str?,"clipToPrevious":bool=false (Use Previous Layer to Create Clipping Mask; inside a clip group the layer joins it anyway),"color":"none|red|orange|yellow|green|seafoam|blue|indigo|magenta|fuchsia|violet|gray"="none","mode":"Normal|Multiply|…"="Normal","opacity":0..1=1,"fillNeutral":bool=false (fill with the mode's neutral color: modes without one are an error),"position":"above|below"="above" (below: the Layers panel's Cmd/Ctrl-click; never under the Background)}
```

### `layer.duplicate`

Duplicate Layer…

```text
{"layer":id?,"inPlace":bool=false (artboards: keep the copy on the original instead of beside it),"name":str? (one layer: the copy's name),"document":index|"new"=same document (Photoshop's Destination: "new" makes a new document the size of this canvas holding just the copies),"documentName":str? (the new document's name)} (no layer: every selected layer)
```

### `layer.delete`

Delete Layer

```text
{"layer":id?} (no layer: every selected layer)
```

### `layer.select`

Select Layer

```text
{"layer":id|str | "name":str,"occurrence":index?,"mode":"replace|toggle|range|add|remove"="replace"} (name occurrence: zero-based document walk order; required for duplicate names; toggle = ⌘-click, range = ⇧-click, remove = deselect it, keeping at least one)
```

### `layer.setProps`

Layer Properties

```text
{"layer":id?,"name":str?,"visible":bool?,"opacity":0..1?,"fill":0..1?,"blend":"Multiply|…"?,"clipped":bool?,"locked":bool?,"locks":{"transparency","pixels","position","artboard","all":bool}?,"channels":[bool,…]? (Advanced Blending: which colour channels blend, R G B / C M Y K / L a b)}
```

### `layer.flattenImage`

Flatten Image

```text
{}
```

### `layer.mergeLayers`

Merge Layers

```text
{} (one layer selected: Merge Down)
```

### `layer.mergeVisible`

Merge Visible

```text
{}
```

### `select.all`

All

```text
{}
```

### `select.deselect`

Deselect

```text
{}
```

### `select.inverse`

Inverse

```text
{}
```

### `select.modify.expand`

Expand…

```text
{"radius":1..500=1,"applyAtCanvasBounds":bool=false}
```

### `select.modify.contract`

Contract…

```text
{"radius":1..500=1,"applyAtCanvasBounds":bool=false}
```

### `select.modify.feather`

Feather…

```text
{"radius":0.1..1000=1,"applyAtCanvasBounds":bool=false}
```

### `filter.blur.gaussianBlur`

Gaussian Blur…

```text
{"radius":0.1..1000=1}
```

### `filter.blur.motionBlur`

Motion Blur…

```text
{"angle":-360..360=0,"distance":1..2000=10}
```

### `filter.noise.addNoise`

Add Noise…

```text
{"amount":0.1..400=12.5,"distribution":"uniform|gaussian","monochromatic":bool,"seed":u32=0}
```

### `filter.sharpen.unsharpMask`

Unsharp Mask…

```text
{"amount":1..500=50,"radius":0.1..1000=1,"threshold":0..255=0}
```

### `filter.other.highPass`

High Pass…

```text
{"radius":0.1..1000=10}
```

## Regenerate

From the PhotoCraft repository root, using a CLI built from the same host source:

```sh
python scripts/plugins/generate-command-reference.py --cli target/debug/photocraft-cli
```

On Windows append `.exe` to the CLI path. This script reads the full command catalog and
creates a disposable in-memory 8 × 8 document to ask for the extension allowlist. It does
not install plugins, edit user documents or attach a persistent settings store.

After regenerating, review changes against `crates/engine/src/extension_cmds.rs` and the
individual command implementations. New grants require a host API/security review.


## Host lifecycle administration

These commands are host/control operations, outside a guest's command allowlist:

| Command | Parameters and behavior |
|---|---|
| `extension.commands` | Reports current worker transports, installed-descriptor cap and scheduler limits/usage. |
| `extension.list` | Includes descriptor lifecycle and optional persistent-runtime status; does not activate dormant plugins. |
| `extension.runtime` | `{id, action:"status"}` (default), `"retry"` or `"reset"`; reset runs as a cancellable background job. |
| `extension.storage` | `{id}` reports active managed settings/data/cache/secrets-reference usage. |
| `extension.rollback` | `{id}` validates and selects the previous code/managed-state pair. |
| `extension.remove` | `{id, purgeData:false}` preserves managed state by default; explicit purge removes only safe owned generations. |

These extend the same developing API. Package, filter and transport format labels describe
different wire contracts, not additional production APIs for each implementation phase.
