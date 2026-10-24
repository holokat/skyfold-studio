// Skyfold Studio sky for Unity (URP and the built-in pipeline): the editor's zenith and horizon
// colours for the world's hour (as the Godot export calibrates them side by side), a darker band
// below the horizon and the sun disc with its glow. Used as the scene's skybox, so it also lights
// the ambient.
Shader "WorldBuilder/Sky"
{
    Properties
    {
        _SkyTop ("Zenith", Color) = (0.3, 0.5, 0.8, 1)
        _SkyHorizon ("Horizon", Color) = (0.7, 0.8, 0.9, 1)
        _Energy ("Energy", Float) = 1
        _SunDir ("Towards the sun", Vector) = (0.3, 0.8, 0.3, 0)
        _SunColor ("Sun colour", Color) = (1, 1, 1, 1)
        _SunSize ("Sun radius (degrees)", Float) = 0.6
    }
    SubShader
    {
        Tags { "Queue" = "Background" "RenderType" = "Background" "PreviewType" = "Skybox" }
        Cull Off
        ZWrite Off
        Pass
        {
            CGPROGRAM
            #pragma vertex vert
            #pragma fragment frag
            #include "UnityCG.cginc"

            fixed4 _SkyTop, _SkyHorizon, _SunColor;
            float4 _SunDir;
            float _Energy, _SunSize;

            struct v2f
            {
                float4 pos : SV_POSITION;
                float3 dir : TEXCOORD0;
            };

            v2f vert(appdata_base v)
            {
                v2f o;
                o.pos = UnityObjectToClipPos(v.vertex);
                o.dir = v.vertex.xyz;
                return o;
            }

            fixed4 frag(v2f i) : SV_Target
            {
                float3 d = normalize(i.dir);
                float3 col = lerp(_SkyHorizon.rgb, _SkyTop.rgb, sqrt(saturate(d.y)));
                col = lerp(col, _SkyHorizon.rgb * 0.55, saturate(-d.y * 4.0));
                float3 s = normalize(_SunDir.xyz);
                float c = dot(d, s);
                float r = radians(_SunSize);
                float disc = smoothstep(cos(r * 1.15), cos(r), c);
                float glow = pow(saturate(c), 600.0) * 0.6 + pow(saturate(c), 24.0) * 0.08;
                float up = saturate(s.y * 8.0 + 0.3);
                col += _SunColor.rgb * (disc * 40.0 + glow) * up;
                return fixed4(col * _Energy, 1.0);
            }
            ENDCG
        }
    }
    Fallback Off
}
