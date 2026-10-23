@tool
extends Node3D
## Skyfold Studio weather and atmosphere, as the editor runs them (`sky::update_weather`,
## `volumetrics::medium`): the zone under the camera sets the weather (a zone's own weather wins,
## elsewhere the painted biome's), its lighting mood and its haze; everything eases towards them
## (weather over a couple of seconds, moods over `fade_seconds` at zone borders). The weather drives
## the cloud deck in the sky shader, rain and snow round the camera, the distance fog, the
## volumetric haze (sunbeams), weather fog and ground mist, how much the sun is dimmed and the
## exposure under heavy cloud, and lightning in storms. Data: atmosphere.json, zones.png and
## biomes.png from Skyfold Studio. In the editor the world's own weather shows (no particles).

@export_file("*.json") var data_path := "res://world_builder/atmosphere/atmosphere.json"
@export var environment_path: NodePath = ^"../WorldEnvironment"
@export var sun_path: NodePath = ^"../Sun"
@export var moon_path: NodePath = ^"../Moon"
## Rain and snow particles round the camera (in game).
@export var precipitation := true
## A weather (clear, cloudy, rain, storm, snow, fog, sandstorm, or its name) to preview
## everywhere; empty follows the zones and biomes.
@export var weather_override := ""
## Hold the ground this wet, puddles this full (0..1; below 0 follows the weather).
@export var wetness_override := -1.0

const KEYS := ["clouds", "fog", "wet", "rain", "snow", "dim", "dust"]

var _d: Dictionary
var _zones: Image
var _biomes: Image
var _env: Environment
var _sky: ShaderMaterial
var _sun: DirectionalLight3D
var _moon: DirectionalLight3D
var _base := {}
var _w := {}
var _light := {}
var _haze := 1.0
var _haze_amount := 0.0
var _first := true
var _first_wind := true
var _first_rain := true
var _first_rain_t := 0.0
var _drift := Vector2.ZERO
var _flash := 0.0
var _strike_in := 8.0
var _rain: GPUParticles3D
var _snow: GPUParticles3D
var _mist: FogVolume
var _zone := -1
var _cloud_offset := Vector2.ZERO
## How wet the ground is and how full the puddles are (`rain::track`: surfaces soak in a minute
## of steady rain and take minutes to dry; puddles fill only under steady rain and linger).
var wetness := 0.0
var puddles := 0.0
## The wind blowing where the camera is (`wind::update_wind`): the world's or the zone's speed,
## strengthened by storms, and the travelling gust field's offset. Grass and foliage read these.
var wind_speed := 0.0
var wind_dir := Vector2(1.0, 0.0)
var gust_offset := Vector2.ZERO


func _ready() -> void:
	add_to_group("wb_atmosphere")
	var d = JSON.parse_string(FileAccess.get_file_as_string(data_path))
	if typeof(d) != TYPE_DICTIONARY:
		push_error("Skyfold Studio: can't read " + data_path)
		return
	_d = d
	_zones = _map(str(_d.get("zones_map", "")))
	# the editor's cloud cover field and the shade under the tree crowns (global shader textures)
	var cl: Dictionary = _d.get("clouds", {})
	if cl.has("cover"):
		RenderingServer.global_shader_parameter_set("wb_cover", load(cl["cover"]))
		_cloud_offset = Vector2(cl["offset"][0], cl["offset"][1])
	var sh = _d.get("shore")
	if sh is Dictionary and FileAccess.file_exists(sh["file"]):
		var m := int(sh["size"])
		var simg := Image.create_from_data(m, m, false, Image.FORMAT_RGBAF, FileAccess.get_file_as_bytes(sh["file"]))
		RenderingServer.global_shader_parameter_set("wb_shore", ImageTexture.create_from_image(simg))
		var g: Dictionary = _d.get("grid", {})
		var half := (float(g.get("vertices", 2)) - 1.0) * float(g.get("cell", 1.0)) * 0.5
		RenderingServer.global_shader_parameter_set("wb_shore_p", Vector4(half, float(m), float(sh["sea_hs"]), float(sh["shore_foam"])))
		RenderingServer.global_shader_parameter_set("wb_water_tint", Vector3(sh["water"][0], sh["water"][1], sh["water"][2]))
	var cp = _d.get("canopy")
	if cp is Dictionary and FileAccess.file_exists(cp["file"]):
		var n := int(cp["size"])
		var img := Image.create_from_data(n, n, false, Image.FORMAT_RGBF, FileAccess.get_file_as_bytes(cp["file"]))
		RenderingServer.global_shader_parameter_set("wb_canopy", ImageTexture.create_from_image(img))
		RenderingServer.global_shader_parameter_set("wb_canopy_grid", Vector4(cp["origin"][0], cp["origin"][1], float(cp["texel"]), float(n)))
	_biomes = _map(str(_d.get("biomes_map", "")))
	var we := get_node_or_null(environment_path) as WorldEnvironment
	if we == null or we.environment == null:
		return
	_env = we.environment
	if _env.sky != null:
		_sky = _env.sky.sky_material as ShaderMaterial
	_sun = get_node_or_null(sun_path) as DirectionalLight3D
	_moon = get_node_or_null(moon_path) as DirectionalLight3D
	_base = {
		"exposure": _env.tonemap_exposure,
		"contrast": _env.adjustment_contrast,
		"saturation": _env.adjustment_saturation,
		"glow": _env.glow_intensity,
		"ambient": _env.ambient_light_energy,
		"sun_scatter": _env.fog_sun_scatter,
		"sun": _sun.light_energy if _sun else 0.0,
		"moon": _moon.light_energy if _moon else 0.0,
	}
	for k in KEYS:
		_w[k] = 0.0
	_light = _neutral()
	for c in get_children(true):
		if c.has_meta("wb_built"):
			c.free()
	_mist = _volume("GroundMist", Vector3(240.0, 40.0, 240.0))
	if precipitation and not Engine.is_editor_hint():
		_rain = _particles("Rain", 9000, 1.3, Vector3(35.0, 12.0, 35.0), Vector2(0.012, 0.9), Color(0.8, 0.84, 0.9, 0.3), false)
		_snow = _particles("Snow", 20000, 8.0, Vector3(25.0, 12.0, 25.0), Vector2(0.09, 0.09), Color(1.0, 1.0, 1.0, 0.95), true)


func _map(path: String) -> Image:
	if path == "" or not ResourceLoader.exists(path):
		return null
	var tex := load(path) as Texture2D
	if tex == null:
		return null
	var img := tex.get_image()
	if img.is_compressed():
		img.decompress()
	return img


func _neutral() -> Dictionary:
	return {"brightness": 0.0, "contrast": 1.0, "saturation": 1.0, "sun": 1.0, "shadow_fill": 1.0, "fog": 0.0, "fog_tint": [1.0, 1.0, 1.0], "glow": 0.0}


func _volume(n: String, size: Vector3) -> FogVolume:
	var v := FogVolume.new()
	v.name = n
	v.size = size
	v.shape = RenderingServer.FOG_VOLUME_SHAPE_BOX
	var m := FogMaterial.new()
	m.density = 0.0
	m.edge_fade = 0.3
	v.material = m
	v.set_meta("wb_built", true)
	add_child(v, false, Node.INTERNAL_MODE_BACK)
	return v


func _particles(n: String, amount: int, life: float, box: Vector3, quad: Vector2, color: Color, snow: bool) -> GPUParticles3D:
	var p := GPUParticles3D.new()
	p.name = n
	p.amount = amount
	p.lifetime = life
	p.preprocess = life
	p.local_coords = false
	p.emitting = false
	p.amount_ratio = 0.0
	p.visibility_aabb = AABB(-box * 1.5 - Vector3(0.0, 40.0, 0.0), box * 3.0 + Vector3(0.0, 40.0, 0.0))
	var pm := ParticleProcessMaterial.new()
	pm.emission_shape = ParticleProcessMaterial.EMISSION_SHAPE_BOX
	pm.emission_box_extents = box
	pm.direction = Vector3(0.0, -1.0, 0.0)
	pm.spread = 4.0 if not snow else 20.0
	pm.gravity = Vector3.ZERO
	pm.initial_velocity_min = 10.0 if not snow else 0.9
	pm.initial_velocity_max = 12.0 if not snow else 1.6
	if snow:
		pm.turbulence_enabled = true
		pm.turbulence_noise_strength = 0.6
		pm.turbulence_noise_scale = 3.0
		pm.scale_min = 0.6
		pm.scale_max = 1.4
	p.process_material = pm
	var mesh := QuadMesh.new()
	mesh.size = quad
	var mat := StandardMaterial3D.new()
	mat.transparency = BaseMaterial3D.TRANSPARENCY_ALPHA
	mat.albedo_color = color
	# rain is the colour of the lit air round it (set as the weather changes); flakes are soft discs
	mat.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
	# (no streak or flake right in front of the lens)
	mat.distance_fade_mode = BaseMaterial3D.DISTANCE_FADE_PIXEL_ALPHA
	mat.distance_fade_min_distance = 0.6
	mat.distance_fade_max_distance = 3.0
	if snow:
		var g := Gradient.new()
		g.set_color(0, Color(1.0, 1.0, 1.0, 1.0))
		g.set_color(1, Color(1.0, 1.0, 1.0, 0.0))
		var disc := GradientTexture2D.new()
		disc.gradient = g
		disc.fill = GradientTexture2D.FILL_RADIAL
		disc.fill_from = Vector2(0.5, 0.5)
		disc.fill_to = Vector2(0.5, 0.0)
		disc.width = 32
		disc.height = 32
		mat.albedo_texture = disc
	mat.billboard_mode = BaseMaterial3D.BILLBOARD_ENABLED if snow else BaseMaterial3D.BILLBOARD_FIXED_Y
	mat.billboard_keep_scale = true
	mat.cull_mode = BaseMaterial3D.CULL_DISABLED
	mat.shadow_to_opacity = false
	mesh.material = mat
	p.draw_pass_1 = mesh
	p.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	p.set_meta("wb_built", true)
	add_child(p, false, Node.INTERNAL_MODE_BACK)
	return p


## The zone and biome ids under a point (the editor's nearest-vertex lookup).
func _ids(x: float, z: float) -> Vector2i:
	var g: Dictionary = _d.get("grid", {})
	var n := int(g.get("vertices", 0))
	var cell := float(g.get("cell", 1.0))
	if n < 2:
		return Vector2i.ZERO
	var half := (n - 1) * cell * 0.5
	if x < -half or x > half or z < -half or z > half:
		return Vector2i.ZERO
	var i := clampi(roundi((x + half) / cell), 0, n - 1)
	var j := clampi(roundi((z + half) / cell), 0, n - 1)
	var zone := roundi(_zones.get_pixel(i, j).r * 255.0) if _zones else 0
	var biome := roundi(_biomes.get_pixel(i, j).r * 255.0) if _biomes else 0
	return Vector2i(zone, biome)


func _params(weather: String, k: float) -> Dictionary:
	var all: Dictionary = _d["weathers"]
	var clear: Dictionary = all["clear"]
	var wanted := weather.to_lower()
	var w: Dictionary = all.get(wanted, {})
	if w.is_empty():
		for id in all:
			if str(all[id]["name"]).to_lower() == wanted:
				w = all[id]
	if w.is_empty():
		w = clear
	var out := {}
	for key in KEYS:
		out[key] = lerpf(float(clear[key]), float(w[key]), clampf(k, 0.0, 1.0))
	return out


func _target(ids: Vector2i) -> Array:
	var world: Dictionary = _d["world"]
	var t := _params("clear", 1.0)
	t["clouds"] = float(world["clouds"])
	var zone: Dictionary = _d["zones"].get(str(ids.x), {})
	var biome: Dictionary = _d["biomes"].get(str(ids.y), {})
	var light = null
	var over := weather_override
	if over == "" and world.get("override") != null:
		over = str(world["override"])
	if over != "":
		t = _params(over, 1.0)
		t["clouds"] = maxf(t["clouds"], float(world["clouds"]))
	else:
		var wk = null
		if bool(zone.get("weather_enabled", false)):
			wk = [str(zone["weather"]), float(zone["intensity"])]
		elif not biome.is_empty():
			wk = [str(biome["weather"]), float(biome["intensity"])]
		if wk != null:
			t = _params(wk[0], wk[1])
			t["clouds"] = maxf(t["clouds"], float(world["clouds"]))
		if bool(zone.get("clouds_enabled", false)):
			var cc := float(zone["cloud_cover"])
			t["clouds"] = maxf(t["clouds"], cc) if bool(zone.get("weather_enabled", false)) else cc
	if not bool(world.get("precipitation", true)):
		t["rain"] = 0.0
		t["snow"] = 0.0
		t["wet"] = 0.0
	# the zone's lighting mood, else the biome's
	if zone.get("light") != null:
		light = zone["light"]
	elif biome.get("light") != null:
		light = biome["light"]
	# the zone's haze override
	var haze := 1.0 if bool(world.get("haze_enabled", true)) else 0.0
	var amount := float(world.get("haze", 0.0))
	match str(zone.get("haze_mode", "inherit")):
		"on":
			haze = 1.0
			amount = float(zone.get("haze_amount", amount))
		"off":
			haze = 0.0
	return [t, light if light != null else _neutral(), haze, amount]


func _process(delta: float) -> void:
	if _env == null or _d.is_empty():
		return
	var cam := get_viewport().get_camera_3d() if not Engine.is_editor_hint() else null
	var pos := cam.global_position if cam else Vector3.ZERO
	var ids := _ids(pos.x, pos.z) if cam else Vector2i.ZERO
	_zone = ids.x
	var target := _target(ids)
	var k := 1.0 if _first else 1.0 - exp(-delta * 0.6)
	var kl := 1.0 if _first else 1.0 - exp(-delta * 3.0 / maxf(float(_d.get("fade_seconds", 3.0)), 0.1))
	_first = false
	for key in KEYS:
		_w[key] = lerpf(_w[key], target[0][key], k)
	for key in ["brightness", "contrast", "saturation", "sun", "shadow_fill", "fog", "glow"]:
		_light[key] = lerpf(float(_light[key]), float(target[1].get(key, _neutral()[key])), kl)
	var tint: Array = target[1].get("fog_tint", [1.0, 1.0, 1.0])
	for c in 3:
		_light["fog_tint"][c] = lerpf(float(_light["fog_tint"][c]), float(tint[c]), kl)
	_haze = lerpf(_haze, target[2], kl)
	_haze_amount = target[3]
	var world: Dictionary = _d["world"]
	var wind: Dictionary = world["wind"]
	var zone: Dictionary = _d["zones"].get(str(ids.x), {})
	var target_speed := float(zone["wind_speed"]) if bool(zone.get("wind_enabled", false)) else float(wind["speed"])
	target_speed = target_speed * (1.0 + _w["rain"] * 0.6) + _w["dim"] * 5.0 * _w["rain"] + _w["dust"] * 9.0
	var kw := 1.0 - exp(-delta * 0.8)
	var d0 := Vector2(wind["dir"][0], wind["dir"][1]).normalized()
	if d0 == Vector2.ZERO:
		d0 = Vector2(1.0, 0.0)
	if _first_wind:
		wind_speed = target_speed
		wind_dir = d0
		_first_wind = false
	wind_speed += (target_speed - wind_speed) * kw
	wind_dir = wind_dir.lerp(d0, kw).normalized()
	gust_offset += wind_dir * maxf(wind_speed, 0.4) * 0.8 * delta
	var wind_v := wind_dir * wind_speed
	_drift += wind_v * float(world.get("cloud_speed", 1.0)) * 8.0 * delta
	# the clouds drift as the editor's (`sky::advance_time`): a fixed wind aloft, faster in rain
	var cl: Dictionary = _d.get("clouds", {})
	if not cl.is_empty():
		var tile := float(cl["tile"])
		_cloud_offset += Vector2(14.0, 6.0) * float(cl["speed"]) * (1.0 + _w["rain"] * 1.5) * delta
		_cloud_offset = Vector2(fposmod(_cloud_offset.x, tile), fposmod(_cloud_offset.y, tile))
	_lightning(delta)
	# wet ground and puddles follow the weather slowly (the editor's time constants)
	var rain_now := clampf(_w["rain"], 0.0, 1.0)
	var target_wet := clampf(_w["wet"], 0.0, 1.0)
	var fill := smoothstep(0.2, 0.85, rain_now) * target_wet
	var follow := func(cur: float, to: float, up: float, down: float) -> float:
		var tau := 0.4 if _first_rain else (up if to > cur else down)
		return cur + (to - cur) * (1.0 - exp(-delta / tau))
	wetness = follow.call(wetness, target_wet, 18.0 / (0.25 + rain_now), 150.0)
	puddles = follow.call(puddles, fill, 40.0 / (0.2 + rain_now), 320.0)
	if wetness_override >= 0.0:
		wetness = clampf(wetness_override, 0.0, 1.0)
		puddles = wetness
	_first_rain_t += delta
	_first_rain = _first_rain_t < 3.0
	var pf := float(_d.get("rain", {}).get("puddles", 0.6))
	RenderingServer.global_shader_parameter_set("wb_rain", Vector4(wetness, puddles * clampf(pf, 0.0, 1.0), rain_now, 0.0))
	_apply(pos, cam, wind_v)


func _lightning(delta: float) -> void:
	_flash = maxf(_flash - delta * 6.0, 0.0)
	var storm := clampf((_w["rain"] - 0.6) * 2.5, 0.0, 1.0) * clampf((_w["dim"] - 0.55) * 4.0, 0.0, 1.0)
	var rate := float(_d["world"].get("lightning", 1.0)) * storm * 3.0 / 60.0
	if rate <= 0.0 or Engine.is_editor_hint():
		return
	_strike_in -= delta
	if _strike_in <= 0.0:
		_flash = 1.0
		_strike_in = randf_range(0.3, 2.0) / rate


func _apply(pos: Vector3, cam: Camera3D, wind_v: Vector2) -> void:
	var w := _w
	var L := _light
	var look: Dictionary = _d["look"]
	var fog: Dictionary = _d["fog"]
	var s := _haze
	var dim: float = w["dim"]
	# exposure: the eye opens under heavy cloud; the zone's brightness
	_env.tonemap_exposure = _base["exposure"] * pow(2.0, 1.3 * dim + float(L["brightness"]))
	_env.adjustment_contrast = _base["contrast"] * float(L["contrast"])
	_env.adjustment_saturation = _base["saturation"] * float(L["saturation"])
	_env.glow_intensity = _base["glow"] + float(L["glow"])
	_env.ambient_light_energy = _base["ambient"] * float(L["shadow_fill"])
	if _sun:
		_sun.light_energy = _base["sun"] * (1.0 - 0.85 * dim) * float(L["sun"]) + _flash * 2.5
	if _moon:
		_moon.light_energy = _base["moon"] * (1.0 - 0.7 * dim)
	# distance fog: clear air (the calibrated haze) plus weather and zone fog
	var lh := float(look["haze"])
	var amount := _haze_amount * s
	var clear := s * float(fog["air"]) + pow((0.00005 * s + amount * 0.00012) * lh, 2.0) * 3000.0 + float(look["color_haze"]) * s / maxf(float(look["haze_distance"]), 10.0) * 0.5
	var col := _rgb(fog["clear_color"]).lerp(_rgb(fog["storm_color"]), dim * 0.7)
	col = col.lerp(_rgb(fog["dust_color"]), float(w["dust"]))
	var t: Array = L["fog_tint"]
	col = Color(col.r * float(t[0]), col.g * float(t[1]), col.b * float(t[2]))
	_env.fog_light_color = col
	var local := 0.25
	var extra2 := (float(w["fog"]) * 0.0016 + float(w["rain"]) * 0.0004 + float(w["snow"]) * 0.0006 + float(L["fog"]) * float(fog["zone_density"])) * local * s
	_env.fog_sun_scatter = _base["sun_scatter"] * (1.0 - dim) * s
	# volumetric air (`volumetrics::medium`): clear haze and street haze where sunbeams show, the
	# weather's and zone's fog, and mist on the ground
	var air := float(look["air"])
	var beams := float(look["sunbeams"])
	var haze_m := (0.00005 * s + amount * 0.00045) * lh * air * (1.0 + 1.2 * beams) + (float(w["rain"]) * 0.0005 + float(w["snow"]) * 0.0009 + float(w["dust"]) * 0.004) * s
	var ground := 0.0015 * air * (0.4 + beams) * minf(lh, 2.0) * s
	var fog_m := (float(w["fog"]) * 0.007 + float(L["fog"]) * 0.005) * lh * s
	var mist := (float(look["mist"]) + 0.35 * float(w["fog"]) + 0.12 * float(w["wet"])) * 0.012 * s
	var albedo := Color(0.92, 0.92, 0.92).lerp(Color(0.95, 0.78, 0.55), float(w["dust"]))
	albedo = Color(albedo.r * float(t[0]), albedo.g * float(t[1]), albedo.b * float(t[2]))
	# the editor's froxel volume reaches 800 m round a walker (Medium): the weather's and the
	# zone's fog live in it (Godot's volumetric fog, as long); past it the distance fog carries
	# the editor's quarter share (exponential-squared as the exponential density that reaches half
	# at the same distance)
	var weather_air := fog_m + (float(w["rain"]) * 0.0005 + float(w["snow"]) * 0.0009 + float(w["dust"]) * 0.004) * s
	_env.fog_enabled = clear + extra2 > 0.0
	_env.fog_density = clear + extra2 * 0.83
	# weather fog is sunlit air (a light warm grey, greyer under storm cloud) and hides the sky
	var share := clampf(extra2 * 0.83 / maxf(_env.fog_density, 1e-6), 0.0, 1.0)
	var lit := _rgb(fog["weather_color"]).lerp(_rgb(fog["storm_color"]), dim * 0.5)
	_env.fog_light_color = _env.fog_light_color.lerp(Color(lit.r * float(t[0]), lit.g * float(t[1]), lit.b * float(t[2])), share)
	_env.fog_sky_affect = clampf(float(w["fog"]) + 0.5 * (float(w["rain"]) + float(w["snow"])), 0.0, 1.0) * s
	_env.volumetric_fog_enabled = haze_m + ground + mist + fog_m > 0.00001
	_env.volumetric_fog_density = haze_m + ground + fog_m
	_env.volumetric_fog_albedo = albedo
	_env.volumetric_fog_anisotropy = clampf(0.72 + 0.1 * beams, 0.2, 0.85)
	var gy := _ground(pos) if cam else 0.0
	_place(_mist, mist, 5.0 + 9.0 * float(w["fog"]), pos, gy, albedo)
	# the water's light, in the editor's display units (`water.rs`): the key light dimmed by the
	# weather and the zone, the sky's irradiance following the fog colour
	var wl: Dictionary = _d.get("water_light", {})
	if not wl.is_empty():
		var key := _v3(wl["sun_disp"]) * ((1.0 - 0.85 * dim) * float(L["sun"]) if float(_d["world"].get("night", 0.0)) <= 0.5 else (1.0 - 0.7 * dim))
		var sky_c := _v3(wl["sky_clear"]).lerp(_v3(wl["sky_storm"]), dim * 0.7).lerp(_v3(wl["sky_dust"]), float(w["dust"]))
		sky_c *= Vector3(float(t[0]), float(t[1]), float(t[2]))
		RenderingServer.global_shader_parameter_set("wb_sun_dir", _v3(wl["sun_dir"]))
		RenderingServer.global_shader_parameter_set("wb_sun_disp", key)
		RenderingServer.global_shader_parameter_set("wb_sky_disp", sky_c)
		RenderingServer.global_shader_parameter_set("wb_exposure", _env.tonemap_exposure)
	var clouds: Dictionary = _d.get("clouds", {})
	if not clouds.is_empty():
		var night_k := float(_d["world"].get("night", 0.0))
		RenderingServer.global_shader_parameter_set("wb_cloud", Vector4(float(w["clouds"]), float(clouds["shadows"]) * (1.0 - night_k), _cloud_offset.x, _cloud_offset.y))
	RenderingServer.global_shader_parameter_set("wb_canopy_cam", Vector4(pos.x, pos.z, 300.0, 370.0))
	if _sky:
		_sky.set_shader_parameter("cloud_cover", float(w["clouds"]))
		_sky.set_shader_parameter("dim", dim)
		_sky.set_shader_parameter("rain", float(w["rain"]))
		_sky.set_shader_parameter("cloud_offset", _drift)
		_sky.set_shader_parameter("flash", _flash)
	# rain and snow take the colour of the lit air round them (bright by day, dark at night)
	var air_c := _env.fog_light_color
	if _rain:
		var rc := air_c.lightened(0.25)
		((_rain.draw_pass_1 as QuadMesh).material as StandardMaterial3D).albedo_color = Color(rc.r, rc.g, rc.b, 0.28)
	if _snow:
		var sc := air_c.lightened(0.6)
		((_snow.draw_pass_1 as QuadMesh).material as StandardMaterial3D).albedo_color = Color(sc.r, sc.g, sc.b, 0.95)
	_precip(_rain, float(w["rain"]), pos, wind_v, 0.1)
	_precip(_snow, float(w["snow"]), pos, wind_v, 0.5)


func _v3(c: Array) -> Vector3:
	return Vector3(float(c[0]), float(c[1]), float(c[2]))


func _rgb(c: Array) -> Color:
	return Color(float(c[0]), float(c[1]), float(c[2]))


## The terrain height under a point (layer 1), or the sea.
func _ground(p: Vector3) -> float:
	var space := get_world_3d().direct_space_state
	var hit := space.intersect_ray(PhysicsRayQueryParameters3D.create(p + Vector3.UP * 3000.0, p + Vector3.DOWN * 3000.0, 1))
	return maxf(hit["position"].y if not hit.is_empty() else 0.0, float(_d.get("sea_level", 0.0)))


## A fog volume round the camera from the ground up, thinning with height over `height` metres.
func _place(v: FogVolume, density: float, height: float, pos: Vector3, gy: float, albedo: Color) -> void:
	var m := v.material as FogMaterial
	m.density = density
	m.albedo = albedo
	m.height_falloff = 1.0 / maxf(height, 1.0)
	v.visible = density > 0.00001
	v.global_position = Vector3(pos.x, gy - 2.0 + v.size.y * 0.5, pos.z)


func _precip(p: GPUParticles3D, amount: float, pos: Vector3, wind_v: Vector2, drift: float) -> void:
	if p == null:
		return
	p.emitting = amount > 0.01
	p.amount_ratio = clampf(amount, 0.0, 1.0)
	p.global_position = pos + Vector3(0.0, 10.0, 0.0)
	var pm := p.process_material as ParticleProcessMaterial
	pm.direction = Vector3(wind_v.x * drift, -1.0, wind_v.y * drift).normalized()


## The weather blended at the camera now (for game code: clouds, fog, wet, rain, snow, dim, dust).
func current_weather() -> Dictionary:
	return _w.duplicate()


## The zone id under the camera (0 outside every zone).
func current_zone() -> int:
	return _zone
