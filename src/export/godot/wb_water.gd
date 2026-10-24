@tool
extends Node3D
## Skyfold Studio lakes, rivers and waterfalls: the surfaces of water.glb, each with its own look
## (water.gdshader parameters from water.json), and the swim volumes of lakes, river stretches and
## the ocean: Area3D boxes from the bed up to the surface (group `wb_water_volume`, metadata
## `wb_surface` / `wb_kind`; collision layer 4). Built as internal children when the scene loads.

@export_file("*.json") var data_path := "res://world_builder/water/water.json"


func _ready() -> void:
	for c in get_children(true):
		if c.has_meta("wb_built"):
			c.free()
	var data = JSON.parse_string(FileAccess.get_file_as_string(data_path))
	if typeof(data) != TYPE_DICTIONARY:
		return
	# the terrain heights for the water column under every point (global shader uniforms)
	var hg = data.get("heights")
	if hg is Dictionary and FileAccess.file_exists(hg["file"]):
		var n := int(hg["vertices"])
		var cell := float(hg["cell"])
		var h := FileAccess.get_file_as_bytes(hg["file"]).to_float32_array()
		# (stored divided by the cell)
		for i in h.size():
			h[i] *= cell
		var img := Image.create_from_data(n, n, false, Image.FORMAT_RF, h.to_byte_array())
		RenderingServer.global_shader_parameter_set("wb_heights", ImageTexture.create_from_image(img))
		RenderingServer.global_shader_parameter_set("wb_hgrid", Vector4((n - 1) * cell * 0.5, cell, float(n), 0.0))
	for v in data.get("volumes", []):
		var area := Area3D.new()
		area.name = (str(v["kind"]) + "_" + str(v["name"])).validate_node_name()
		area.collision_layer = 8
		area.collision_mask = 0
		area.monitoring = false
		area.position = Vector3(v["center"][0], v["center"][1], v["center"][2])
		area.rotation = Vector3(0.0, float(v["yaw"]), 0.0)
		var shape := CollisionShape3D.new()
		var box := BoxShape3D.new()
		box.size = Vector3(v["size"][0], v["size"][1], v["size"][2])
		shape.shape = box
		area.add_child(shape)
		area.set_meta("wb_surface", float(v["surface"]))
		area.set_meta("wb_kind", str(v["kind"]))
		area.set_meta("wb_built", true)
		area.add_to_group("wb_water_volume")
		add_child(area, false, Node.INTERNAL_MODE_BACK)
	if str(data.get("mesh", "")) == "":
		return
	var scene := load(data["mesh"]) as PackedScene
	var shader := load(data["shader"]) as Shader
	var ripples := load(data["normal_map"]) as Texture2D
	var noise := load(data["noise"]) as Texture2D if data.has("noise") else null
	var foam := load(data["foam"]) as Texture2D if data.has("foam") else null
	if scene == null or shader == null:
		push_error("Skyfold Studio: water files missing")
		return
	var src := scene.instantiate()
	for b in data["bodies"]:
		var from := src.find_child(b["node"], true, false) as MeshInstance3D
		if from == null:
			continue
		var mi := MeshInstance3D.new()
		mi.name = str(b["node"]).validate_node_name()
		mi.mesh = from.mesh
		mi.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
		var mat := ShaderMaterial.new()
		mat.shader = shader
		mat.set_shader_parameter("ripple_normal", ripples)
		mat.set_shader_parameter("noise_tex", noise)
		mat.set_shader_parameter("foam_tex", foam)
		var params: Dictionary = b["params"]
		for k in params:
			mat.set_shader_parameter(k, _value(k, params[k]))
		mi.material_override = mat
		mi.set_meta("wb_built", true)
		mi.add_to_group("wb_water")
		add_child(mi, false, Node.INTERNAL_MODE_BACK)
	src.free()


static func _value(key: String, v):
	if v is Array and v.size() == 3 and (key == "extinct" or key == "inscat"):
		return Vector3(v[0], v[1], v[2])
	if v is Array and v.size() == 3:
		return Color(v[0], v[1], v[2])
	if v is Array and v.size() == 2:
		return Vector2(v[0], v[1])
	return v
