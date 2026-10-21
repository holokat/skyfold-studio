# Crop texture atlas — shared spec

One atlas for every field crop: `assets/models/crops/atlas_albedo.ktx2` (BC3, sRGB colour + cut-out
alpha, full mip chain with alpha-coverage-preserving mips) and `assets/models/crops/atlas_normal.ktx2`
(BC5, tangent-space normal XY), plus `assets/models/crops/atlas.json`:

```json
{ "size": [4096, 4608], "regions": { "<name>": [u0, v0, u1, v1], ... },
  "means": { "<name>": [r, g, b, coverage], ... } }
```

`means`: per region, the 0–1 sRGB average over its opaque texels (alpha > 0.5) and the opaque
fraction of the region (for far-field colour calibration).

UVs are normalized (0..1), `v` grows **downwards** in the image (row 0 = v 0). Each region's rectangle
in the JSON is already inset by 4 texels (the painter bleeds colour into a ≥ 8 px gutter so mips
don't pick up neighbours). The game (src/fields/plants.rs) maps every plant part into one region.

Written by `tools/crops/paint.py` (python3 + numpy + Pillow; reuses `mip_chain`, `write_ktx2` from
`tools/process_photoscan.py`). It also writes `tools/crops/out/atlas_preview.png` (albedo over a grey
checker so cut-outs show) and `tools/crops/out/normal_preview.png` for review.

## Conventions

- Region cell grid: 512 px cells on the 4096 × 4608 atlas (8 columns × 9 rows; `size` = [width, height]). A region is 1 cell unless noted (2×1 =
  two cells wide, 1×2 = two cells tall, 2×2).
- **Alpha** = the part's real outline (cut-out at 0.5). Geometry is a plain strip / card / fan that
  covers the outline's envelope; the texture carves the shape (serrations, lobes, pointed tips,
  feathery fronds, awns). Opaque parts (stems, fruit, straw) are alpha 1 everywhere in the region.
- **Normal map**: computed from a painted height map, `n = normalize(-dh/du * s, -dh/dv * s, 1)`
  where `u` = image x (right) and `v` = image y (down) — i.e. +Y of the stored normal points to
  **increasing v (down the image)**. Stored as `n.xy * 0.5 + 0.5` in RG.
- **Colour** is the real, final albedo (not a tint), photographic and natural: no saturated toy
  colours; variation everywhere (mottling, blemishes, gradients, slight colour shifts). Except the
  `tulip_petal`, which is painted near-white and tinted per row by the game.
- Linear-light mips; sRGB storage.

### Strip parts (leaves, petals, ears, stalk skins)
`u` (image x, 0..1) runs **across** the part (0.5 = its midline), `v` (image y, 0..1) runs **along** it
from its **base (v = 0, top of the image) to its tip (v = 1, bottom of the image)**.

### Round parts (stalks, stems, fruit skins)
`u` = **around** the part (0..1 = 0..360°), `v` = **along** it (stalk: bottom v=0 → top v=1; fruit:
top/stem end v=0 → bottom v=1). Must tile seamlessly across `u` (left/right edges match).

### Fan leaves (palmate: pumpkin, grape, hop, cotton) and discs
Painted **top-down**: the leaf's petiole attaches at the **centre** of the region; the leaf's tip
points **up** (towards v = 0) and the petiole/basal notch points **down** (v = 1). The game maps a
fan of vertices around the attach point with `uv = 0.5 + 0.5·r·(sin a, −cos a)` (a = angle from
the tip direction, r = 0..1 to the region edge). Discs (sunflower disc, bale end): centred circle
touching the region edges.

## Regions

68 of the 72 cells are used; pack them in any order (the JSON tells the game where each is).

| name | cells | what | notes |
|---|---|---|---|
| corn_leaf | 1×2 | maize leaf blade | long strap; deep green, many fine parallel veins, pale raised midrib, wavy margin (alpha ripples), slight yellow-brown drying at tip, faint lighter sheath near base |
| corn_stalk | 1 | maize stalk skin (round) | green-yellow, fine lengthwise striations, 8 nodes along v (darker rings, a little purple-brown), leaf-sheath edges |
| corn_husk | 1 | husk leaf (strip) | pale green → straw yellow, strong parallel veins, papery, drying tips |
| corn_tassel | 1 | one tassel branch (strip) | central rachis with paired spikelets along it (alpha), straw-gold with green/purple tinge, tiny dangling yellow anthers |
| corn_silk | 1 | silk tuft (strip) | dozens of fine hairs (alpha), pale gold at base → red-brown tips |
| sunflower_petal | 1 | ray petal (strip) | narrow pointed oval, 2–3 tiny tip notches, fine lengthwise veins, golden yellow, slightly deeper orange at base |
| sunflower_disc | 2×2 | the flower head disc (disc) | ~2000 florets in golden-angle (Vogel) spirals: green-yellow tiny centre, wide band of dark brown-black seeds with highlights, outer 2–3 rings of open tubular florets yellow-orange with dark tips and pollen dots |
| sunflower_leaf | 1 | heart-shaped leaf (strip: base at v 0) | cordate, serrated margin, 3 main veins from the base + net of finer veins, dark rough green, hairy speckle |
| sunflower_bract | 1 | phyllary (strip) | lanceolate, long pointed tip, green, veined, slightly fringed edges |
| sunflower_stem | 1 | stem skin (round) | green, coarse hairs (white speckles), faint purple low |
| pumpkin_skin | 2×1 | fruit skin (round) | orange with subtle lengthwise colour bands (lighter on rib tops, darker in grooves at u multiples of 0.1 — 10 ribs), fine light speckles, a few scars, yellowish ground spot near v = 1 |
| pumpkin_stem | 1 | woody stem (round) | ridged tan-green, corky |
| pumpkin_leaf | 2×2 | palmate leaf (fan) | 5 shallow lobes, finely serrated, palmate veins from the centre, mid-green with silvery mottling between veins, rough hairy look |
| pumpkin_vine | 1 | hairy vine (round) | green with ridges and fine hairs |
| carrot_frond | 1×2 | finely divided frond (strip) | bipinnate/tripinnate feathery leaf: rachis up the middle, pinnae in alternate pairs, each cut into tiny lobes (alpha); fresh green, darker on the rachis |
| carrot_root | 1 | carrot shoulder (round) | orange with horizontal rings/ridges, a few root-hair marks, green-purple tinge at the top |
| strawberry_leaf | 1 | one leaflet (strip) | ovate, coarse serrations, deeply impressed pinnate veins, dark glossy green |
| strawberry_berry | 1 | berry skin (round) | glossy red, yellow achenes (seeds) sunk in little pits in a quincunx pattern, paler/white shoulders near v 0 on some |
| strawberry_calyx | 1 | calyx (fan) | star of ~10 pointed green sepals |
| strawberry_flower | 1 | flower (fan) | 5 rounded white petals, yellow centre |
| grape_leaf | 1 | vine leaf (fan) | 5 lobes with deep sinuses, coarse serration, palmate veins, green with yellowing edges on some |
| grape_berry | 1 | berry skin (round) | dark blue-purple with a dusty bloom (lighter, desaturated haze), tiny highlight scar |
| grape_stalk | 1 | bunch stalk (round) | green-brown thin stalk |
| vine_bark | 1 | grapevine trunk (round) | shaggy, peeling brown bark strips, lengthwise |
| wood_post | 1 | weathered post (round) | grey-brown wood grain, cracks |
| straw | 2×1 | cut hay / straw mass | dense tangle of straw stems mostly along v, varied gold/tan/grey-green, gaps darker; tiles in u and v (photoscan reed_roof_04, graded) |
| bale_end | 1 | round bale end (disc) | tightly rolled hay spiral (layers), fibres, darker crevices (the straw photo polar-warped) |
| bale_net | 1 | net wrap overlay (strip, alpha) | white plastic net mesh (thin crossing strands) on transparent |
| cereal_leaf | 1 | wheat/barley/rye leaf (strip) | narrow, golden-tan ripe (with a little green), parallel veins |
| cereal_stalk | 1 | straw stalk (round) | golden, glossy nodes |
| wheat_ear | 1 | wheat ear (strip) | two rows of plump spikelets with glumes, golden, no long awns (alpha outline) |
| barley_ear | 1 | barley ear (strip) | slender ear with very long fine awns (alpha) |
| rye_ear | 1 | rye ear (strip) | long thin ear, short awns, grey-gold |
| rice_leaf | 1 | rice leaf (strip) | bright green, parallel veins |
| rice_panicle | 1 | rice panicle (strip) | drooping branches of golden grains (alpha) |
| lavender_spike | 1 | flower spike (strip) | whorls of tiny purple-violet flowers along the top two-thirds, grey-green stalk below |
| lavender_leaf | 1 | leaf (strip) | narrow grey-green, felted |
| tulip_petal | 1 | petal (strip, near-white) | broad spoon-shaped tepal, fine veins, darker base blotch |
| tulip_leaf | 1 | leaf (strip) | broad blue-green, parallel veins |
| cabbage_leaf | 1 | outer leaf (strip) | blue-green, waxy bloom, thick white midrib + branching white veins, puckered |
| cabbage_head | 1 | head (round) | pale green wrapped leaves with arcing veins |
| potato_leaf | 1 | leaflet (strip) | oval, dark green, impressed veins |
| potato_flower | 1 | flower (fan) | 5 white/lilac petals, yellow cone centre |
| tomato_leaf | 1 | leaflet (strip) | lobed/serrated, dark green |
| tomato_fruit | 1 | skin (round) | glossy red (some orange/green shoulders) |
| hop_leaf | 1 | leaf (fan) | 3 lobes, serrated, rough dark green |
| hop_cone | 1 | cone (round) | overlapping pale green papery bracts |
| cotton_leaf | 1 | leaf (fan) | 3–5 lobes, green with red tinge |
| cotton_boll | 1 | open boll (round) | fluffy white fibre with brown bract bases |
| tea_leaf | 1 | leaf (strip) | elliptic, finely serrated, glossy dark green |
| green_stem | 1 | generic stem (round) | green with fine striations |
| field_soil | 2×2 | tilled soil (TILED, wrap gutter) | photoscan farm_soil |
| tea_canopy | 2×1 | clipped tea hedge surface (TILED in u and v, wrap gutter) | dense small elliptic glossy dark-green leaves in all orientations, pale yellow-green flush tips (~10%), deep dark gaps; opaque; strong per-leaf normals; u along the hedge (1 m per repeat), v around its cross-section (1.15 m); mean ≈ sRGB (46, 86, 38) |
| leaf_clump | 1 | filler card (cut-out, not tiled) | loose clump of ~16 overlapping hop / vine-like serrated leaves seen face-on, petioles towards the centre, ragged tips reaching the cell edges, inner leaves darker; mean ≈ sRGB (60, 104, 44) |
