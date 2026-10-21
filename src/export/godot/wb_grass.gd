extends Node3D
## Skyfold Studio meadow grass (in game): photoscanned grass tufts scattered around the camera from
## the world's grass map (R = density) and set on the terrain heights, rebuilt as the camera
## moves. One MultiMeshInstance3D per tuft variant; the same spot always grows the same tufts.

@export_file("*.glb") var tufts_path := "res://world_builder/terrain/grass_tufts.glb"
@export_file("*.png") var density_path := "res://world_builder/terrain/grass.png"
## Heights divided by `cell` (the terrain collider's data).
@export_file("*.f32") var heights_path := "res://world_builder/terrain/collision.f32"
@export var vertices := 1025
@export var cell := 4.0
@export var radius := 70.0
## Tufts per square metre where the grass map is full.
@export var per_m2 := 0.9

var _layers: Array[MultiMeshInstance3D] = []
var _heights := PackedFloat32Array()
var _density: Image
var _center := Vector2(INF, INF)


func _ready() -> void:
	if Engine.is_editor_hint():
		return
	var scene := load(tufts_path) as PackedScene
	var tex := load(density_path) as Texture2D
	if scene == null or tex == null or not FileAccess.file_exists(heights_path):
		return
	_density = tex.get_image()
	if _density.is_compressed():
		_density.decompress()
	_heights = FileAccess.get_file_as_bytes(heights_path).to_float32_array()
	var src := scene.instantiate()
	for mi in src.find_children("*_LOD0", "MeshInstance3D", true, false):
		var mm := MultiMesh.new()
		mm.transform_format = MultiMesh.TRANSFORM_3D
		mm.mesh = mi.mesh
		var mmi := MultiMeshInstance3D.new()
		mmi.multimesh = mm
		mmi.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
		add_child(mmi)
		_layers.append(mmi)
	src.free()


func _process(_delta: float) -> void:
	if _layers.is_empty():
		return
	var cam := get_viewport().get_camera_3d()
	if cam == null:
		return
	var p := Vector2(cam.global_position.x, cam.global_position.z)
	if p.distance_to(_center) < radius * 0.2:
		return
	_center = p.snapped(Vector2(4.0, 4.0))
	_rebuild()


func _height(x: float, z: float) -> float:
	var half := (vertices - 1) * cell * 0.5
	var gx := clampf((x + half) / cell, 0.0, vertices - 1.001)
	var gz := clampf((z + half) / cell, 0.0, vertices - 1.001)
	var i := int(gx)
	var j := int(gz)
	var fx := gx - i
	var fz := gz - j
	var a := lerpf(_heights[j * vertices + i], _heights[j * vertices + i + 1], fx)
	var b := lerpf(_heights[(j + 1) * vertices + i], _heights[(j + 1) * vertices + i + 1], fx)
	return lerpf(a, b, fz) * cell


func _rebuild() -> void:
	var half := (vertices - 1) * cell * 0.5
	var w := _density.get_width()
	var step := 2.0
	var layers := _layers.size()
	var all := PackedFloat32Array()
	var which := PackedInt32Array()
	var n := int(radius / step)
	for cz in range(-n, n + 1):
		for cx in range(-n, n + 1):
			var c := _center + Vector2(cx, cz) * step
			if c.distance_to(_center) > radius:
				continue
			var u := clampi(int((c.x + half) / (half * 2.0) * (w - 1)), 0, w - 1)
			var v := clampi(int((c.y + half) / (half * 2.0) * (w - 1)), 0, w - 1)
			var d := _density.get_pixel(u, v).r
			if d < 0.03:
				continue
			# a stable hash per cell: the same spot always grows the same tufts
			var h := hash(Vector2i(int(floor(c.x / step)), int(floor(c.y / step))))
			var count := d * per_m2 * step * step
			var k := 0
			while k < count:
				h = hash(h)
				if float((h >> 20) & 1023) / 1023.0 > count - k:
					break
				var fx := float(h & 1023) / 1023.0
				var fz := float((h >> 10) & 1023) / 1023.0
				var pos := Vector3(c.x + (fx - 0.5) * step, 0.0, c.y + (fz - 0.5) * step)
				pos.y = _height(pos.x, pos.z) - 0.05
				var s := 0.75 + float((h >> 5) & 255) / 255.0 * 0.55
				var b := Basis(Vector3.UP, fx * TAU).scaled(Vector3(s, s, s))
				all.append_array([b.x.x, b.y.x, b.z.x, pos.x, b.x.y, b.y.y, b.z.y, pos.y, b.x.z, b.y.z, b.z.z, pos.z])
				which.append(h % layers)
				k += 1
	for l in layers:
		var buf := PackedFloat32Array()
		for t in which.size():
			if which[t] == l:
				buf.append_array(all.slice(t * 12, t * 12 + 12))
		var mm := _layers[l].multimesh
		mm.instance_count = 0
		mm.instance_count = buf.size() / 12
		if mm.instance_count > 0:
			mm.buffer = buf
