# Fire flipbooks

AetherFX's flame effects (MIT, <https://github.com/holokat/AetherFX>, `LICENSE-AetherFX.txt`):
`fire_torch` (torch), `fire_torch_brazier` (brazier), `campfire` (campfire) and `lantern_light`
(candle), baked to seamless looping sprite sheets by `tools/flames/bake_flames.py` with the
AetherFX command line tool: side-on, without their smoke, halo and embers, the wind lean taken
out, the flame sprites upright. Premultiplied colour and coverage; `flames.json` holds each sheet's
grid, pace and frame size. Played by `src/flames.rs`.

Rebake (after changing an effect):

    cmake -S <aetherfx> -B <build> -G Ninja -DCMAKE_BUILD_TYPE=Release -DAETHER_BUILD_TESTS=OFF
    cmake --build <build>
    python3 tools/flames/bake_flames.py --aetherfx <aetherfx> --cli <build>/bin/aetherfx
