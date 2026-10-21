extends Node3D
## A door, double door, portcullis, set of bars, secret door, lever, pressure plate or spike trap
## from a Skyfold Studio dungeon. Its parts (in the dungeon's GLB, under the node `node_name`)
## swing, slide or rise between their `closed` and `opened` poses. On foot, E works the one looked
## at within reach (a hint shows under the crosshair); stepping on a plate works it; a lever or a
## plate works its `links` too. Shut doors, gates and bars block the way (bars always).
## `is_open` is how it stands now; call `work()` from your own game code to work it.

signal worked(open: bool)

@export var kind := "door"
@export var node_name := ""
@export var start_open := false
@export var blocks := false
@export var usable := false
## width, height, depth (a plate's or trap's depth is its width)
@export var size := Vector3(1.6, 2.6, 0.24)
@export var closed: Array = []
@export var opened: Array = []
## the other pieces (their ids: nodes named Piece<id> beside this one) a lever or plate works
@export var links: Array = []
@export var seconds := 0.9
@export var label := ""

var is_open := false
var _t := 0.0
var _root: Node3D
var _parts: Array = []
var _shape: CollisionShape3D
var _on_plate := false


func _ready() -> void:
	add_to_group("wb_pieces")
	is_open = start_open
	_t = 1.0 if is_open else 0.0
	var top: Node = owner if owner != null else get_tree().current_scene
	if top != null and node_name != "":
		_root = top.find_child(node_name, true, false) as Node3D
	if _root == null:
		push_warning("wb_piece: no node %s in the dungeon" % node_name)
		return
	for k in closed.size():
		var part := _root.find_child("%s_%d" % [node_name, k], false, false) as Node3D
		_parts.append(part)
	_pose()
	if blocks:
		var body := StaticBody3D.new()
		_shape = CollisionShape3D.new()
		var box := BoxShape3D.new()
		box.size = size
		_shape.shape = box
		_shape.position = Vector3(0.0, size.y * 0.5, 0.0)
		body.add_child(_shape)
		_root.add_child(body)
		_shape.disabled = is_open and kind != "bars"


func work() -> void:
	is_open = not is_open
	worked.emit(is_open)
	if kind == "lever" or kind == "pressure_plate":
		for id in links:
			var other := get_parent().get_node_or_null("Piece%d" % int(id))
			if other != null and other.has_method("work"):
				other.is_open = not other.is_open
				other.worked.emit(other.is_open)


func _pose() -> void:
	var k := _t * _t * (3.0 - 2.0 * _t)
	for i in _parts.size():
		if _parts[i] != null and i < opened.size():
			_parts[i].transform = (closed[i] as Transform3D).interpolate_with(opened[i] as Transform3D, k)


func _process(delta: float) -> void:
	if _root == null:
		return
	var target := 1.0 if is_open else 0.0
	if _t != target:
		_t = move_toward(_t, target, delta / maxf(seconds, 0.05))
		_pose()
	if _shape != null:
		_shape.disabled = _t > 0.5 and kind != "bars"
	var cam := get_viewport().get_camera_3d()
	if cam == null:
		return
	var on_foot: bool = cam.get("walking") != false
	if kind == "pressure_plate":
		var eye: float = cam.get("eye_height") if "eye_height" in cam else 1.7
		var l := _root.global_transform.affine_inverse() * (cam.global_position - Vector3.UP * eye)
		var on := on_foot and absf(l.x) < size.x * 0.5 and absf(l.z) < size.x * 0.5 and absf(l.y) < 0.6
		if on and not _on_plate:
			work()
		_on_plate = on
	_hint(cam, on_foot)


## How near the piece the eye is, when it's within reach and looked at (else INF).
func score(cam: Camera3D) -> float:
	if not usable or _root == null:
		return INF
	var c := _root.global_transform * Vector3(0.0, minf(size.y * 0.5, 1.2), 0.0)
	var to := c - cam.global_position
	var d := to.length()
	if d > 2.8 or (-cam.global_basis.z).dot(to / maxf(d, 0.001)) < 0.55:
		return INF
	return d


func _focused(cam: Camera3D) -> bool:
	var mine := score(cam)
	if mine == INF:
		return false
	for p in get_tree().get_nodes_in_group("wb_pieces"):
		if p != self and p.score(cam) < mine:
			return false
	return true


func _hint(cam: Camera3D, on_foot: bool) -> void:
	var top: Node = owner if owner != null else get_tree().current_scene
	var hint := top.get_node_or_null("Hints/Use") as Label if top != null else null
	if hint == null:
		return
	if on_foot and _focused(cam):
		var what := "Open the %s" % kind.replace("_", " ")
		match kind:
			"lever":
				what = "Push the lever back" if is_open else "Pull the lever"
			"secret_door":
				what = "Close the passage" if is_open else "Push the loose stone"
			_:
				if is_open:
					what = "Close the %s" % kind.replace("_", " ")
		hint.text = "[E]  " + what
		hint.visible = true
		hint.set_meta("by", get_path())
	elif hint.get_meta("by", NodePath()) == get_path():
		hint.visible = false


func _unhandled_input(event: InputEvent) -> void:
	if not usable or not (event is InputEventKey) or not event.pressed or event.echo or event.keycode != KEY_E:
		return
	var cam := get_viewport().get_camera_3d()
	if cam == null or cam.get("walking") == false:
		return
	if _focused(cam):
		work()
		get_viewport().set_input_as_handled()
