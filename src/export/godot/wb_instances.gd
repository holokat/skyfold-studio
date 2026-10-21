@tool
extends Node3D
## Skyfold Studio trees, plants, rocks and props: one MultiMeshInstance3D per model, variant, level
## of detail and map chunk, switched by visibility ranges (fading), plus colliders. Transforms come
## pre-packed per chunk in Godot's MultiMesh buffer layout (instances/<model>.bin); everything is
## built as internal children when the scene loads, so nothing large is saved into the scene.
## Colliders come from each variant's collision proxy (`<variant>_COL`: a convex hull, or a walk-in
## model's simplified shape) with a physics material from its footstep surface.

@export_file("*.json") var index_path := "res://world_builder/instances/instances.json"
@export var colliders := true


func _ready() -> void:
	for c in get_children(true):
		if c.has_meta("wb_built"):
			c.free()
	var index = JSON.parse_string(FileAccess.get_file_as_string(index_path))
	if typeof(index) != TYPE_DICTIONARY:
		push_error("Skyfold Studio: can't read " + index_path)
		return
	for m in index["models"]:
		_build_model(m)


## Mesh parts (mesh, transform relative to the model root) of a node and its children.
func _parts(node: Node, root: Node) -> Array:
	var out := []
	var nodes := [node] + node.find_children("*", "MeshInstance3D", true, false)
	for n in nodes:
		if n is MeshInstance3D and n.mesh != null:
			var t := Transform3D.IDENTITY
			var p: Node = n
			while p != root and p != null:
				t = (p as Node3D).transform * t
				p = p.get_parent()
			out.append([n.mesh, t])
	return out


func _build_model(m: Dictionary) -> void:
	var scene := load(m["glb"]) as PackedScene
	if scene == null:
		push_error("Skyfold Studio: can't load " + str(m["glb"]))
		return
	var root := scene.instantiate()
	# variant -> LOD -> parts
	var lods := []
	# collision proxy parts per variant ([] = use the render meshes)
	var proxies := []
	for v in m["variants"]:
		var col := str(v.get("collision", ""))
		var cn := root.find_child(col, true, false) if col != "" else null
		proxies.append(_parts(cn, root) if cn != null else [])
		var per_lod := []
		if str(v["node"]) == "":
			per_lod.append(_parts(root, root))
		else:
			for lod_name in v["lods"]:
				var n := root.find_child(lod_name, true, false)
				per_lod.append(_parts(n, root) if n != null else [])
		lods.append(per_lod)
	var holder := Node3D.new()
	holder.name = str(m["name"]).validate_node_name()
	holder.set_meta("wb_built", true)
	add_child(holder, false, Node.INTERNAL_MODE_BACK)
	var file := FileAccess.open(m["bin"], FileAccess.READ)
	if file == null:
		push_error("Skyfold Studio: can't open " + str(m["bin"]))
		root.free()
		return
	var body: StaticBody3D = null
	var collider := str(m["collider"]) if colliders else "none"
	if collider != "none":
		body = StaticBody3D.new()
		body.name = "Colliders"
		# layer 2: objects (the terrain is layer 1)
		body.collision_layer = 2
		if m.has("physics"):
			var pm := PhysicsMaterial.new()
			pm.friction = float(m["physics"]["friction"])
			pm.bounce = float(m["physics"]["bounciness"])
			body.physics_material_override = pm
			body.set_meta("wb_surface", str(m["physics"]["surface"]))
		holder.add_child(body)
	var convex := {}
	# coarse blocks (the far LOD) by variant and cell: the parents of the fine blocks inside them
	var coarse := {}
	for b in m["blocks"]:
		var variant := int(b["variant"])
		var count := int(b["count"])
		file.seek(int(b["offset"]))
		var data := file.get_buffer(count * 48).to_float32_array()
		var center := Vector3(b["center"][0], b["center"][1], b["center"][2])
		var per_lod: Array = lods[min(variant, lods.size() - 1)]
		var parent: Node = null
		if b.has("parent"):
			parent = coarse.get("%d,%d,%d" % [variant, int(b["parent"][0]), int(b["parent"][1])])
		var first: MultiMeshInstance3D = null
		for d in b["draw"]:
			var lod := mini(int(d[0]), per_lod.size() - 1)
			var b0: float = d[1]
			var e0: float = d[2]
			if e0 > 0.0 and e0 <= b0:
				continue
			for part in per_lod[lod]:
				var mm := MultiMesh.new()
				mm.transform_format = MultiMesh.TRANSFORM_3D
				mm.mesh = part[0]
				mm.instance_count = count
				var local: Transform3D = part[1]
				if local.is_equal_approx(Transform3D.IDENTITY):
					mm.buffer = data
				else:
					for i in count:
						mm.set_instance_transform(i, _instance(data, i) * local)
				var mmi := MultiMeshInstance3D.new()
				mmi.multimesh = mm
				mmi.position = center
				mmi.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_ON if bool(m["shadows"]) and lod < 2 else GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
				mmi.visibility_range_begin = b0
				mmi.visibility_range_end = e0
				mmi.visibility_range_begin_margin = b0 * 0.08 if b0 > 0.0 else 0.0
				mmi.visibility_range_end_margin = e0 * 0.08
				mmi.visibility_range_fade_mode = GeometryInstance3D.VISIBILITY_RANGE_FADE_DEPENDENCIES if b.has("cell") else GeometryInstance3D.VISIBILITY_RANGE_FADE_SELF
				holder.add_child(mmi)
				if parent != null:
					mmi.visibility_parent = mmi.get_path_to(parent)
				if first == null:
					first = mmi
		if b.has("cell"):
			if first != null:
				coarse["%d,%d,%d" % [variant, int(b["cell"][0]), int(b["cell"][1])]] = first
		elif body != null:
			_colliders(body, collider, m, variant, per_lod, proxies[min(variant, proxies.size() - 1)], data, count, center, convex)
	file.close()
	root.free()


## Instance i of a packed buffer (rows of the basis, then the origin per row).
func _instance(d: PackedFloat32Array, i: int) -> Transform3D:
	var o := i * 12
	var basis := Basis(Vector3(d[o], d[o + 4], d[o + 8]), Vector3(d[o + 1], d[o + 5], d[o + 9]), Vector3(d[o + 2], d[o + 6], d[o + 10]))
	return Transform3D(basis, Vector3(d[o + 3], d[o + 7], d[o + 11]))


func _colliders(body: StaticBody3D, kind: String, m: Dictionary, variant: int, per_lod: Array, proxy: Array, data: PackedFloat32Array, count: int, center: Vector3, convex: Dictionary) -> void:
	var v: Dictionary = m["variants"][min(variant, m["variants"].size() - 1)]
	for i in count:
		var t := _instance(data, i)
		t.origin += center
		var owner_id := body.create_shape_owner(body)
		if kind == "capsule":
			var s := t.basis.get_scale().x
			var cap := CapsuleShape3D.new()
			cap.radius = max(float(v["trunk"]) * s, 0.05)
			cap.height = max(min(float(v["height"]) * 0.6, 8.0) * s, cap.radius * 2.0 + 0.1)
			body.shape_owner_add_shape(owner_id, cap)
			body.shape_owner_set_transform(owner_id, Transform3D(Basis.IDENTITY, t.origin + Vector3(0.0, cap.height * 0.5, 0.0)))
		elif kind == "convex":
			var key := variant
			if not convex.has(key):
				var shapes := []
				for part in (proxy if not proxy.is_empty() else per_lod[per_lod.size() - 1]):
					shapes.append([part[0].create_convex_shape(true, proxy.is_empty()), part[1]])
				convex[key] = shapes
			for s in convex[key]:
				body.shape_owner_add_shape(owner_id, s[0])
			body.shape_owner_set_transform(owner_id, t * (convex[key][0][1] if convex[key].size() > 0 else Transform3D.IDENTITY))
		else:
			var key := "mesh%d" % variant
			if not convex.has(key):
				var shapes := []
				for part in (proxy if not proxy.is_empty() else per_lod[0]):
					shapes.append([part[0].create_trimesh_shape(), part[1]])
				convex[key] = shapes
			for s in convex[key]:
				var o2 := body.create_shape_owner(body)
				body.shape_owner_add_shape(o2, s[0])
				body.shape_owner_set_transform(o2, t * s[1])
			body.remove_shape_owner(owner_id)
