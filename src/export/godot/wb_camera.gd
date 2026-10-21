extends Camera3D
## Explore the exported world. WASD moves (Q / E down / up while flying), the right mouse button
## (or a click, which captures the mouse) looks around, Shift goes fast, F switches between flying
## and walking on the ground, Esc frees the mouse, H hides the hints.

@export var fly_speed := 40.0
@export var walk_speed := 4.5
@export var eye_height := 1.7
@export var sea_level := 0.0
## dungeons: start on foot, find the floor from the eye down (not from the sky: there are
## ceilings and levels above), and stop at walls
@export var start_walking := false
@export var ray_from_eye := false
@export var stop_at_walls := false
var walking := false
var _yaw := 0.0
var _pitch := 0.0
var _fall := 0.0


func _ready() -> void:
	_yaw = rotation.y
	_pitch = rotation.x
	walking = start_walking


func _unhandled_input(event: InputEvent) -> void:
	if event is InputEventMouseButton and event.pressed and event.button_index == MOUSE_BUTTON_LEFT:
		Input.mouse_mode = Input.MOUSE_MODE_CAPTURED
	elif event is InputEventKey and event.pressed and not event.echo:
		match event.keycode:
			KEY_ESCAPE:
				Input.mouse_mode = Input.MOUSE_MODE_VISIBLE
			KEY_F:
				walking = not walking
				_fall = 0.0
			KEY_H:
				var hints := get_node_or_null("../Hints")
				if hints != null:
					hints.visible = not hints.visible
	elif event is InputEventMouseMotion:
		if Input.mouse_mode == Input.MOUSE_MODE_CAPTURED or Input.is_mouse_button_pressed(MOUSE_BUTTON_RIGHT):
			_yaw -= event.relative.x * 0.0025
			_pitch = clampf(_pitch - event.relative.y * 0.0025, -1.55, 1.55)
			rotation = Vector3(_pitch, _yaw, 0.0)


func _physics_process(delta: float) -> void:
	var dir := Vector3(
		float(Input.is_key_pressed(KEY_D)) - float(Input.is_key_pressed(KEY_A)),
		float(Input.is_key_pressed(KEY_E)) - float(Input.is_key_pressed(KEY_Q)),
		float(Input.is_key_pressed(KEY_S)) - float(Input.is_key_pressed(KEY_W)))
	var fast := 5.0 if Input.is_key_pressed(KEY_SHIFT) else 1.0
	if not walking:
		global_position += global_transform.basis * dir * fly_speed * fast * delta
		return
	var flat := Basis(Vector3.UP, _yaw) * Vector3(dir.x, 0.0, dir.z)
	if flat.length() > 0.0:
		var step := flat.normalized() * walk_speed * (2.5 if fast > 1.0 else 1.0) * delta
		if not (stop_at_walls and _blocked(step)):
			global_position += step
	var ground := _ground(global_position)
	var target := maxf(ground, sea_level - 1.2) + eye_height
	if global_position.y > target + 0.05:
		_fall += 9.8 * delta
		global_position.y = maxf(target, global_position.y - _fall * delta)
	else:
		_fall = 0.0
		global_position.y = target


## A wall within reach at chest height the way we're stepping.
func _blocked(step: Vector3) -> bool:
	var from := global_position + Vector3.DOWN * (eye_height * 0.45)
	var q := PhysicsRayQueryParameters3D.create(from, from + step.normalized() * (step.length() + 0.35))
	return not get_world_3d().direct_space_state.intersect_ray(q).is_empty()


func _ground(p: Vector3) -> float:
	var top := p + (Vector3.UP * 0.4 if ray_from_eye else Vector3.UP * 400.0)
	var q := PhysicsRayQueryParameters3D.create(top, p + Vector3.DOWN * 3000.0)
	var hit := get_world_3d().direct_space_state.intersect_ray(q)
	return hit["position"].y if hit else p.y - eye_height
