You are reading a top-down level-design sheet of an open-air dungeon (an image). Turn what it shows into JSON for a 3D level builder. Measure in the image's own pixels: x from the left edge, y from the TOP edge, of the full image. Be exact and complete; when the sheet contradicts itself, say so in "notes" and give your best reading.

Read:
- "name": the title.
- "frame_px": [x0, y0, x1, y1] — the painted map's outer border (not the sidebar, not the title).
- "frame_m": [width, depth] — the numbers on the dimension lines along the map's top and left side.
- "zones": every numbered zone (red disc with 01, 02, …), in number order:
  - "num", "name" (from its caption, without the number), "size_m": [w, d] and "elevation_m" (from its caption "W × D m • Elevation +N m"),
  - "marker_px": the red disc's centre,
  - "box_px": [x0, y0, x1, y1] — the zone's floor area as drawn (its paved court, terrace or clearing, not its caption box),
  - "round": true if its floor is round,
  - "boss": null, or {"label": "Boss 1" / "Final boss", "diameter_m": from "Ø28 m", "center_px": the dashed red ring's centre, "radius_px": the ring's radius},
  - "side": true for the optional side area, "final": true for the final boss zone.
- "links": every drawn connection between two zones — the red dashed route between consecutive zones, side paths, and the grey dotted return path or shortcut:
  - "from", "to": zone numbers (in walking direction),
  - "points_px": 3–12 points along the walkway's centre, from inside the "from" zone to inside the "to" zone, following its bends,
  - "kind": "path", "bridge" (over water or a chasm), "causeway", or "return" (the grey dotted return path or shortcut),
  - "width_m": the walkway's width if it differs from about 9 m,
  - "stairs": every stair or ramp on it, in order: {"at_px": its middle, "rise_m": +4 for "↑ 4 m", −4 for "↓ 4 m" (in walking direction), "tag": its key letter if tagged}.
- "water": each lake, river, pool or sea: {"name", "level_m" (from a label like "Lake −2 m"; else your estimate), "outline_px": 6–20 points round it}.
- "chasms": each chasm, ravine, pit or drop: {"name", "floor_m" (from "Chasm −10 m"), "outline_px": 6–20 points}.
- "objects": the object key, every line: {"letter", "name", "group": "structure" or "prop", "size_m": [w, d, h], "count", "at_px": the centres of every copy of it you can see on the map (at least the tagged one; up to its count)}.
- "start_px": the player start "★"; "start_toward_px": a point it faces (into the dungeon).
- "ending": {"kind": "exit" (an "Exit portal"), "shortcut" (a "Shortcut") or "return" (a "Return gate" path), "at_px": the portal, the shortcut's head or the gate}.
- "sightline_px": [[x, y], [x, y]] — the dotted sightline's two ends (the eye icon end first), or null.
- "art": {"feel", "materials", "light", "vfx", "post"} — copied from the sidebar's art direction.
- "notes": short notes on anything missing, unreadable or contradictory (a zone without a marker, a stair whose rise disagrees with the elevations, an unreachable zone, a key letter not on the map…).

Reply with the JSON object only — no prose, no code fence.
