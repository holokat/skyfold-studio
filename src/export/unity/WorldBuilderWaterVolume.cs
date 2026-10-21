// Skyfold Studio water volume: a trigger box from a lake's, river stretch's or the ocean's bed up
// to its surface (on the Water layer). Character controllers can ask it whether a point is under
// water and how deep, to switch to swimming.

using UnityEngine;

namespace WorldBuilder
{
    [RequireComponent(typeof(BoxCollider))]
    public class WorldBuilderWaterVolume : MonoBehaviour
    {
        /// lake, river, ocean, or lava (a glowing lake: hurts, nobody swims in it)
        public string kind;
        /// world height of the water surface (m)
        public float surfaceHeight;

        /// How far `point` is below the surface (0 or less: above it).
        public float Depth(Vector3 point)
        {
            return surfaceHeight - point.y;
        }

        void OnDrawGizmosSelected()
        {
            var box = GetComponent<BoxCollider>();
            Gizmos.color = new Color(0.2f, 0.5f, 1f, 0.25f);
            Gizmos.matrix = transform.localToWorldMatrix;
            Gizmos.DrawCube(box.center, box.size);
        }
    }
}
