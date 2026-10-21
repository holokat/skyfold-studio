// A Skyfold Studio fire's flicker: the light's intensity wavers round `baseIntensity` as the
// editor's does (`amount` 0 steady … 1 a wild fire).
using UnityEngine;

namespace WorldBuilder
{
    [RequireComponent(typeof(Light))]
    public class WorldBuilderFlicker : MonoBehaviour
    {
        public float baseIntensity = 1f;
        public float amount = 0.35f;
        public int seed;
        Light lightSource;

        void Awake()
        {
            lightSource = GetComponent<Light>();
        }

        void Update()
        {
            float t = Time.time * 1.3f;
            float s = seed * 0.137f;
            float n = Mathf.Sin(t * 9.1f + s) * 0.35f + Mathf.Sin(t * 15.7f + s * 2.3f) * 0.25f + Mathf.Sin(t * 4.3f + s * 0.7f) * 0.4f;
            lightSource.intensity = baseIntensity * Mathf.Max(1f + n * 0.22f * amount, 0.2f);
        }
    }
}
