extends Light3D
## A Skyfold Studio fire's flicker: the light's energy wavers round `base_energy` as the editor's
## does (`amount` 0 steady … 1 a wild fire).

@export var base_energy := 1.0
@export var amount := 0.35
@export var seed := 0


func _process(_delta: float) -> void:
	var t := Time.get_ticks_msec() / 1000.0 * 1.3
	var s := seed * 0.137
	var n := sin(t * 9.1 + s) * 0.35 + sin(t * 15.7 + s * 2.3) * 0.25 + sin(t * 4.3 + s * 0.7) * 0.4
	light_energy = base_energy * maxf(1.0 + n * 0.22 * amount, 0.2)
