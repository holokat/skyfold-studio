extends Node3D
## A way between a Skyfold Studio world and its dungeons. Walking into this box (the camera's
## position in this node's frame) loads `target`. Players come out where they should: in a
## dungeon at its start; back in the world in front of the way in they came through
## (`arrivals`, keyed by way-in id), else at `arrive_position`.

@export_file("*.tscn") var target := ""
@export var half_size := Vector3(1.6, 2.2, 0.6)
## the way in this is (a world's portal); 0 for a dungeon's way out
@export var via := 0
## where players come out in the target (eye position) and the way they look
@export var arrive_position := Vector3.ZERO
@export var arrive_look := Vector3.FORWARD
## a dungeon's way out: per way in (its id as a string) [eye position, look] in the world
@export var arrivals := {}

var _armed := false


func _ready() -> void:
	# just came through a portal: the first portal of the scene places the camera
	if Engine.has_meta("wb_arrive"):
		var a = Engine.get_meta("wb_arrive")
		Engine.remove_meta("wb_arrive")
		_place.call_deferred(a)


func _place(a) -> void:
	var cam := get_viewport().get_camera_3d()
	if cam == null:
		return
	cam.global_position = a[0]
	var look: Vector3 = a[1]
	if look.length() > 0.01:
		cam.look_at(cam.global_position + look, Vector3.UP)
	# (Skyfold's walk camera keeps its own heading and walking state)
	if "_yaw" in cam:
		cam.set("_yaw", cam.rotation.y)
		cam.set("_pitch", cam.rotation.x)
	if "walking" in cam:
		cam.set("walking", Engine.get_meta("wb_walking", true))


func _process(_delta: float) -> void:
	var cam := get_viewport().get_camera_3d()
	if cam == null or target == "":
		return
	var p := global_transform.affine_inverse() * cam.global_position
	var inside := absf(p.x) <= half_size.x and absf(p.y) <= half_size.y and absf(p.z) <= half_size.z
	# (a camera that starts inside has to step out first)
	if not _armed:
		_armed = not inside
		return
	if inside:
		_go(cam)


func _go(cam: Camera3D) -> void:
	set_process(false)
	var arrive = [arrive_position, arrive_look]
	if not arrivals.is_empty():
		var k := str(Engine.get_meta("wb_via", 0))
		arrive = arrivals[k] if arrivals.has(k) else arrivals.values()[0]
	if via != 0:
		Engine.set_meta("wb_via", via)
	Engine.set_meta("wb_arrive", arrive)
	if "walking" in cam:
		Engine.set_meta("wb_walking", cam.get("walking"))
	get_tree().change_scene_to_file.call_deferred(target)
