// Skyfold Studio water for Unity (URP and the built-in pipeline): the editor's water optics in
// two unlit passes (no depth or opaque texture needed). The water column under each pixel comes
// from the ground heights the importer bakes (`_Ground`); the first pass keeps the bed behind it
// by its transmittance (per channel), the second adds the light scattered in it, the Fresnel sky
// reflection and a sun glint. Two scrolling normal maps faded with distance, a thin foam line on
// the shore and the world's haze. The importer sets the optics, the light, the haze and the
// ground from worldbuilder.json.
Shader "WorldBuilder/Water"
{
    Properties
    {
        _Shallow ("Water colour (a sandy bed at the visibility depth)", Color) = (0.1, 0.6, 0.6, 1)
        _Deep ("Depth colour", Color) = (0.02, 0.1, 0.2, 1)
        _Clarity ("Visibility (m)", Float) = 6
        _Transparency ("See-through", Range(0, 1)) = 0.8
        _Roughness ("Roughness", Range(0, 1)) = 0.2
        _ShoreFoam ("Shore foam", Range(0, 2)) = 1
        _Flow ("Drift (m/s)", Vector) = (0.3, 0.1, 0, 0)
        _BumpMap ("Normal map", 2D) = "bump" {}
        _Ground ("Ground heights (m)", 2D) = "black" {}
        _GroundRect ("Ground rect (x0, z0, 1/size, has ground)", Vector) = (0, 0, 0, 0)
        _SunDir ("Towards the sun", Vector) = (0.3, 0.8, 0.3, 0)
        _SunColor ("Sun colour x intensity", Color) = (1, 1, 1, 1)
        _SkyTop ("Sky (zenith)", Color) = (0.3, 0.5, 0.8, 1)
        _SkyHorizon ("Sky (horizon)", Color) = (0.7, 0.8, 0.9, 1)
        _FogColor ("Haze colour", Color) = (0.7, 0.75, 0.8, 1)
        _FogDensity ("Haze density (per m)", Float) = 0
        _Extinct ("Extinction per metre (linear)", Vector) = (0.3, 0.08, 0.06, 0)
        _Inscat ("In-scatter albedo (linear)", Vector) = (0.01, 0.05, 0.1, 0)
        _SunDisp ("Sun light (display units)", Vector) = (1, 1, 1, 0)
        _SkyDisp ("Sky light (display units)", Vector) = (0.3, 0.4, 0.5, 0)
        _Exposure ("Display units per scene unit", Float) = 1
    }
    CGINCLUDE
            #include "UnityCG.cginc"

            fixed4 _Shallow, _Deep, _SunColor, _SkyTop, _SkyHorizon, _FogColor;
            float _Clarity, _Transparency, _Roughness, _ShoreFoam, _FogDensity, _Exposure;
            float4 _Flow, _GroundRect, _SunDir, _Extinct, _Inscat, _SunDisp, _SkyDisp;
            sampler2D _BumpMap, _Ground;

            struct v2f
            {
                float4 pos : SV_POSITION;
                float3 world : TEXCOORD0;
            };

            v2f vert(appdata_base v)
            {
                v2f o;
                o.world = mul(unity_ObjectToWorld, v.vertex).xyz;
                o.pos = UnityWorldToClipPos(o.world);
                return o;
            }

            float3 waves(float2 xz, float t, float fade)
            {
                float2 a = xz / 9.0 + _Flow.xy * t / 9.0;
                float2 b = float2(xz.x * 0.8 - xz.y * 0.6, xz.x * 0.6 + xz.y * 0.8) / 23.0 - _Flow.yx * t / 23.0;
                float3 n1 = UnpackNormal(tex2D(_BumpMap, a));
                float3 n2 = UnpackNormal(tex2D(_BumpMap, b));
                float2 d = (n1.xy + n2.xy) * fade;
                return normalize(float3(d.x, 1.0, d.y));
            }

            float groundAt(float2 xz)
            {
                float2 guv = (xz - _GroundRect.xy) * _GroundRect.z;
                bool inside = _GroundRect.w > 0.5 && all(guv > 0.0) && all(guv < 1.0);
                return inside ? tex2Dlod(_Ground, float4(guv, 0, 0)).r : -10000.0;
            }

            struct Water
            {
                // light the water adds, and how much of the scene behind it each channel keeps
                float3 add;
                float3 keep;
            };

            Water shade(v2f i, float face)
            {
                float3 toEye = _WorldSpaceCameraPos - i.world;
                float dist = length(toEye);
                float3 v = toEye / max(dist, 1e-4);
                float t = _Time.y;
                float3 n = waves(i.world.xz, t, lerp(0.55, 0.02, saturate(dist / 350.0)) * (1.0 - _Roughness * 0.5));
                if (face < 0) n = -n;
                float ndv = saturate(dot(n, v));
                // ---- the editor's water optics (`water.wesl`, as the Godot export ports them):
                // the column along the refracted view ray through the ground heights, Beer-law
                // extinction, sun and sky light scattered in it towards the depth colour, the lit
                // bed seen through it, Fresnel. Light terms are in the editor's display units.
                float3 tr = refract(-v, n, 1.0 / 1.333);
                float3 tv = normalize(float3(tr.x, min(tr.y, -0.08), tr.z));
                float tdown = max(-tv.y, 0.04);
                float column = max(i.world.y - groundAt(i.world.xz), 0.0);
                float l0 = column / tdown;
                float path = l0;
                if (l0 < 100.0)
                {
                    float l1 = max(i.world.y - groundAt(i.world.xz + tv.xz * min(l0, 200.0)), 0.0) / tdown;
                    path = max(i.world.y - groundAt(i.world.xz + tv.xz * min(0.5 * (l0 + l1), 200.0)), 0.0) / tdown;
                }
                path = min(path, 400.0);
                float3 trans = exp(-_Extinct.rgb * path);
                float3 L = normalize(_SunDir.xyz);
                float3 ls = normalize(float3(L.x / 1.333, max(sqrt(max(1.0 - (1.0 - L.y * L.y) / (1.333 * 1.333), 0.0)), 0.1), L.z / 1.333));
                float muS = max(ls.y, 0.1);
                float muV = max(-tv.y, 0.15);
                float kSun = 1.0 + muV / muS;
                float kAmb = 1.0 + muV / 0.75;
                float phase = 1.0 + 0.6 * smoothstep(-0.6, 0.2, dot(tv, ls));
                float3 sunIn = _SunDisp.rgb * (0.98 - 0.98 * pow(1.0 - max(L.y, 0.0), 5.0)) * smoothstep(0.0, 0.12, L.y) * max(L.y, 0.0) / muS;
                float3 inSun = sunIn * _Inscat.rgb * (2.7 * phase) * (1.0 - exp(-_Extinct.rgb * kSun * path)) / kSun;
                float3 inAmb = _SkyDisp.rgb * _Inscat.rgb * 3.7 * (1.0 - exp(-_Extinct.rgb * kAmb * path)) / kAmb;
                float sunW = dot(sunIn, float3(0.3, 0.5, 0.2)) / max(dot(sunIn + _SkyDisp.rgb * 1.5, float3(0.3, 0.5, 0.2)), 1e-4);
                float3 lit = lerp(exp(-_Extinct.rgb * column * 1.2) * 0.9, exp(-_Extinct.rgb * column / muS), sunW);
                float3 opaqueBody = sunIn * _Inscat.rgb * (2.7 * phase) / kSun + _SkyDisp.rgb * _Inscat.rgb * 3.7 / kAmb;
                float T = saturate(_Transparency);
                float ex = max(_Exposure, 1e-3);
                float3 bedK = lit * trans * T;
                float3 body = (inSun + inAmb) * T / ex + opaqueBody * (1.0 - T) / ex;
                // reflection of the sky, a sun glint
                float3 r = reflect(-v, n);
                float3 sky = lerp(_SkyHorizon.rgb, _SkyTop.rgb, sqrt(saturate(r.y)));
                float fres = (0.02 + 0.98 * pow(1.0 - ndv, 5.0)) * lerp(1.0, 0.6, _Roughness);
                float shine = lerp(1200.0, 60.0, _Roughness);
                float glint = pow(saturate(dot(r, L)), shine) * (shine / 60.0) * 0.12 * saturate(L.y * 3.0 + 0.1);
                float3 add = body * (1.0 - fres) + sky * fres + _SunColor.rgb * glint;
                float3 keep = bedK * (1.0 - fres);
                // a thin foam line where the water meets the shore
                float lace = saturate(1.0 - column / 0.25) * _ShoreFoam * (_GroundRect.w > 0.5 ? 1.0 : 0.0);
                lace *= 0.55 + 0.45 * sin(t * 1.3 + (i.world.x + i.world.z) * 0.15 + n.x * 6.0);
                float foam = saturate(lace) * 0.8;
                add = lerp(add, float3(0.92, 0.94, 0.95) * _SunColor.rgb * saturate(L.y * 2.0 + 0.2), foam);
                keep *= 1.0 - foam;
                // the world's haze
                float f = exp(-dist * _FogDensity);
                add = add * f + _FogColor.rgb * (1.0 - f);
                keep *= f;
                // fade in over the first centimetres so the shoreline has no hard edge
                float edge = smoothstep(0.0, 0.05, column);
                Water o;
                o.add = add * edge;
                o.keep = lerp(float3(1.0, 1.0, 1.0), keep, edge);
                return o;
            }

            fixed4 fragKeep(v2f i, float face : VFACE) : SV_Target
            {
                return fixed4(shade(i, face).keep, 1.0);
            }

            fixed4 fragAdd(v2f i, float face : VFACE) : SV_Target
            {
                return fixed4(shade(i, face).add, 1.0);
            }
    ENDCG
    // URP draws an untagged pass and a UniversalForward pass of the same object one after the
    // other; the built-in pipeline draws every untagged pass
    SubShader
    {
        Tags { "RenderPipeline" = "UniversalPipeline" "Queue" = "Transparent-10" "RenderType" = "Transparent" "IgnoreProjector" = "True" }
        Pass
        {
            // the scene behind, through the water column (per channel)
            Blend DstColor Zero
            ZWrite Off
            Cull Off
            CGPROGRAM
            #pragma target 3.0
            #pragma vertex vert
            #pragma fragment fragKeep
            ENDCG
        }
        Pass
        {
            Tags { "LightMode" = "UniversalForward" }
            // the light the water scatters, reflects and glints
            Blend One One
            ZWrite Off
            Cull Off
            CGPROGRAM
            #pragma target 3.0
            #pragma vertex vert
            #pragma fragment fragAdd
            ENDCG
        }
    }
    SubShader
    {
        Tags { "Queue" = "Transparent-10" "RenderType" = "Transparent" "IgnoreProjector" = "True" }
        Pass
        {
            // the scene behind, through the water column (per channel)
            Blend DstColor Zero
            ZWrite Off
            Cull Off
            CGPROGRAM
            #pragma target 3.0
            #pragma vertex vert
            #pragma fragment fragKeep
            ENDCG
        }
        Pass
        {
            // the light the water scatters, reflects and glints
            Blend One One
            ZWrite Off
            Cull Off
            CGPROGRAM
            #pragma target 3.0
            #pragma vertex vert
            #pragma fragment fragAdd
            ENDCG
        }
    }
    Fallback Off
}
