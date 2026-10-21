extends SceneTree
## Checks an exported Skyfold Studio project: loads world.tscn, lets it build, and prints
## `WBCHECK` lines (node, mesh, multimesh and instance counts, collision, terrain height probes
## against the exported heights). With a window it can also save screenshots, one per --shot
## (each with the --cam that follows it):
##   godot --path <project> --script res://world_builder/scripts/wb_check.gd -- [--shot out.png
##         --cam x,z,yaw,pitch,distance]... [--frames 240]
## (--cam uses Skyfold Studio's orbit camera: focus on the ground at x, z.)

var _frames := 0
var _wait := 240
var _shots := []
var _next := 0
var _since := 0
var _world: Node
var _start := 0


func _initialize() -> void:
	var args := OS.get_cmdline_user_args()
	for i in args.size():
		match args[i]:
			"--shot":
				_shots.append([args[i + 1], PackedFloat32Array()])
			"--frames":
				_wait = int(args[i + 1])
			"--cam":
				if not _shots.is_empty():
					_shots[-1][1] = PackedFloat32Array(Array(args[i + 1].split(",")).map(func(v): return float(v)))
	_start = Time.get_ticks_msec()
	var scene := load("res://world.tscn") as PackedScene
	if scene == null:
		print("WBCHECK FAIL can't load world.tscn")
		quit(1)
		return
	_world = scene.instantiate()
	root.add_child(_world)
	print("WBCHECK loaded in %d ms" % (Time.get_ticks_msec() - _start))


func _process(_delta: float) -> bool:
	_frames += 1
	if _frames == 4:
		_report()
		if not _shots.is_empty():
			_place_camera(_shots[0][1])
			_since = _frames
	if _shots.is_empty():
		return _frames >= 4
	if _frames < 4:
		return false
	if _frames - _since == _wait - 1:
		print("WBCHECK fps %.1f, draw calls %d, primitives %d" % [Performance.get_monitor(Performance.TIME_FPS), Performance.get_monitor(Performance.RENDER_TOTAL_DRAW_CALLS_IN_FRAME), Performance.get_monitor(Performance.RENDER_TOTAL_PRIMITIVES_IN_FRAME)])
	if _frames - _since >= _wait:
		var img := root.get_viewport().get_texture().get_image()
		img.save_png(_shots[_next][0])
		print("WBCHECK shot %s %s" % [_shots[_next][0], img.get_size()])
		_next += 1
		if _next >= _shots.size():
			return true
		_place_camera(_shots[_next][1])
		_since = _frames
	return false


func _count(n: Node, c: Dictionary) -> void:
	c["nodes"] += 1
	if n is MultiMeshInstance3D:
		c["multimesh_instances"] += 1
		if n.multimesh == null or n.multimesh.mesh == null:
			c["null_meshes"] += 1
		else:
			c["instances"] += n.multimesh.instance_count
	elif n is MeshInstance3D:
		c["mesh_instances"] += 1
		if n.mesh == null:
			c["null_meshes"] += 1
		if n.is_in_group("wb_water"):
			c["water_bodies"] += 1
	elif n is Marker3D:
		c["markers"] += 1
	elif n is DirectionalLight3D:
		c["lights"] += 1
	elif n is CollisionShape3D:
		c["collision_shapes"] += 1
	if n is CollisionObject3D:
		c["shape_owners"] += n.get_shape_owners().size()
	for ch in n.get_children(true):
		_count(ch, c)


func _report() -> void:
	var c := {"nodes": 0, "multimesh_instances": 0, "instances": 0, "mesh_instances": 0, "null_meshes": 0, "markers": 0, "lights": 0, "collision_shapes": 0, "shape_owners": 0, "water_bodies": 0}
	_count(_world, c)
	c["build_ms"] = Time.get_ticks_msec() - _start
	print("WBCHECK counts ", JSON.stringify(c))
	var expect = JSON.parse_string(FileAccess.get_file_as_string("res://world_builder/data/check.json"))
	if typeof(expect) != TYPE_DICTIONARY:
		return
	print("WBCHECK expected instances %d, got %d" % [int(expect["instances"]), _lod0(_world)])
	var space := root.world_3d.direct_space_state
	var worst := 0.0
	for p in expect["probes"]:
		var q := PhysicsRayQueryParameters3D.create(Vector3(p[0], 5000.0, p[1]), Vector3(p[0], -5000.0, p[1]))
		q.collision_mask = 1
		var hit := space.intersect_ray(q)
		if hit.is_empty():
			print("WBCHECK probe %.1f, %.1f: no hit" % [p[0], p[1]])
			worst = 1e9
			continue
		var err: float = abs(hit["position"].y - float(p[2]))
		worst = max(worst, err)
		print("WBCHECK probe %.1f, %.1f: hit %.2f expected %.2f" % [p[0], p[1], hit["position"].y, p[2]])
	print("WBCHECK probe worst error %.3f m" % worst)


## Instances drawn at the finest LOD (every instance once).
func _lod0(n: Node) -> int:
	var total := 0
	for ch in n.get_children(true):
		if ch.name == "Grass":
			continue
		if ch is MultiMeshInstance3D and ch.multimesh != null and ch.visibility_range_begin == 0.0:
			total += ch.multimesh.instance_count
		total += _lod0(ch)
	return total


func _place_camera(v: PackedFloat32Array) -> void:
	var cam := root.get_viewport().get_camera_3d()
	if cam == null or v.size() != 5:
		return
	var space := root.world_3d.direct_space_state
	var focus := Vector3(v[0], 0.0, v[1])
	# the terrain only (layer 1), like Skyfold Studio's orbit focus
	var hit := space.intersect_ray(PhysicsRayQueryParameters3D.create(focus + Vector3.UP * 5000.0, focus + Vector3.DOWN * 5000.0, 1))
	var sea: float = cam.get("sea_level") if cam.get("sea_level") != null else 0.0
	focus.y = max(hit["position"].y if not hit.is_empty() else 0.0, sea)
	var yaw := v[2]
	var pitch := v[3]
	var eye := focus + Vector3(cos(pitch) * sin(yaw), sin(pitch), cos(pitch) * cos(yaw)) * v[4]
	cam.global_position = eye
	cam.look_at(focus, Vector3.UP)
	cam.set_physics_process(false)
	print("WBCHECK camera at %s looking at %s" % [eye, focus])
