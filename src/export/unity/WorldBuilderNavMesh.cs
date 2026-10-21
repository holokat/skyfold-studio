// Skyfold Studio navmesh: adds the NavMeshData the importer baked (without the AI Navigation
// package) to the scene's navigation while enabled, in the editor and in play mode.

using UnityEngine;
using UnityEngine.AI;

namespace WorldBuilder
{
    [ExecuteAlways]
    public class WorldBuilderNavMesh : MonoBehaviour
    {
        public UnityEngine.AI.NavMeshData data;
        NavMeshDataInstance instance;

        void OnEnable()
        {
            if (data != null && !instance.valid) instance = NavMesh.AddNavMeshData(data, transform.position, transform.rotation);
        }

        void OnDisable()
        {
            if (instance.valid) instance.Remove();
        }
    }
}
