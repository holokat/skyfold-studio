// A Skyfold Studio gameplay item (player start, POI, NPC, spawn region, resource node, fishing
// spot), created by the Skyfold Studio importer. Read these from your game code; the gizmos show
// them in the Scene view.

using UnityEngine;

namespace WorldBuilder
{
    public class WorldBuilderMarker : MonoBehaviour
    {
        /// player_start, poi, npc, spawn_region, resource, fishing
        public string kind;
        public int id;
        public string displayName;
        /// POI kind, NPC role, spawn behaviour, resource kind or fish species
        public string type;
        public float radius = 1f;
        [TextArea] public string notes;
        /// Spawn regions: the outline in world XZ (Unity space).
        public Vector2[] polygon = new Vector2[0];

        void OnDrawGizmos()
        {
            Gizmos.color = kind switch
            {
                "player_start" => new Color(0.3f, 1f, 0.4f),
                "poi" => new Color(1f, 0.8f, 0.2f),
                "npc" => new Color(0.4f, 0.7f, 1f),
                "spawn_region" => new Color(1f, 0.35f, 0.3f),
                "resource" => new Color(0.75f, 0.5f, 1f),
                _ => new Color(0.3f, 0.9f, 1f),
            };
            Gizmos.DrawWireSphere(transform.position, Mathf.Max(radius, 0.5f));
            Gizmos.DrawLine(transform.position, transform.position + transform.forward * 2f);
            for (int i = 0; i < polygon.Length; i++)
            {
                var a = polygon[i];
                var b = polygon[(i + 1) % polygon.Length];
                Gizmos.DrawLine(new Vector3(a.x, transform.position.y, a.y), new Vector3(b.x, transform.position.y, b.y));
            }
        }
    }
}
