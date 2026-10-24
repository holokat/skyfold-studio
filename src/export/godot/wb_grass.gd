extends Node3D
## Skyfold Studio meadow grass (in game), as the editor draws it (`grass.rs`): one shared patch of
## blade clumps per 8 m cell round the camera, at three levels of detail by distance (near, mid,
## far), placed and shaped in `wb_grass.gdshader` (a port of the editor's grass shader) from the
## world's grass map (density, dryness, tint, height) and the terrain heights. The wind and its
## travelling gusts come from the Atmosphere node.

@export_file("*.json") var data_path := "res://world_builder/grass/grass.json"

const CELL := 8.0

var _d: Dictionary
var _mat: ShaderMaterial
var _layers: Array[MultiMeshInstance3D] = []
var _heights := PackedFloat32Array()
var _hn := 0
var _hcell := 1.0
var _density: Image
var _dn := 0
var _dcell := 1.0
var _half := 0.0
var _ranges := [16.0, 40.0, 75.0]
var _cells := {}
var _last := Vector3(INF, INF, INF)
var _clock := 0.0
var _atm: Node


func _ready() -> void:
	if Engine.is_editor_hint():
		return
	var d = JSON.parse_string(FileAccess.get_file_as_string(data_path))
	if typeof(d) != TYPE_DICTIONARY:
		push_error("Skyfold Studio: can't read " + data_path)
		return
	_d = d
	_half = float(_d["half"])
	_hn = int(_d["height_vertices"])
	_hcell = float(_d["height_cell"])
	_heights = FileAccess.get_file_as_bytes(_d["heights"]).to_float32_array()
	var htex := ImageTexture.create_from_image(Image.create_from_data(_hn, _hn, false, Image.FORMAT_RF, _heights.to_byte_array()))
	var dtex := load(_d["density"]) as Texture2D
	_density = dtex.get_image()
	if _density.is_compressed():
		_density.decompress()
	_dn = _density.get_width()
	_dcell = _half * 2.0 / float(_dn - 1)
	_ranges = _d.get("ranges", _ranges)
	_mat = ShaderMaterial.new()
	_mat.shader = load(_d["shader"])
	_mat.set_shader_parameter("height_tex", htex)
	_mat.set_shader_parameter("density_tex", dtex)
	_mat.set_shader_parameter("grid", Vector4(_half, _hcell, float(_hn), float(_d["sea_level"])))
	_mat.set_shader_parameter("dgrid", Vector2(_dcell, float(_dn)))
	var p: Dictionary = _d["params"]
	for k in ["base_c", "tip_c", "dry_c", "cold_c", "warm_c"]:
		var c: Array = p[k]
		_mat.set_shader_parameter(k, Vector3(c[0], c[1], c[2]))
	_mat.set_shader_parameter("amount", Vector4(float(p["amount"]), float(p["realistic"]), float(p["lushness"]), 0.0))
	for path in _d["meshes"]:
		var mmi := MultiMeshInstance3D.new()
		var mm := MultiMesh.new()
		mm.transform_format = MultiMesh.TRANSFORM_3D
		mm.mesh = _mesh(path)
		mmi.multimesh = mm
		mmi.material_override = _mat
		mmi.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
		add_child(mmi)
		_layers.append(mmi)
	_atm = get_tree().get_first_node_in_group("wb_atmosphere")


## A patch mesh from its raw buffers (positions, uv = (height fraction, across), uv2 = clump slot,
## custom0 = (rnd, tier, slot rand, azimuth)).
func _mesh(path: String) -> ArrayMesh:
	var m = JSON.parse_string(FileAccess.get_file_as_string(path))
	var f := FileAccess.open(str(m["bin"]), FileAccess.READ)
	var nv := int(m["vertices"])
	var ni := int(m["indices"])
	var pos := f.get_buffer(nv * 12).to_float32_array()
	var uv := f.get_buffer(nv * 8).to_float32_array()
	var uv2 := f.get_buffer(nv * 8).to_float32_array()
	var custom := f.get_buffer(nv * 16).to_float32_array()
	var idx := f.get_buffer(ni * 4).to_int32_array()
	f.close()
	var verts := PackedVector3Array()
	var uvs := PackedVector2Array()
	var uvs2 := PackedVector2Array()
	verts.resize(nv)
	uvs.resize(nv)
	uvs2.resize(nv)
	for i in nv:
		verts[i] = Vector3(pos[i * 3], pos[i * 3 + 1], pos[i * 3 + 2])
		uvs[i] = Vector2(uv[i * 2], uv[i * 2 + 1])
		uvs2[i] = Vector2(uv2[i * 2], uv2[i * 2 + 1])
	var arrays := []
	arrays.resize(Mesh.ARRAY_MAX)
	arrays[Mesh.ARRAY_VERTEX] = verts
	arrays[Mesh.ARRAY_TEX_UV] = uvs
	arrays[Mesh.ARRAY_TEX_UV2] = uvs2
	arrays[Mesh.ARRAY_CUSTOM0] = custom
	arrays[Mesh.ARRAY_INDEX] = idx
	var mesh := ArrayMesh.new()
	mesh.add_surface_from_arrays(Mesh.PRIMITIVE_TRIANGLES, arrays, [], {}, Mesh.ARRAY_CUSTOM_RGBA_FLOAT << Mesh.ARRAY_FORMAT_CUSTOM0_SHIFT)
	return mesh


## (max density 0..255, lowest and highest ground) of a cell, worked out once.
func _cell(cx: int, cz: int) -> Vector3:
	var key := Vector2i(cx, cz)
	if _cells.has(key):
		return _cells[key]
	var x0 := cx * CELL - _half
	var z0 := cz * CELL - _half
	var dmax := 0.0
	var lo := INF
	var hi := -INF
	for j in 3:
		for i in 3:
			var x := x0 + i * CELL * 0.5
			var z := z0 + j * CELL * 0.5
			var u := clampi(roundi((x + _half) / _dcell), 0, _dn - 1)
			var v := clampi(roundi((z + _half) / _dcell), 0, _dn - 1)
			dmax = maxf(dmax, _density.get_pixel(u, v).r * 255.0)
			var hu := clampi(roundi((x + _half) / _hcell), 0, _hn - 1)
			var hv := clampi(roundi((z + _half) / _hcell), 0, _hn - 1)
			var h := _heights[hv * _hn + hu] * _hcell
			lo = minf(lo, h)
			hi = maxf(hi, h)
	var c := Vector3(dmax, lo, hi)
	_cells[key] = c
	return c


func _process(delta: float) -> void:
	if _layers.is_empty():
		return
	var cam := get_viewport().get_camera_3d()
	if cam == null:
		return
	var eye := cam.global_position
	_clock += delta
	var wdir := Vector2(1.0, 0.0)
	var wspeed := 0.0
	var goff := Vector2.ZERO
	if _atm:
		wdir = _atm.wind_dir
		wspeed = _atm.wind_speed
		goff = _atm.gust_offset
	_mat.set_shader_parameter("wind", Vector4(wdir.x, wdir.y, wspeed, _clock))
	_mat.set_shader_parameter("gust", Vector4(goff.x, goff.y, eye.x, eye.z))
	_mat.set_shader_parameter("view", Vector4(eye.y, _ranges[2] - 20.0, _ranges[2] - 2.0, float(_d["params"]["height_scale"])))
	if eye.distance_to(_last) < 1.0:
		return
	_last = eye
	_place(eye)


func _place(p: Vector3) -> void:
	var per := int(round(_half * 2.0 / CELL))
	var reach: float = _ranges[2] + CELL
	var cx0 := maxi(int(floor((p.x - reach + _half) / CELL)), 0)
	var cz0 := maxi(int(floor((p.z - reach + _half) / CELL)), 0)
	var cx1 := mini(int(ceil((p.x + reach + _half) / CELL)), per - 1)
	var cz1 := mini(int(ceil((p.z + reach + _half) / CELL)), per - 1)
	var wanted := [PackedVector3Array(), PackedVector3Array(), PackedVector3Array()]
	var ground_here := _cell(clampi(int((p.x + _half) / CELL), 0, per - 1), clampi(int((p.z + _half) / CELL), 0, per - 1)).y
	if p.y - ground_here < _ranges[2]:
		for cz in range(cz0, cz1 + 1):
			for cx in range(cx0, cx1 + 1):
				var c := _cell(cx, cz)
				if c.x < 8.0:
					continue
				var o := Vector2(cx * CELL - _half, cz * CELL - _half)
				var near := Vector2(clampf(p.x, o.x, o.x + CELL), clampf(p.z, o.y, o.y + CELL))
				var horizontal := near.distance_to(Vector2(p.x, p.z))
				var dy := absf(p.y - clampf(p.y, c.y, c.z + 1.0))
				var dist := sqrt(horizontal * horizontal + dy * dy)
				for l in 3:
					if dist < _ranges[l]:
						wanted[l].append(Vector3(o.x, 0.0, o.y))
						break
	for l in 3:
		var mm := _layers[l].multimesh
		var list: PackedVector3Array = wanted[l]
		mm.instance_count = list.size()
		for k in list.size():
			mm.set_instance_transform(k, Transform3D(Basis.IDENTITY, list[k]))
		# (the blades are placed in the shader: bounds round the whole reach)
		mm.custom_aabb = AABB(Vector3(-reach - 8.0, -1000.0, -reach - 8.0) + Vector3(p.x, 0.0, p.z), Vector3(reach * 2.0 + 16.0, 3000.0, reach * 2.0 + 16.0))
