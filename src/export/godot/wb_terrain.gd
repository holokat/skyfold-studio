@tool
extends Node3D
## Skyfold Studio terrain: per tile (one up to 4097 samples, 2049-sample tiles beyond), the chunk
## meshes of terrain_<x>_<z>.glb wearing the tile's splat material, plus a HeightMapShape3D
## collider. Built as internal children when the scene loads (in the editor too), so the scene
## file stays small and they are never saved into it. Each chunk has levels of detail
## (`chunk_XX_ZZ_LODk`, skirted so neighbours at different levels show no cracks) switched by
## visibility ranges from the tile's terrain_lods_<x>_<z>.json.

@export_file("*.json") var tiles_path := "res://world_builder/terrain/tiles.json"
@export var collision := true


func _ready() -> void:
	for c in get_children(true):
		if c.has_meta("wb_built"):
			c.free()
	var index = JSON.parse_string(FileAccess.get_file_as_string(tiles_path))
	if typeof(index) != TYPE_DICTIONARY:
		push_error("Skyfold Studio: can't read " + tiles_path)
		return
	var cell := float(index["cell"])
	for tile in index["tiles"]:
		_build_tile(tile, cell)


func _build_tile(tile: Dictionary, cell: float) -> void:
	var scene := load(tile["mesh"]) as PackedScene
	var material := load(tile["material"]) as Material
	if scene == null:
		push_error("Skyfold Studio: can't load " + str(tile["mesh"]))
		return
	var ranges := {}
	if FileAccess.file_exists(tile["lods"]):
		var lods = JSON.parse_string(FileAccess.get_file_as_string(tile["lods"]))
		if typeof(lods) == TYPE_DICTIONARY:
			ranges = lods.get("ranges", {})
	var src := scene.instantiate()
	for mi in src.find_children("*", "MeshInstance3D", true, false):
		var m := MeshInstance3D.new()
		m.name = mi.name
		m.mesh = mi.mesh
		m.material_override = material
		if ranges.has(str(mi.name)):
			# hard switches (no fade: the skirts hide the seams) and no margins: without a fade
			# Godot treats margins as hysteresis, and a chunk first seen inside the band has neither
			# level shown (a hole in the ground)
			var r: Array = ranges[str(mi.name)]
			m.visibility_range_begin = float(r[0])
			m.visibility_range_end = float(r[1])
		m.set_meta("wb_built", true)
		add_child(m, false, Node.INTERNAL_MODE_BACK)
	src.free()
	if collision and FileAccess.file_exists(tile["collision"]):
		var body := StaticBody3D.new()
		body.name = "TerrainCollision"
		body.set_meta("wb_built", true)
		var shape := CollisionShape3D.new()
		var hm := HeightMapShape3D.new()
		hm.map_width = int(tile["vertices"])
		hm.map_depth = int(tile["vertices"])
		# heights divided by `cell` (float32, rows north to south): the collider is scaled by `cell`
		hm.map_data = FileAccess.get_file_as_bytes(tile["collision"]).to_float32_array()
		shape.shape = hm
		# the height map is centred on its node with 1 unit between samples
		shape.scale = Vector3(cell, cell, cell)
		shape.position = Vector3(tile["center"][0], 0.0, tile["center"][1])
		body.add_child(shape)
		add_child(body, false, Node.INTERNAL_MODE_BACK)
