// Skyfold Studio importer for Unity 6 (editor only). Builds a scene from a world exported by
// Skyfold Studio into Assets/WorldBuilder/Worlds/<World>/: a Terrain (heights, photoscan
// TerrainLayers, alphamaps, trees with LOD groups and trunk colliders, grass and plant details),
// rocks / props / buildings as prefab instances with colliders, water, roads, sun / sky / fog,
// gameplay markers and a fly camera.
//
//   Menu:   Tools > Skyfold Studio > Import World
//   Batch:  Unity -batchmode -projectPath <project> -executeMethod WorldBuilder.Importer.ImportFromCommandLine
//                 [-wbWorld <world folder name>] -quit -logFile -
//
// Models are GLB files and need the glTFast package (com.unity.cloud.gltfast). All data is
// already in Unity's coordinates (left-handed, Y up; Skyfold Studio's +X east became -X).

using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using UnityEditor;
using UnityEditor.SceneManagement;
using UnityEngine;
using UnityEngine.Rendering;
using Object = UnityEngine.Object;

namespace WorldBuilder
{
    public static class Importer
    {
        const string WorldsRoot = "Assets/WorldBuilder/Worlds";

        // Navigation Static is deprecated in Unity 6 (the navmesh is baked from the colliders,
        // by a NavMeshSurface or the core builder): only older editors get the flag
#if UNITY_6000_0_OR_NEWER
        const StaticEditorFlags NavStatic = 0;
#else
        const StaticEditorFlags NavStatic = StaticEditorFlags.NavigationStatic;
#endif

        [MenuItem("Tools/Skyfold Studio/Import World")]
        public static void ImportMenu()
        {
            var worlds = FindWorlds();
            if (worlds.Count == 0)
            {
                EditorUtility.DisplayDialog("Skyfold Studio", "No exported world found. Copy the exported WorldBuilder folder into Assets first (it holds Worlds/<name>/worldbuilder.json).", "OK");
                return;
            }
            var folder = worlds[0];
            if (worlds.Count > 1)
            {
                var picked = EditorUtility.OpenFolderPanel("Pick the world to import", WorldsRoot, "");
                if (string.IsNullOrEmpty(picked)) return;
                folder = ToAssetPath(picked);
            }
            try
            {
                var report = Import(folder);
                EditorUtility.DisplayDialog("Skyfold Studio", "Imported: " + report, "OK");
            }
            finally
            {
                EditorUtility.ClearProgressBar();
            }
        }

        /// Batch-mode entry point (see the header). Exits with code 1 on failure.
        public static void ImportFromCommandLine()
        {
            try
            {
                var args = Environment.GetCommandLineArgs();
                string wanted = null;
                for (int i = 0; i < args.Length - 1; i++)
                    if (args[i] == "-wbWorld") wanted = args[i + 1];
                var worlds = FindWorlds();
                if (worlds.Count == 0) throw new Exception("no worldbuilder.json under " + WorldsRoot);
                var folder = wanted == null ? worlds[0] : worlds.FirstOrDefault(w => w.EndsWith("/" + wanted)) ?? wanted;
                Debug.Log("WBIMPORT OK " + Import(folder));
            }
            catch (Exception e)
            {
                Debug.LogError("WBIMPORT FAILED " + e);
                EditorApplication.Exit(1);
            }
        }

        static List<string> FindWorlds()
        {
            return AssetDatabase.FindAssets("worldbuilder t:TextAsset", new[] { "Assets" })
                .Select(AssetDatabase.GUIDToAssetPath)
                .Where(p => p.EndsWith("/worldbuilder.json"))
                .Select(p => p.Substring(0, p.Length - "/worldbuilder.json".Length))
                .OrderBy(p => p)
                .ToList();
        }

        static string ToAssetPath(string abs)
        {
            abs = abs.Replace('\\', '/');
            var data = Application.dataPath.Replace('\\', '/');
            return abs.StartsWith(data) ? "Assets" + abs.Substring(data.Length) : abs;
        }

        static void EnsureFolder(string path)
        {
            if (AssetDatabase.IsValidFolder(path)) return;
            var parent = Path.GetDirectoryName(path).Replace('\\', '/');
            EnsureFolder(parent);
            AssetDatabase.CreateFolder(parent, Path.GetFileName(path));
        }

        static void Step(string what, float t)
        {
            if (!Application.isBatchMode) EditorUtility.DisplayProgressBar("Skyfold Studio", what, t);
        }

        /// Builds the world's scene; returns a one-line report.
        public static string Import(string folder)
        {
            var w = JsonUtility.FromJson<WorldData>(File.ReadAllText(Path.Combine(folder, "worldbuilder.json")));
            var gen = folder + "/Generated";
            EnsureFolder(gen + "/Prefabs");
            EnsureFolder(gen + "/Materials");
            var report = new List<string>();
            var scene = EditorSceneManager.NewScene(NewSceneSetup.EmptyScene, NewSceneMode.Single);

            // the shared model textures: normal maps and linear data set up before the models load
            if (w.textures != null)
            {
                foreach (var tex in w.textures)
                    Texture(folder + "/" + tex.file, tex.role == "normal", tex.role != "color");
                AssetDatabase.Refresh();
            }
            Step("Terrain", 0.05f);
            var terrains = BuildTerrain(w, folder, gen, report);
            Step("Models", 0.3f);
            var prefabs = BuildPrefabs(w, folder, gen, report);
            Step("Trees", 0.5f);
            PlaceTrees(w, terrains, prefabs, report);
            Step("Grass and plants", 0.6f);
            PlaceDetails(w, folder, gen, terrains, prefabs, report);
            Step("Rocks and props", 0.7f);
            PlaceProps(w, prefabs, report);
            Step("Water and roads", 0.85f);
            BuildWater(w, folder, gen, terrains, report);
            BuildRoads(w, folder, report);
            BuildCaves(w, folder, report);
            Step("Light and markers", 0.95f);
            BuildLighting(w, gen);
            BuildMarkers(w, report);
            BuildWaysIn(w, folder, report);
            BuildFires(w.fires, w, folder, true);
            BuildFixtureLights(w, report);
            var camera = BuildCamera(w);
            BuildLook(w, folder, gen, camera);
            Step("Navigation", 0.97f);
            BuildNavMesh(w, gen, report);

            var scenePath = folder + "/" + w.safeName + ".unity";
            EditorSceneManager.SaveScene(scene, scenePath);
            report.Add("scene " + scenePath);
            // the dungeons: a scene each, in the build with the world's (portals load them by name)
            var dungeonScenes = BuildDungeons(w, folder, scenePath, prefabs, report);
            if (dungeonScenes.Count > 0)
            {
                AddToBuild(scenePath, dungeonScenes);
                EditorSceneManager.OpenScene(scenePath);
            }
            AssetDatabase.SaveAssets();
            return string.Join("; ", report);
        }

        // ------------------------------------------------------------------ terrain

        static Texture2D Texture(string path, bool normal, bool linear)
        {
            var imp = AssetImporter.GetAtPath(path) as TextureImporter;
            if (imp != null)
            {
                var type = normal ? TextureImporterType.NormalMap : TextureImporterType.Default;
                if (imp.textureType != type || imp.sRGBTexture == linear)
                {
                    imp.textureType = type;
                    imp.sRGBTexture = !linear && !normal;
                    imp.SaveAndReimport();
                }
            }
            return AssetDatabase.LoadAssetAtPath<Texture2D>(path);
        }

        /// The terrain tiles (one up to 4097 samples, 2049-sample tiles beyond), row-major from
        /// the -X -Z corner, joined with SetNeighbors.
        static Terrain[] BuildTerrain(WorldData w, string folder, string gen, List<string> report)
        {
            int n = w.heightmapResolution;
            int tiles = Mathf.Max(1, w.tilesPerSide);
            var layers = new TerrainLayer[w.layers.Length];
            for (int i = 0; i < layers.Length; i++)
            {
                var l = w.layers[i];
                var tl = new TerrainLayer
                {
                    name = l.name,
                    diffuseTexture = Texture(folder + "/" + l.albedo, false, false),
                    normalMapTexture = Texture(folder + "/" + l.normal, true, true),
                    maskMapTexture = Texture(folder + "/" + l.mask, false, true),
                    tileSize = new Vector2(l.tile, l.tile),
                    smoothness = l.smoothness,
                    metallic = 0f,
                    normalScale = 1f,
                };
                AssetDatabase.CreateAsset(tl, gen + "/Layer_" + l.key + ".terrainlayer");
                layers[i] = tl;
            }
            var result = new Terrain[tiles * tiles];
            int holes = 0;
            foreach (var tile in w.tiles)
            {
                var td = new TerrainData();
                td.heightmapResolution = n;
                td.alphamapResolution = w.alphamapResolution;
                td.size = new Vector3(w.tileSize, Mathf.Max(w.heightMax - w.heightMin, 1f), w.tileSize);
                // the asset first: the alphamaps and holes live in sub-assets that are only saved
                // when they are written to a TerrainData that is already an asset (else Unity
                // reloads it as all first layer)
                AssetDatabase.CreateAsset(td, gen + "/TerrainData_" + tile.x + "_" + tile.z + ".asset");
                var raw = File.ReadAllBytes(Path.Combine(folder, tile.heightmap));
                var h = new float[n, n];
                for (int z = 0; z < n; z++)
                    for (int x = 0; x < n; x++)
                        h[z, x] = BitConverter.ToUInt16(raw, (z * n + x) * 2) / 65535f;
                td.SetHeights(0, 0, h);
                td.terrainLayers = layers;
                int a = w.alphamapResolution;
                var bytes = File.ReadAllBytes(Path.Combine(folder, tile.alphamaps));
                int L = layers.Length;
                var maps = new float[a, a, L];
                for (int z = 0; z < a; z++)
                    for (int x = 0; x < a; x++)
                        for (int k = 0; k < L; k++)
                            maps[z, x, k] = bytes[(z * a + x) * L + k] / 255f;
                td.SetAlphamaps(0, 0, maps);
                // cave mouths: the terrain is cut open where the caves draw the ground
                if (tile.holes != null && tile.holes.Length >= 2)
                {
                    int c = n - 1;
                    var solid = new bool[c, c];
                    for (int z = 0; z < c; z++)
                        for (int x = 0; x < c; x++)
                            solid[z, x] = true;
                    for (int k = 0; k + 1 < tile.holes.Length; k += 2)
                        solid[tile.holes[k], tile.holes[k + 1]] = false;
                    td.SetHoles(0, 0, solid);
                    holes += tile.holes.Length / 2;
                }
                EditorUtility.SetDirty(td);

                var go = Terrain.CreateTerrainGameObject(td);
                go.name = tiles == 1 ? "Terrain" : "Terrain " + tile.x + "_" + tile.z;
                go.transform.position = new Vector3(tile.position[0], tile.position[1], tile.position[2]);
                GameObjectUtility.SetStaticEditorFlags(go, StaticEditorFlags.ContributeGI | StaticEditorFlags.OccluderStatic | StaticEditorFlags.BatchingStatic | NavStatic);
                var t = go.GetComponent<Terrain>();
                t.drawInstanced = true;
                t.heightmapPixelError = 3f;
                t.basemapDistance = 2000f;
                // trees are prefabs with LOD groups: the terrain draws them through their LODs (its
                // billboard settings only apply to Tree Editor trees), the far LOD reaching the draw
                // distance; Tree Distance still culls them, so it is the world's draw distance
                t.treeDistance = w.treeDistance;
                t.treeBillboardDistance = w.treeDistance;
                t.treeLODBiasMultiplier = 1f;
                t.treeMaximumFullLODCount = 100000;
                t.detailObjectDistance = 120f;
                t.detailObjectDensity = 1f;
                t.groupingID = 1;
                t.allowAutoConnect = true;
                result[tile.z * tiles + tile.x] = t;
            }
            Terrain At(int x, int z) => x >= 0 && z >= 0 && x < tiles && z < tiles ? result[z * tiles + x] : null;
            for (int z = 0; z < tiles; z++)
                for (int x = 0; x < tiles; x++)
                    if (At(x, z) != null) At(x, z).SetNeighbors(At(x - 1, z), At(x, z + 1), At(x + 1, z), At(x, z - 1));
            if (holes > 0) report.Add(holes + " terrain cells opened for caves");
            report.Add("terrain " + tiles + "x" + tiles + " tiles of " + n + "x" + n + " (" + layers.Length + " layers, alphamaps " + w.alphamapResolution + ")");
            return result;
        }

        /// The tile holding a Unity-space position.
        static Terrain TileAt(WorldData w, Terrain[] tiles, Vector3 p)
        {
            int k = Mathf.Max(1, w.tilesPerSide);
            int x = Mathf.Clamp(Mathf.FloorToInt((p.x + w.size * 0.5f) / w.tileSize), 0, k - 1);
            int z = Mathf.Clamp(Mathf.FloorToInt((p.z + w.size * 0.5f) / w.tileSize), 0, k - 1);
            return tiles[z * k + x];
        }

        // ------------------------------------------------------------------ models

        class Variant
        {
            public GameObject prefab;
            /// single-mesh prefab of the finest LOD (terrain detail prototypes)
            public GameObject detail;
        }

        static Transform FindDeep(Transform t, string name)
        {
            if (t.name == name) return t;
            foreach (Transform c in t)
            {
                var r = FindDeep(c, name);
                if (r != null) return r;
            }
            return null;
        }

        /// Screen height at which an object of `size` metres is `distance` away (60° FOV).
        static float ScreenHeight(float size, float distance)
        {
            return Mathf.Clamp(size / (Mathf.Max(distance, 1f) * 2f * Mathf.Tan(30f * Mathf.Deg2Rad)), 0.0005f, 0.99f);
        }

        static Dictionary<string, List<Variant>> BuildPrefabs(WorldData w, string folder, string gen, List<string> report)
        {
            var result = new Dictionary<string, List<Variant>>();
            int built = 0, missing = 0;
            foreach (var m in w.models)
            {
                var asset = AssetDatabase.LoadAssetAtPath<GameObject>(folder + "/" + m.file);
                if (asset == null)
                {
                    missing++;
                    continue;
                }
                var list = new List<Variant>();
                var physics = PhysicsFor(m.physics, gen);
                // generated trees and scans, and packed models reorganised into a variant node
                // with LOD children; older packed files are taken whole
                if (m.variants.Length > 0 && !string.IsNullOrEmpty(m.variants[0].node))
                {
                    foreach (var v in m.variants)
                    {
                        var src = FindDeep(asset.transform, v.node);
                        if (src == null) continue;
                        var go = Object.Instantiate(src.gameObject);
                        go.name = v.node;
                        go.transform.localPosition = Vector3.zero;
                        go.transform.localRotation = Quaternion.identity;
                        go.transform.localScale = Vector3.one;
                        var lods = new List<LOD>();
                        float size = Mathf.Max(v.height, v.width, 0.2f);
                        float last = 1f;
                        Mesh finest = null, coarsest = null;
                        for (int k = 0; k < v.lods.Length; k++)
                        {
                            var child = FindDeep(go.transform, v.lods[k]);
                            if (child == null) continue;
                            var renderers = child.GetComponentsInChildren<Renderer>();
                            if (!m.shadows || k >= 2)
                                foreach (var r in renderers) r.shadowCastingMode = ShadowCastingMode.Off;
                            // lightmapped pieces: LOD0 with lightmap UVs, the others lit by probes
                            if (m.lightmap)
                                foreach (var r in renderers)
                                {
                                    var mr = r as MeshRenderer;
                                    if (mr == null) continue;
                                    if (k == 0)
                                    {
                                        var f = mr.GetComponent<MeshFilter>();
                                        if (f != null && f.sharedMesh != null) f.sharedMesh = WithLightmapUVs(f.sharedMesh, gen, v.node + "_" + f.name);
                                        mr.receiveGI = ReceiveGI.Lightmaps;
                                    }
                                    else mr.receiveGI = ReceiveGI.LightProbes;
                                }
                            float end = m.lodEnd[Mathf.Min(k, m.lodEnd.Length - 1)];
                            float sh = Mathf.Min(ScreenHeight(size, end), last * 0.95f);
                            last = sh;
                            lods.Add(new LOD(sh, renderers));
                            var mf = child.GetComponentInChildren<MeshFilter>();
                            if (mf != null)
                            {
                                if (finest == null) finest = mf.sharedMesh;
                                coarsest = mf.sharedMesh;
                            }
                        }
                        if (lods.Count > 0)
                        {
                            var group = go.AddComponent<LODGroup>();
                            group.SetLODs(lods.ToArray());
                            group.RecalculateBounds();
                        }
                        AddCollider(go, m.collider, v, finest, coarsest, CollisionMesh(asset, v), physics);
                        var prefab = PrefabUtility.SaveAsPrefabAsset(go, gen + "/Prefabs/" + v.node + ".prefab");
                        Object.DestroyImmediate(go);
                        var detail = m.detail && finest != null ? DetailPrefab(gen, v.node, src, v.lods.Length > 0 ? v.lods[0] : null) : null;
                        list.Add(new Variant { prefab = prefab, detail = detail });
                        built++;
                    }
                }
                else
                {
                    var go = Object.Instantiate(asset);
                    go.name = m.safeName;
                    foreach (var mf in go.GetComponentsInChildren<MeshFilter>())
                    {
                        if (m.collider == "mesh" || m.collider == "convex")
                        {
                            var mc = mf.gameObject.AddComponent<MeshCollider>();
                            mc.sharedMesh = mf.sharedMesh;
                            mc.convex = m.collider == "convex";
                            mc.sharedMaterial = physics;
                        }
                    }
                    var prefab = PrefabUtility.SaveAsPrefabAsset(go, gen + "/Prefabs/" + m.safeName + ".prefab");
                    Object.DestroyImmediate(go);
                    list.Add(new Variant { prefab = prefab });
                    built++;
                }
                result[m.safeName] = list;
            }
            report.Add(built + " prefabs from " + (w.models.Length - missing) + " models");
            if (missing > 0)
            {
                report.Add(missing + " GLB models not imported (install glTFast: com.unity.cloud.gltfast)");
                Debug.LogWarning("WBIMPORT " + missing + " GLB models are not imported: add the glTFast package (com.unity.cloud.gltfast) and run Tools > Skyfold Studio > Import World again.");
            }
            return result;
        }

        /// A copy of `src` with a generated lightmap UV set (saved once under Generated/Meshes).
        static Mesh WithLightmapUVs(Mesh src, string gen, string name)
        {
            var path = gen + "/Meshes/" + name + ".asset";
            var known = AssetDatabase.LoadAssetAtPath<Mesh>(path);
            if (known != null) return known;
            var m = Object.Instantiate(src);
            m.name = name;
            // the unwrap splits vertices along its seams: a big mesh outgrows 16-bit indices (the
            // triangles have to be set again after the switch)
            if (m.indexFormat == IndexFormat.UInt16)
            {
                var subs = new int[m.subMeshCount][];
                for (int i = 0; i < subs.Length; i++) subs[i] = m.GetTriangles(i);
                m.indexFormat = IndexFormat.UInt32;
                for (int i = 0; i < subs.Length; i++) m.SetTriangles(subs[i], i);
            }
            Unwrapping.GenerateSecondaryUVSet(m);
            EnsureFolder(gen + "/Meshes");
            AssetDatabase.CreateAsset(m, path);
            return m;
        }

        /// The variant's collision proxy (`<variant>_COL`: a convex hull, or a walk-in model's
        /// simplified shape), if the export made one.
        static Mesh CollisionMesh(GameObject asset, VariantData v)
        {
            if (string.IsNullOrEmpty(v.collision)) return null;
            var t = FindDeep(asset.transform, v.collision);
            var mf = t != null ? t.GetComponentInChildren<MeshFilter>() : null;
            return mf != null ? mf.sharedMesh : null;
        }

        static void AddCollider(GameObject go, string kind, VariantData v, Mesh finest, Mesh coarsest, Mesh proxy, PhysicsMaterial physics)
        {
            Collider c = null;
            if (kind == "capsule")
            {
                var cap = go.AddComponent<CapsuleCollider>();
                cap.radius = Mathf.Max(v.trunk, 0.05f);
                cap.height = Mathf.Max(Mathf.Min(v.height * 0.6f, 8f), cap.radius * 2f + 0.1f);
                cap.center = new Vector3(0f, cap.height * 0.5f, 0f);
                c = cap;
            }
            else if (kind == "convex" && (proxy ?? coarsest) != null)
            {
                var mc = go.AddComponent<MeshCollider>();
                mc.sharedMesh = proxy ?? coarsest;
                mc.convex = true;
                c = mc;
            }
            else if (kind == "mesh" && (proxy ?? finest) != null)
            {
                var mc = go.AddComponent<MeshCollider>();
                mc.sharedMesh = proxy ?? finest;
                c = mc;
            }
            if (c != null && physics != null) c.sharedMaterial = physics;
        }

        static readonly Dictionary<string, PhysicsMaterial> physicsMaterials = new Dictionary<string, PhysicsMaterial>();

        /// One physics material per footstep surface (friction and bounciness from the export).
        static PhysicsMaterial PhysicsFor(PhysicsData p, string gen)
        {
            if (p == null || string.IsNullOrEmpty(p.surface)) return null;
            if (physicsMaterials.TryGetValue(p.surface, out var known) && known != null) return known;
            var m = new PhysicsMaterial("WB_" + p.surface)
            {
                dynamicFriction = p.friction,
                staticFriction = Mathf.Min(1f, p.friction * 1.15f),
                bounciness = p.bounciness,
                frictionCombine = PhysicsMaterialCombine.Average,
                bounceCombine = PhysicsMaterialCombine.Average,
            };
            EnsureFolder(gen + "/Physics");
            AssetDatabase.CreateAsset(m, gen + "/Physics/WB_" + p.surface + ".asset");
            physicsMaterials[p.surface] = m;
            return m;
        }

        /// A detail prototype must be one mesh at the prefab root, with instancing materials.
        static GameObject DetailPrefab(string gen, string name, Transform src, string lod0)
        {
            var node = lod0 != null ? FindDeep(src, lod0) : src;
            var mf = node != null ? node.GetComponentInChildren<MeshFilter>() : null;
            var mr = mf != null ? mf.GetComponent<MeshRenderer>() : null;
            if (mf == null || mr == null) return null;
            var mats = new Material[mr.sharedMaterials.Length];
            for (int i = 0; i < mats.Length; i++)
            {
                var path = gen + "/Materials/" + name + "_detail_" + i + ".mat";
                var copy = new Material(mr.sharedMaterials[i]) { enableInstancing = true };
                AssetDatabase.CreateAsset(copy, path);
                mats[i] = copy;
            }
            var go = new GameObject(name + "_detail");
            go.AddComponent<MeshFilter>().sharedMesh = mf.sharedMesh;
            var r = go.AddComponent<MeshRenderer>();
            r.sharedMaterials = mats;
            r.shadowCastingMode = ShadowCastingMode.Off;
            var prefab = PrefabUtility.SaveAsPrefabAsset(go, gen + "/Prefabs/" + name + "_detail.prefab");
            Object.DestroyImmediate(go);
            return prefab;
        }

        // ------------------------------------------------------------------ placement

        static void PlaceTrees(WorldData w, Terrain[] terrains, Dictionary<string, List<Variant>> prefabs, List<string> report)
        {
            var protos = new List<TreePrototype>();
            var instances = new Dictionary<Terrain, List<TreeInstance>>();
            foreach (var t in terrains) instances[t] = new List<TreeInstance>();
            int count = 0;
            foreach (var m in w.models)
            {
                if (!m.tree || !prefabs.TryGetValue(m.safeName, out var list) || list.Count == 0) continue;
                int first = protos.Count;
                foreach (var v in list) protos.Add(new TreePrototype { prefab = v.prefab, bendFactor = 0f });
                var stretch = Stretch(m);
                for (int i = 0; i + 8 < m.data.Length; i += 9)
                {
                    int variant = Mathf.Clamp((int)m.data[i], 0, list.Count - 1);
                    var world = new Vector3(m.data[i + 1], m.data[i + 2], m.data[i + 3]);
                    var terrain = TileAt(w, terrains, world);
                    var td = terrain.terrainData;
                    var p = world - terrain.transform.position;
                    var q = new Quaternion(m.data[i + 4], m.data[i + 5], m.data[i + 6], m.data[i + 7]);
                    float s = m.data[i + 8];
                    var f = stretch.TryGetValue(i / 9, out var sf) ? sf : Vector3.one;
                    instances[terrain].Add(new TreeInstance
                    {
                        prototypeIndex = first + variant,
                        position = new Vector3(Mathf.Clamp01(p.x / td.size.x), p.y / td.size.y, Mathf.Clamp01(p.z / td.size.z)),
                        rotation = q.eulerAngles.y * Mathf.Deg2Rad,
                        widthScale = s * Mathf.Max(f.x, f.z),
                        heightScale = s * f.y,
                        color = Color.white,
                        lightmapColor = Color.white,
                    });
                    count++;
                }
            }
            foreach (var t in terrains)
            {
                t.terrainData.treePrototypes = protos.ToArray();
                t.terrainData.RefreshPrototypes();
                t.terrainData.SetTreeInstances(instances[t].ToArray(), false);
            }
            report.Add(count + " trees (" + protos.Count + " prototypes)");
        }

        static void PlaceDetails(WorldData w, string folder, string gen, Terrain[] terrains, Dictionary<string, List<Variant>> prefabs, List<string> report)
        {
            if (w.details == null || w.details.Length == 0) return;
            int res = w.detailResolution;
            var protos = new List<DetailPrototype>();
            var layers = new List<int>();
            for (int i = 0; i < w.details.Length; i++)
            {
                var d = w.details[i];
                if (!prefabs.TryGetValue(d.model, out var list) || list.Count == 0) continue;
                var v = list[Mathf.Clamp(d.variant, 0, list.Count - 1)];
                if (v.detail == null) continue;
                protos.Add(new DetailPrototype
                {
                    usePrototypeMesh = true,
                    prototype = v.detail,
                    renderMode = DetailRenderMode.VertexLit,
                    useInstancing = true,
                    minWidth = d.minScale,
                    maxWidth = d.maxScale,
                    minHeight = d.minScale,
                    maxHeight = d.maxScale,
                    noiseSpread = 0.4f,
                    healthyColor = Color.white,
                    dryColor = new Color(0.92f, 0.9f, 0.85f),
                    alignToGround = d.align,
                });
                layers.Add(i);
            }
            long placed = 0;
            int k = Mathf.Max(1, w.tilesPerSide);
            foreach (var tile in w.tiles)
            {
                if (string.IsNullOrEmpty(tile.detailMaps)) continue;
                var td = terrains[tile.z * k + tile.x].terrainData;
                td.SetDetailResolution(res, 32);
                td.SetDetailScatterMode(DetailScatterMode.InstanceCountMode);
                td.detailPrototypes = protos.ToArray();
                var bytes = File.ReadAllBytes(Path.Combine(folder, tile.detailMaps));
                for (int j = 0; j < layers.Count; j++)
                {
                    int off = layers[j] * res * res;
                    var map = new int[res, res];
                    for (int z = 0; z < res; z++)
                        for (int x = 0; x < res; x++)
                        {
                            map[z, x] = bytes[off + z * res + x];
                            placed += map[z, x];
                        }
                    td.SetDetailLayer(0, 0, j, map);
                }
            }
            report.Add(protos.Count + " detail layers (" + placed + " grass tufts and plants)");
        }

        /// Per-axis factors of a model's non-uniformly scaled instances, by instance row.
        static Dictionary<int, Vector3> Stretch(ModelData m)
        {
            var d = new Dictionary<int, Vector3>();
            if (m.stretch == null) return d;
            for (int k = 0; k + 3 < m.stretch.Length; k += 4)
                d[(int)m.stretch[k]] = new Vector3(m.stretch[k + 1], m.stretch[k + 2], m.stretch[k + 3]);
            return d;
        }

        static void PlaceProps(WorldData w, Dictionary<string, List<Variant>> prefabs, List<string> report)
        {
            // grouped by world cell (Props/Cell x_z/<model>): streaming or scene partitioning can
            // load a cell's objects together
            var root = new GameObject("Props");
            var cells = new Dictionary<string, Transform>();
            Transform Parent(Vector3 p, string model)
            {
                float c = Mathf.Max(w.cellSize, 1f);
                string key = Mathf.FloorToInt((p.x + w.size * 0.5f) / c) + "_" + Mathf.FloorToInt((p.z + w.size * 0.5f) / c);
                if (!cells.TryGetValue(key, out var cell))
                {
                    cell = new GameObject("Cell " + key).transform;
                    cell.SetParent(root.transform, false);
                    cells[key] = cell;
                }
                if (!cells.TryGetValue(key + "/" + model, out var parent))
                {
                    parent = new GameObject(model).transform;
                    parent.SetParent(cell, false);
                    cells[key + "/" + model] = parent;
                }
                return parent;
            }
            int count = 0;
            foreach (var m in w.models)
            {
                if (m.tree || m.detail || !prefabs.TryGetValue(m.safeName, out var list) || list.Count == 0) continue;
                var stretch = Stretch(m);
                for (int i = 0; i + 8 < m.data.Length; i += 9)
                {
                    var v = list[Mathf.Clamp((int)m.data[i], 0, list.Count - 1)];
                    var go = (GameObject)PrefabUtility.InstantiatePrefab(v.prefab, Parent(new Vector3(m.data[i + 1], m.data[i + 2], m.data[i + 3]), m.name));
                    go.transform.SetPositionAndRotation(new Vector3(m.data[i + 1], m.data[i + 2], m.data[i + 3]), new Quaternion(m.data[i + 4], m.data[i + 5], m.data[i + 6], m.data[i + 7]));
                    go.transform.localScale = (stretch.TryGetValue(i / 9, out var f) ? f : Vector3.one) * m.data[i + 8];
                    var flags = StaticEditorFlags.BatchingStatic | StaticEditorFlags.OccluderStatic | StaticEditorFlags.OccludeeStatic | NavStatic;
                    if (m.lightmap) flags |= StaticEditorFlags.ContributeGI;
                    GameObjectUtility.SetStaticEditorFlags(go, flags);
                    count++;
                }
            }
            report.Add(count + " rocks, props and buildings");
        }

        // ------------------------------------------------------------------ water, roads, light

        /// The ground under the water (metres, Unity space) for the water shader's depth: the
        /// terrain heights resampled over the whole world.
        static Texture2D GroundHeights(WorldData w, Terrain[] terrains, string gen)
        {
            const int n = 1024;
            var tex = new Texture2D(n, n, TextureFormat.RFloat, false, true)
            {
                name = "WaterGround",
                wrapMode = TextureWrapMode.Clamp,
                filterMode = FilterMode.Bilinear,
            };
            var px = new Color[n * n];
            float h = w.size * 0.5f, cell = w.size / n;
            for (int j = 0; j < n; j++)
            {
                for (int i = 0; i < n; i++)
                {
                    var p = new Vector3(-h + (i + 0.5f) * cell, 0f, -h + (j + 0.5f) * cell);
                    var t = TileAt(w, terrains, p);
                    px[j * n + i] = new Color(t.SampleHeight(p) + t.transform.position.y, 0f, 0f, 1f);
                }
            }
            tex.SetPixels(px);
            tex.Apply(false, false);
            AssetDatabase.CreateAsset(tex, gen + "/Materials/WaterGround.asset");
            return tex;
        }

        static Material WaterMaterial(string gen, string name, WaterStyle s, Texture2D normal, WorldData w, Texture2D ground)
        {
            var custom = Shader.Find("WorldBuilder/Water");
            if (custom != null)
            {
                var wm = new Material(custom) { name = name };
                wm.SetColor("_Shallow", Rgb(s.shallow));
                wm.SetColor("_Deep", Rgb(s.deep));
                wm.SetFloat("_Clarity", Mathf.Max(s.clarity, 0.05f));
                wm.SetFloat("_Transparency", Mathf.Clamp01(s.transparency));
                wm.SetFloat("_Roughness", Mathf.Clamp01(s.roughness));
                wm.SetFloat("_ShoreFoam", s.shoreFoam);
                if (s.extinct != null && s.extinct.Length >= 3)
                {
                    wm.SetVector("_Extinct", new Vector4(s.extinct[0], s.extinct[1], s.extinct[2], 0f));
                    wm.SetVector("_Inscat", new Vector4(s.inscat[0], s.inscat[1], s.inscat[2], 0f));
                }
                if (normal != null) wm.SetTexture("_BumpMap", normal);
                if (ground != null)
                {
                    wm.SetTexture("_Ground", ground);
                    wm.SetVector("_GroundRect", new Vector4(-w.size * 0.5f, -w.size * 0.5f, 1f / w.size, 1f));
                }
                var l = w.light;
                if (l != null)
                {
                    var toSun = new Vector3(l.sunDirection[0], l.sunDirection[1], l.sunDirection[2]).normalized;
                    wm.SetVector("_SunDir", toSun);
                    wm.SetColor("_SunColor", Rgb(l.sunColor) * l.intensity);
                    if (l.skyTop != null && l.skyTop.Length >= 3)
                    {
                        wm.SetColor("_SkyTop", Rgb(l.skyTop));
                        wm.SetColor("_SkyHorizon", Rgb(l.skyHorizon));
                    }
                    if (l.waterSun != null && l.waterSun.Length >= 3)
                    {
                        wm.SetVector("_SunDisp", new Vector4(l.waterSun[0], l.waterSun[1], l.waterSun[2], 0f));
                        wm.SetVector("_SkyDisp", new Vector4(l.waterSky[0], l.waterSky[1], l.waterSky[2], 0f));
                        wm.SetFloat("_Exposure", Mathf.Pow(2f, l.postExposure));
                    }
                    if (l.hazeColor != null && l.hazeColor.Length >= 3)
                    {
                        wm.SetColor("_FogColor", Rgb(l.hazeColor));
                        wm.SetFloat("_FogDensity", l.hazeDensity);
                    }
                }
                AssetDatabase.CreateAsset(wm, gen + "/Materials/" + name + ".mat");
                return wm;
            }
            bool urp = GraphicsSettings.currentRenderPipeline != null;
            var shader = Shader.Find(urp ? "Universal Render Pipeline/Lit" : "Standard");
            var m = new Material(shader) { name = name };
            var c = new Color(s.shallow[0], s.shallow[1], s.shallow[2], Mathf.Lerp(1f, 0.55f, s.transparency));
            var smooth = Mathf.Clamp01(1f - s.roughness * 0.5f);
            m.SetOverrideTag("RenderType", "Transparent");
            m.SetInt("_ZWrite", 0);
            m.renderQueue = (int)RenderQueue.Transparent;
            if (urp)
            {
                m.SetFloat("_Surface", 1f);
                m.SetFloat("_Blend", 0f);
                m.SetColor("_BaseColor", c);
                m.SetFloat("_Smoothness", smooth);
                m.SetInt("_SrcBlend", (int)BlendMode.SrcAlpha);
                m.SetInt("_DstBlend", (int)BlendMode.OneMinusSrcAlpha);
                m.EnableKeyword("_SURFACE_TYPE_TRANSPARENT");
            }
            else
            {
                m.SetFloat("_Mode", 3f);
                m.SetColor("_Color", c);
                m.SetFloat("_Glossiness", smooth);
                m.SetInt("_SrcBlend", (int)BlendMode.One);
                m.SetInt("_DstBlend", (int)BlendMode.OneMinusSrcAlpha);
                m.EnableKeyword("_ALPHAPREMULTIPLY_ON");
            }
            if (normal != null)
            {
                m.SetTexture("_BumpMap", normal);
                m.SetTextureScale(urp ? "_BaseMap" : "_MainTex", new Vector2(8f, 8f));
                m.EnableKeyword("_NORMALMAP");
            }
            AssetDatabase.CreateAsset(m, gen + "/Materials/" + name + ".mat");
            return m;
        }

        static void BuildWater(WorldData w, string folder, string gen, Terrain[] terrains, List<string> report)
        {
            if (w.water == null || !w.water.enabled) return;
            var normal = string.IsNullOrEmpty(w.water.normalMap) ? null : Texture(folder + "/" + w.water.normalMap, true, true);
            var ground = terrains != null && terrains.Length > 0 && Shader.Find("WorldBuilder/Water") != null
                ? GroundHeights(w, terrains, gen) : null;
            var root = new GameObject("Water");
            int bodies = 0;
            if (w.water.ocean)
            {
                var plane = GameObject.CreatePrimitive(PrimitiveType.Plane);
                plane.name = "Ocean";
                Object.DestroyImmediate(plane.GetComponent<Collider>());
                plane.transform.SetParent(root.transform, false);
                plane.transform.position = new Vector3(0f, w.water.seaLevel, 0f);
                plane.transform.localScale = new Vector3(w.size * 0.4f, 1f, w.size * 0.4f);
                var r = plane.GetComponent<MeshRenderer>();
                r.sharedMaterial = WaterMaterial(gen, "Water_Ocean", w.water.oceanStyle, normal, w, ground);
                r.shadowCastingMode = ShadowCastingMode.Off;
                bodies++;
            }
            if (!string.IsNullOrEmpty(w.water.mesh))
            {
                var asset = AssetDatabase.LoadAssetAtPath<GameObject>(folder + "/" + w.water.mesh);
                if (asset != null)
                {
                    var go = (GameObject)PrefabUtility.InstantiatePrefab(asset, root.transform);
                    foreach (var b in w.water.bodies)
                    {
                        var t = FindDeep(go.transform, b.node);
                        var r = t != null ? t.GetComponentInChildren<MeshRenderer>() : null;
                        if (r == null) continue;
                        r.sharedMaterial = WaterMaterial(gen, "Water_" + b.node, b.style, normal, w, ground);
                        r.shadowCastingMode = ShadowCastingMode.Off;
                        bodies++;
                    }
                }
            }
            report.Add(bodies + " water surfaces");
            // swim volumes: trigger boxes on the Water layer, from the bed up to the surface
            if (w.water.volumes != null && w.water.volumes.Length > 0)
            {
                var vroot = new GameObject("Water Volumes");
                vroot.transform.SetParent(root.transform, false);
                int layer = LayerMask.NameToLayer("Water");
                foreach (var v in w.water.volumes)
                {
                    var go = new GameObject(v.kind + ": " + v.name);
                    go.transform.SetParent(vroot.transform, false);
                    go.transform.SetPositionAndRotation(new Vector3(v.center[0], v.center[1], v.center[2]), Quaternion.Euler(0f, v.yawDegrees, 0f));
                    if (layer >= 0) go.layer = layer;
                    var box = go.AddComponent<BoxCollider>();
                    box.isTrigger = true;
                    box.size = new Vector3(v.size[0], v.size[1], v.size[2]);
                    var wv = go.AddComponent<WorldBuilderWaterVolume>();
                    wv.kind = v.kind;
                    wv.surfaceHeight = v.surface;
                }
                report.Add(w.water.volumes.Length + " water volumes");
            }
        }

        static void BuildCaves(WorldData w, string folder, List<string> report)
        {
            if (string.IsNullOrEmpty(w.caves)) return;
            var asset = AssetDatabase.LoadAssetAtPath<GameObject>(folder + "/" + w.caves);
            if (asset == null) return;
            var go = (GameObject)PrefabUtility.InstantiatePrefab(asset);
            go.name = "Caves";
            // rock, floors and timbers collide; still pools and cobwebs don't
            foreach (var mf in go.GetComponentsInChildren<MeshFilter>())
                if (!mf.gameObject.name.EndsWith("_water_webs"))
                    mf.gameObject.AddComponent<MeshCollider>().sharedMesh = mf.sharedMesh;
            GameObjectUtility.SetStaticEditorFlags(go, StaticEditorFlags.ContributeGI | StaticEditorFlags.OccluderStatic | StaticEditorFlags.BatchingStatic);
            report.Add(go.GetComponentsInChildren<MeshRenderer>().Length + " cave meshes");
            // the glowing crystals
            if (w.caveLights != null && w.caveLights.Length > 0)
            {
                var lights = new GameObject("Crystal Lights");
                lights.transform.SetParent(go.transform, false);
                foreach (var l in w.caveLights)
                {
                    var lg = new GameObject("Crystal Light");
                    lg.transform.SetParent(lights.transform, false);
                    lg.transform.position = new Vector3(l.position[0], l.position[1], l.position[2]);
                    var light = lg.AddComponent<Light>();
                    light.type = LightType.Point;
                    light.color = Rgb(l.color);
                    light.intensity = l.intensity;
                    light.range = l.range;
                    light.shadows = LightShadows.None;
                    light.lightmapBakeType = LightmapBakeType.Mixed;
                }
                report.Add(w.caveLights.Length + " crystal lights");
            }
        }

        static void BuildRoads(WorldData w, string folder, List<string> report)
        {
            if (string.IsNullOrEmpty(w.roads)) return;
            var asset = AssetDatabase.LoadAssetAtPath<GameObject>(folder + "/" + w.roads);
            if (asset == null) return;
            var go = (GameObject)PrefabUtility.InstantiatePrefab(asset);
            go.name = "Roads";
            report.Add(go.GetComponentsInChildren<MeshRenderer>().Length + " road meshes");
        }

        static Color Rgb(float[] c, float a = 1f)
        {
            return c != null && c.Length >= 3 ? new Color(c[0], c[1], c[2], a) : Color.white;
        }

        static void BuildLighting(WorldData w, string gen)
        {
            var l = w.light;
            var sun = new GameObject("Sun");
            var light = sun.AddComponent<Light>();
            light.type = LightType.Directional;
            light.color = Rgb(l.sunColor);
            light.intensity = l.intensity;
            light.shadows = LightShadows.Soft;
            var toSun = new Vector3(l.sunDirection[0], l.sunDirection[1], l.sunDirection[2]);
            sun.transform.rotation = Quaternion.LookRotation(-toSun.normalized);
            RenderSettings.sun = light;
            var ownSky = Shader.Find("WorldBuilder/Sky");
            var skyShader = Shader.Find("Skybox/Procedural");
            if (ownSky != null && l.skyTop != null && l.skyTop.Length >= 3)
            {
                // the editor's sky colours for the hour, as calibrated for the Godot export
                var sky = new Material(ownSky) { name = "Sky" };
                sky.SetColor("_SkyTop", Rgb(l.skyTop));
                sky.SetColor("_SkyHorizon", Rgb(l.skyHorizon));
                sky.SetVector("_SunDir", toSun.normalized);
                sky.SetColor("_SunColor", Rgb(l.sunColor) * Mathf.Clamp01(l.intensity));
                AssetDatabase.CreateAsset(sky, gen + "/Materials/Sky.mat");
                RenderSettings.skybox = sky;
            }
            else if (skyShader != null)
            {
                var sky = new Material(skyShader);
                sky.SetFloat("_SunSize", 0.04f);
                sky.SetFloat("_AtmosphereThickness", 1f + l.haze);
                sky.SetFloat("_Exposure", 1.25f * Mathf.Pow(2f, l.exposure));
                AssetDatabase.CreateAsset(sky, gen + "/Materials/Sky.mat");
                RenderSettings.skybox = sky;
            }
            RenderSettings.ambientMode = AmbientMode.Skybox;
            if (l.hazeColor != null && l.hazeColor.Length >= 3)
            {
                // the editor's distance haze (aerial perspective, haze amount, colour haze) as one
                // exponential fog, as the Godot export calibrates it
                RenderSettings.fog = l.hazeDensity > 0f;
                RenderSettings.fogMode = FogMode.Exponential;
                RenderSettings.fogDensity = l.hazeDensity;
                RenderSettings.fogColor = Rgb(l.hazeColor);
            }
            else
            {
                RenderSettings.fog = l.fog;
                RenderSettings.fogMode = FogMode.ExponentialSquared;
                RenderSettings.fogDensity = l.fogDensity;
                RenderSettings.fogColor = Rgb(l.fogColor);
            }
            DynamicGI.UpdateEnvironment();
        }

        /// The world's art style in a URP project: a global Volume with the editor's tone curve
        /// (Tonemapping), exposure, bloom, vignette and its post-tonemap grade (saturation, contrast,
        /// split toning, lift, foliage hue, vibrance) as a Color Lookup LUT. Built by name, so the
        /// importer still compiles in built-in pipeline projects (where it does nothing).
        static void BuildLook(WorldData w, string folder, string gen, Camera cam)
        {
            var l = w.light;
            if (l == null || string.IsNullOrEmpty(l.tonemap) || GraphicsSettings.currentRenderPipeline == null) return;
            var volumeType = FindType("UnityEngine.Rendering.Volume");
            var profileType = FindType("UnityEngine.Rendering.VolumeProfile");
            if (volumeType == null || profileType == null) return;
            var profile = ScriptableObject.CreateInstance(profileType);
            profile.name = "Look";
            AssetDatabase.CreateAsset(profile, gen + "/Look.asset");
            var add = profileType.GetMethod("Add", new[] { typeof(Type), typeof(bool) });
            void Component(string type, params (string field, object value)[] sets)
            {
                var t = FindType("UnityEngine.Rendering.Universal." + type);
                if (t == null || add == null) return;
                var comp = (ScriptableObject)add.Invoke(profile, new object[] { t, false });
                comp.name = type;
                foreach (var (field, value) in sets)
                {
                    var p = t.GetField(field)?.GetValue(comp);
                    var valueProp = p?.GetType().GetProperty("value");
                    if (valueProp == null) continue;
                    var v = value is string s && valueProp.PropertyType.IsEnum ? Enum.Parse(valueProp.PropertyType, s) : value;
                    valueProp.SetValue(p, v);
                    p.GetType().GetProperty("overrideState")?.SetValue(p, true);
                }
                AssetDatabase.AddObjectToAsset(comp, profile);
            }
            Component("Tonemapping", ("mode", l.tonemap));
            Component("ColorAdjustments", ("postExposure", l.postExposure));
            if (l.bloom > 0f) Component("Bloom", ("intensity", l.bloom), ("threshold", 1f));
            if (l.vignette > 0f) Component("Vignette", ("intensity", l.vignette), ("smoothness", 0.4f));
            if (!string.IsNullOrEmpty(l.lut))
            {
                var path = folder + "/" + l.lut;
                if (AssetImporter.GetAtPath(path) is TextureImporter imp)
                {
                    imp.textureType = TextureImporterType.Default;
                    imp.sRGBTexture = false;
                    imp.mipmapEnabled = false;
                    imp.textureCompression = TextureImporterCompression.Uncompressed;
                    imp.wrapMode = TextureWrapMode.Clamp;
                    imp.filterMode = FilterMode.Bilinear;
                    imp.npotScale = TextureImporterNPOTScale.None;
                    imp.SaveAndReimport();
                }
                var lut = AssetDatabase.LoadAssetAtPath<Texture2D>(path);
                if (lut != null) Component("ColorLookup", ("texture", lut), ("contribution", 1f));
            }
            EditorUtility.SetDirty(profile);
            var go = new GameObject("Look");
            var volume = go.AddComponent(volumeType);
            volumeType.GetProperty("isGlobal")?.SetValue(volume, true);
            volumeType.GetField("isGlobal")?.SetValue(volume, true);
            volumeType.GetField("sharedProfile")?.SetValue(volume, profile);
            // URP cameras render post-processing only when asked to
            var camData = FindType("UnityEngine.Rendering.Universal.UniversalAdditionalCameraData");
            if (cam != null && camData != null)
            {
                var data = cam.GetComponent(camData) ?? cam.gameObject.AddComponent(camData);
                camData.GetProperty("renderPostProcessing")?.SetValue(data, true);
            }
        }

        static Type FindType(string fullName)
        {
            foreach (var a in AppDomain.CurrentDomain.GetAssemblies())
            {
                var t = a.GetType(fullName, false);
                if (t != null) return t;
            }
            return null;
        }

        static void BuildMarkers(WorldData w, List<string> report)
        {
            if (w.markers == null || w.markers.Length == 0) return;
            var root = new GameObject("Gameplay");
            foreach (var m in w.markers)
            {
                var go = new GameObject(m.kind + ": " + m.name);
                go.transform.SetParent(root.transform, false);
                go.transform.SetPositionAndRotation(new Vector3(m.position[0], m.position[1], m.position[2]), Quaternion.Euler(0f, m.yawDegrees, 0f));
                var c = go.AddComponent<WorldBuilderMarker>();
                c.kind = m.kind;
                c.id = m.id;
                c.displayName = m.name;
                c.type = m.type;
                c.radius = m.radius;
                c.notes = m.notes;
                var poly = new List<Vector2>();
                for (int i = 0; i + 1 < (m.polygon?.Length ?? 0); i += 2) poly.Add(new Vector2(m.polygon[i], m.polygon[i + 1]));
                c.polygon = poly.ToArray();
            }
            report.Add(w.markers.Length + " gameplay markers");
        }

        /// Unity's own navmesh baked from the colliders (terrain, trunks, rocks, buildings' collision
        /// proxies, caves; not the water volumes). With the AI Navigation package
        /// (com.unity.ai.navigation) it is a NavMeshSurface you can rebake; without it the core
        /// builder's NavMeshData, loaded by a WorldBuilderNavMesh component. `-wbNoNavMesh` skips it.
        static void BuildNavMesh(WorldData w, string gen, List<string> report)
        {
            if (w.navMesh == null || !w.navMesh.build || Environment.GetCommandLineArgs().Contains("-wbNoNavMesh")) return;
            var t0 = DateTime.Now;
            var settings = UnityEngine.AI.NavMesh.GetSettingsByID(0);
            settings.agentRadius = w.navMesh.agentRadius;
            settings.agentHeight = w.navMesh.agentHeight;
            settings.agentSlope = w.navMesh.maxSlope;
            settings.agentClimb = w.navMesh.stepHeight;
            settings.overrideVoxelSize = true;
            settings.voxelSize = w.navMesh.voxelSize;
            settings.overrideTileSize = true;
            settings.tileSize = 256;
            int water = LayerMask.NameToLayer("Water");
            int mask = water >= 0 ? ~(1 << water) : ~0;
            var bounds = new Bounds(new Vector3(0f, (w.heightMin + w.heightMax) * 0.5f, 0f), new Vector3(w.size, w.heightMax - w.heightMin + 200f, w.size));
            var go = new GameObject("NavMesh");
            UnityEngine.AI.NavMeshData data = null;
            var surfaceType = Type.GetType("Unity.AI.Navigation.NavMeshSurface, Unity.AI.Navigation");
            if (surfaceType != null)
            {
                var surface = go.AddComponent(surfaceType);
                void Set(string name, object value)
                {
                    var p = surfaceType.GetProperty(name);
                    if (p != null && p.CanWrite) p.SetValue(surface, p.PropertyType.IsEnum ? Enum.ToObject(p.PropertyType, value) : value);
                }
                Set("agentTypeID", settings.agentTypeID);
                Set("collectObjects", 0); // All
                Set("useGeometry", (int)UnityEngine.AI.NavMeshCollectGeometry.PhysicsColliders);
                Set("layerMask", (LayerMask)mask);
                Set("overrideVoxelSize", true);
                Set("voxelSize", w.navMesh.voxelSize);
                Set("overrideTileSize", true);
                Set("tileSize", 256);
                surfaceType.GetMethod("BuildNavMesh")?.Invoke(surface, null);
                data = surfaceType.GetProperty("navMeshData")?.GetValue(surface) as UnityEngine.AI.NavMeshData;
            }
            else
            {
                var sources = new List<UnityEngine.AI.NavMeshBuildSource>();
                UnityEngine.AI.NavMeshBuilder.CollectSources(bounds, mask, UnityEngine.AI.NavMeshCollectGeometry.PhysicsColliders, 0, new List<UnityEngine.AI.NavMeshBuildMarkup>(), sources);
                data = UnityEngine.AI.NavMeshBuilder.BuildNavMeshData(settings, sources, bounds, Vector3.zero, Quaternion.identity);
                var loader = go.AddComponent<WorldBuilderNavMesh>();
                loader.data = data;
                // (OnEnable ran before the data was set)
                loader.enabled = false;
                loader.enabled = true;
            }
            if (data == null)
            {
                report.Add("navmesh not built");
                return;
            }
            AssetDatabase.CreateAsset(data, gen + "/NavMesh.asset");
            var tri = UnityEngine.AI.NavMesh.CalculateTriangulation();
            report.Add("navmesh " + tri.indices.Length / 3 + " triangles in " + (DateTime.Now - t0).TotalSeconds.ToString("0") + " s" + (surfaceType != null ? " (NavMeshSurface)" : ""));
        }

        // ------------------------------------------------------------------ dungeons

        static Vector3 V(float[] a) => a != null && a.Length >= 3 ? new Vector3(a[0], a[1], a[2]) : Vector3.zero;
        static Quaternion Q(float[] a) => a != null && a.Length >= 4 ? new Quaternion(a[0], a[1], a[2], a[3]) : Quaternion.identity;

        static Quaternion Facing(Vector3 dir)
        {
            if (dir.sqrMagnitude < 1e-6f) return Quaternion.identity;
            return Quaternion.LookRotation(dir, Mathf.Abs(dir.normalized.y) > 0.99f ? Vector3.forward : Vector3.up);
        }

        /// The world's ways into its dungeons: their glowing sheets and a portal trigger each.
        static void BuildWaysIn(WorldData w, string folder, List<string> report)
        {
            if (w.waysIn == null || w.waysIn.Length == 0) return;
            var root = new GameObject("Dungeon Ways In");
            if (!string.IsNullOrEmpty(w.portalSheets))
            {
                var sheets = AssetDatabase.LoadAssetAtPath<GameObject>(folder + "/" + w.portalSheets);
                if (sheets != null)
                {
                    var go = (GameObject)PrefabUtility.InstantiatePrefab(sheets);
                    go.name = "Portal Sheets";
                    go.transform.SetParent(root.transform, false);
                }
            }
            foreach (var p in w.waysIn)
            {
                var go = new GameObject("Way In " + p.label);
                go.transform.SetParent(root.transform, false);
                go.transform.SetPositionAndRotation(V(p.center), Q(p.rotation));
                var portal = go.AddComponent<WorldBuilderPortal>();
                portal.targetScene = p.scene;
                portal.via = p.via;
                portal.halfSize = V(p.halfSize);
                if (p.arrive != null)
                {
                    portal.arrivePosition = V(p.arrive.position);
                    portal.arriveLook = V(p.arrive.look);
                }
            }
            report.Add(w.waysIn.Length + " dungeon ways in");
        }

        /// A scene per dungeon, saved next to the world's: its architecture with mesh colliders,
        /// lights (fires flicker), objects, mood, the way back out and a walking camera.
        /// Fires: AetherFX's flames baked to flipbooks, one particle each turning to the camera about
        /// the upright, its sheet animating from a random frame; `lights`: the ones with a light of
        /// their own get a flickering point light.
        static void BuildFires(DungeonFire[] list, WorldData w, string folder, bool lights)
        {
            if (list == null || list.Length == 0) return;
            var fires = new GameObject("Fires");
            var fireMats = new Dictionary<string, Material>();
            int k = 0;
            foreach (var f in list)
            {
                k++;
                var sheet = (w.flames ?? new FlameSheet[0]).FirstOrDefault(s => s.fire == f.fire);
                if (sheet == null || f.size == null || f.size.Length < 2) continue;
                var key = f.fire + string.Join(",", (f.color ?? new float[0]).Select(c => c.ToString("0.00")));
                if (!fireMats.TryGetValue(key, out var mat))
                {
                    mat = FlameMaterial(folder, sheet, f.color);
                    fireMats[key] = mat;
                }
                if (mat == null) continue;
                var go = new GameObject(f.fire + " fire");
                go.transform.SetParent(fires.transform, false);
                go.transform.position = V(f.position);
                var ps = go.AddComponent<ParticleSystem>();
                ps.Stop(true, ParticleSystemStopBehavior.StopEmittingAndClear);
                var main = ps.main;
                main.loop = true;
                main.duration = 1f;
                main.startLifetime = 3600f;
                main.startSpeed = 0f;
                main.startSize3D = true;
                main.startSizeX = f.size[0];
                main.startSizeY = f.size[1];
                main.startSizeZ = 1f;
                main.maxParticles = 1;
                main.simulationSpace = ParticleSystemSimulationSpace.Local;
                main.playOnAwake = true;
                var em = ps.emission;
                em.rateOverTime = 0f;
                em.SetBursts(new[] { new ParticleSystem.Burst(0f, 1) });
                var shape = ps.shape;
                shape.enabled = false;
                var tsa = ps.textureSheetAnimation;
                tsa.enabled = true;
                tsa.mode = ParticleSystemAnimationMode.Grid;
                tsa.numTilesX = sheet.columns;
                tsa.numTilesY = sheet.rows;
                tsa.animation = ParticleSystemAnimationType.WholeSheet;
                tsa.cycleCount = Mathf.Max(1, Mathf.RoundToInt(3600f * sheet.fps / Mathf.Max(sheet.frames, 1)));
                tsa.startFrame = new ParticleSystem.MinMaxCurve(0f, sheet.frames - 1);
                var pr = go.GetComponent<ParticleSystemRenderer>();
                pr.renderMode = ParticleSystemRenderMode.VerticalBillboard;
                pr.sharedMaterial = mat;
                pr.shadowCastingMode = ShadowCastingMode.Off;
                pr.receiveShadows = false;
                if (lights && f.light)
                {
                    var lg = new GameObject(f.fire + " light");
                    lg.transform.SetParent(fires.transform, false);
                    lg.transform.position = V(f.lightPosition);
                    var light = lg.AddComponent<Light>();
                    light.type = LightType.Point;
                    light.color = new Color(1f, 0.58f, 0.26f);
                    light.intensity = f.lightIntensity;
                    light.range = f.lightRange;
                    light.shadows = LightShadows.None;
                    var fl = lg.AddComponent<WorldBuilderFlicker>();
                    fl.baseIntensity = f.lightIntensity;
                    fl.amount = 0.4f;
                    fl.seed = k;
                }
            }
        }

        /// The placed models' light fixtures (downlights, lanterns, street lights): a point or spot
        /// light each under "Fixture Lights".
        static void BuildFixtureLights(WorldData w, List<string> report)
        {
            if (w.fixtureLights == null || w.fixtureLights.Length == 0) return;
            var root = new GameObject("Fixture Lights");
            foreach (var l in w.fixtureLights)
            {
                var lg = new GameObject(l.spot ? "Spot Light" : "Point Light");
                lg.transform.SetParent(root.transform, false);
                lg.transform.position = V(l.position);
                var light = lg.AddComponent<Light>();
                light.type = l.spot ? LightType.Spot : LightType.Point;
                if (l.spot)
                {
                    var d = V(l.direction);
                    if (d.sqrMagnitude > 1e-6f)
                        lg.transform.rotation = Quaternion.LookRotation(d, Mathf.Abs(d.normalized.y) > 0.98f ? Vector3.forward : Vector3.up);
                    light.spotAngle = l.angle;
                }
                light.color = Rgb(l.color);
                light.intensity = l.intensity;
                light.range = l.range;
                light.shadows = l.shadows ? LightShadows.Soft : LightShadows.None;
            }
            report.Add(w.fixtureLights.Length + " fixture lights");
        }

        /// A fire's material: its flipbook sheet (premultiplied colour), drawn additively and unlit
        /// in either pipeline; saved next to the sheet.
        static Material FlameMaterial(string folder, FlameSheet sheet, float[] color)
        {
            var tex = AssetDatabase.LoadAssetAtPath<Texture2D>(folder + "/" + sheet.file);
            if (tex == null) return null;
            // one per fire and colour, shared by the world's scene and the dungeons'
            var tag = color != null && color.Length >= 3 ? string.Join("_", color.Take(3).Select(c => Mathf.RoundToInt(c * 100f))) : "fire";
            var path = Path.GetDirectoryName(folder + "/" + sheet.file).Replace('\\', '/') + "/" + sheet.fire + "_" + tag + ".mat";
            var have = AssetDatabase.LoadAssetAtPath<Material>(path);
            if (have != null) return have;
            bool urp = GraphicsSettings.currentRenderPipeline != null;
            var shader = Shader.Find(urp ? "Universal Render Pipeline/Particles/Unlit" : "Legacy Shaders/Particles/Additive");
            if (shader == null) shader = Shader.Find("Particles/Standard Unlit");
            if (shader == null) return null;
            var mat = new Material(shader) { name = "Fire " + sheet.fire };
            var tint = color != null && color.Length >= 3 ? new Color(color[0], color[1], color[2], 1f) : Color.white;
            mat.mainTexture = tex;
            if (urp)
            {
                mat.SetTexture("_BaseMap", tex);
                mat.SetColor("_BaseColor", tint * 2.2f);
                mat.SetFloat("_Surface", 1f);
                mat.SetFloat("_Blend", 2f);
                mat.SetFloat("_SrcBlend", (float)BlendMode.One);
                mat.SetFloat("_DstBlend", (float)BlendMode.One);
                mat.SetFloat("_ZWrite", 0f);
                mat.EnableKeyword("_SURFACE_TYPE_TRANSPARENT");
                mat.EnableKeyword("_BLENDMODE_ADD");
                mat.renderQueue = (int)RenderQueue.Transparent;
            }
            else if (mat.HasProperty("_TintColor"))
            {
                mat.SetColor("_TintColor", tint * 0.5f);
            }
            AssetDatabase.CreateAsset(mat, path);
            return mat;
        }

        static List<string> BuildDungeons(WorldData w, string folder, string worldScene, Dictionary<string, List<Variant>> prefabs, List<string> report)
        {
            var scenes = new List<string>();
            if (w.dungeons == null) return scenes;
            foreach (var d in w.dungeons)
            {
                Step("Dungeon " + d.name, 0.98f);
                var asset = AssetDatabase.LoadAssetAtPath<GameObject>(folder + "/" + d.glb);
                if (asset == null)
                {
                    report.Add("dungeon " + d.name + ": " + d.glb + " isn't imported");
                    continue;
                }
                var scene = EditorSceneManager.NewScene(NewSceneSetup.EmptyScene, NewSceneMode.Single);
                var arch = (GameObject)PrefabUtility.InstantiatePrefab(asset);
                arch.name = d.name;
                // (doors, gates, levers and traps move: not static, a box collider of their own)
                var piecesNode = d.safeName + "_pieces";
                bool InPieces(Transform t)
                {
                    for (var x = t; x != null; x = x.parent)
                        if (x.name == piecesNode) return true;
                    return false;
                }
                // walls, floors, stairs and ceilings collide; liquids and the way out's sheet don't
                foreach (var mf in arch.GetComponentsInChildren<MeshFilter>())
                    if (!mf.gameObject.name.EndsWith("_liquids") && !mf.gameObject.name.EndsWith("_exit") && !InPieces(mf.transform))
                        mf.gameObject.AddComponent<MeshCollider>().sharedMesh = mf.sharedMesh;
                foreach (var t in arch.GetComponentsInChildren<Transform>())
                    if (!InPieces(t))
                        GameObjectUtility.SetStaticEditorFlags(t.gameObject, StaticEditorFlags.ContributeGI | StaticEditorFlags.OccluderStatic | StaticEditorFlags.OccludeeStatic | StaticEditorFlags.BatchingStatic | NavStatic);

                // doors, gates, levers, plates and traps come alive; the game's markers (arenas,
                // checkpoints, loot and spawn spots) are WorldBuilderMarkers
                var made = new Dictionary<int, WorldBuilderPiece>();
                GameObject markers = null;
                foreach (var p in d.pieces ?? new DungeonPiece[0])
                {
                    if (p.marker)
                    {
                        if (markers == null) markers = new GameObject("Markers");
                        var mg = new GameObject(p.name + " " + p.id);
                        mg.transform.SetParent(markers.transform, false);
                        mg.transform.SetPositionAndRotation(V(p.position), Q(p.rotation));
                        var mk = mg.AddComponent<WorldBuilderMarker>();
                        mk.kind = p.kind;
                        mk.id = p.id;
                        mk.displayName = p.name;
                        mk.radius = Mathf.Max(p.width * 0.5f, 0.5f);
                        mk.notes = p.label;
                        continue;
                    }
                    var node = string.IsNullOrEmpty(p.node) ? null : FindDeep(arch.transform, p.node);
                    if (node == null)
                    {
                        report.Add("dungeon " + d.name + ": no node " + p.node + " for its " + p.name);
                        continue;
                    }
                    var c = node.gameObject.AddComponent<WorldBuilderPiece>();
                    c.kind = p.kind;
                    c.id = p.id;
                    c.startOpen = p.open;
                    c.blocks = p.blocks;
                    c.usable = p.usable;
                    c.size = new Vector3(p.width, Mathf.Max(p.height, 0.05f), p.blocks ? 0.24f : p.width);
                    c.seconds = p.seconds;
                    c.label = p.label;
                    var parts = p.parts ?? new PiecePart[0];
                    c.parts = parts.Select(x => FindDeep(node, x.node)).ToArray();
                    c.closedPosition = parts.Select(x => V(x.closed?.position)).ToArray();
                    c.closedRotation = parts.Select(x => Q(x.closed?.rotation)).ToArray();
                    c.openPosition = parts.Select(x => V(x.open?.position)).ToArray();
                    c.openRotation = parts.Select(x => Q(x.open?.rotation)).ToArray();
                    made[p.id] = c;
                }
                foreach (var p in d.pieces ?? new DungeonPiece[0])
                    if (made.TryGetValue(p.id, out var c) && p.links != null)
                        c.links = p.links.Where(made.ContainsKey).Select(i => made[i]).ToArray();

                // lights: the brightest few cast shadows, fires flicker
                var lights = new GameObject("Lights");
                int shadowed = 0;
                foreach (var l in (d.lights ?? new DungeonLight[0]).OrderByDescending(x => x.intensity))
                {
                    var lg = new GameObject(l.spot ? "Spot Light" : "Point Light");
                    lg.transform.SetParent(lights.transform, false);
                    lg.transform.position = V(l.position);
                    var light = lg.AddComponent<Light>();
                    light.type = l.spot ? LightType.Spot : LightType.Point;
                    if (l.spot)
                    {
                        lg.transform.rotation = Facing(V(l.direction));
                        light.spotAngle = Mathf.Clamp(l.spotAngle, 1f, 179f);
                    }
                    light.color = Rgb(l.color);
                    light.intensity = l.intensity;
                    light.range = l.range;
                    bool shadows = l.shadows && shadowed < 12;
                    light.shadows = shadows ? LightShadows.Soft : LightShadows.None;
                    if (shadows) shadowed++;
                    light.lightmapBakeType = LightmapBakeType.Mixed;
                    if (l.flicker > 0.01f)
                    {
                        var f = lg.AddComponent<WorldBuilderFlicker>();
                        f.baseIntensity = l.intensity;
                        f.amount = l.flicker;
                        f.seed = l.seed;
                    }
                }

                // light fixtures, dressing and the user's own objects
                var props = new GameObject("Objects");
                int count = 0;
                foreach (var o in d.objects ?? new DungeonObjects[0])
                {
                    if (o.data == null || !prefabs.TryGetValue(o.safeName, out var list) || list.Count == 0) continue;
                    var stretch = new Dictionary<int, Vector3>();
                    if (o.stretch != null)
                        for (int i = 0; i + 3 < o.stretch.Length; i += 4)
                            stretch[(int)o.stretch[i]] = new Vector3(o.stretch[i + 1], o.stretch[i + 2], o.stretch[i + 3]);
                    for (int i = 0; i + 8 < o.data.Length; i += 9)
                    {
                        var v = list[Mathf.Clamp((int)o.data[i], 0, list.Count - 1)];
                        var go = (GameObject)PrefabUtility.InstantiatePrefab(v.prefab, props.transform);
                        go.transform.SetPositionAndRotation(new Vector3(o.data[i + 1], o.data[i + 2], o.data[i + 3]), new Quaternion(o.data[i + 4], o.data[i + 5], o.data[i + 6], o.data[i + 7]));
                        go.transform.localScale = (stretch.TryGetValue(i / 9, out var sf) ? sf : Vector3.one) * o.data[i + 8];
                        GameObjectUtility.SetStaticEditorFlags(go, StaticEditorFlags.BatchingStatic | StaticEditorFlags.OccludeeStatic | NavStatic);
                        count++;
                    }
                }

                // the mood: dark, a flat fill, exponential fog; under a sky its sun or moon
                RenderSettings.skybox = null;
                if (d.sky != null && !string.IsNullOrEmpty(d.sky.mode) && d.sky.mode != "none")
                {
                    var sunGo = new GameObject("Sun");
                    var sun = sunGo.AddComponent<Light>();
                    sun.type = LightType.Directional;
                    sun.color = Rgb(d.sky.color);
                    sun.intensity = d.sky.intensity;
                    sun.shadows = LightShadows.Soft;
                    sunGo.transform.rotation = Facing(V(d.sky.lightDirection));
                    var sky = new Material(Shader.Find("Skybox/Procedural"));
                    sky.SetFloat("_Exposure", d.sky.mode == "night" ? 0.05f : d.sky.mode == "dusk" ? 0.6f : 1.2f);
                    RenderSettings.skybox = sky;
                    RenderSettings.sun = sun;
                }
                if (d.mood != null && d.mood.ambient != null)
                {
                    RenderSettings.ambientMode = AmbientMode.Flat;
                    RenderSettings.ambientLight = Rgb(d.mood.ambient) * d.mood.ambientIntensity;
                    RenderSettings.fog = true;
                    RenderSettings.fogMode = FogMode.Exponential;
                    RenderSettings.fogDensity = d.mood.fogDensity;
                    RenderSettings.fogColor = Rgb(d.mood.fogColor);
                }

                // the way back out: to the world, at the way in players came through
                if (d.exit != null && d.exit.halfSize != null && d.exit.halfSize.Length == 3)
                {
                    var ex = new GameObject("Way Out");
                    ex.transform.SetPositionAndRotation(V(d.exit.center), Q(d.exit.rotation));
                    var p = ex.AddComponent<WorldBuilderPortal>();
                    p.targetScene = Path.GetFileNameWithoutExtension(worldScene);
                    p.halfSize = V(d.exit.halfSize);
                    var arr = d.exit.arrivals ?? new LookData[0];
                    p.arrivalVia = arr.Select(a => a.via).ToArray();
                    p.arrivalPosition = arr.Select(a => V(a.position)).ToArray();
                    p.arrivalLook = arr.Select(a => V(a.look)).ToArray();
                    if (arr.Length > 0)
                    {
                        p.arrivePosition = p.arrivalPosition[0];
                        p.arriveLook = p.arrivalLook[0];
                    }
                }

                // the fires (their lights are the dungeon's)
                BuildFires(d.fires, w, folder, false);

                // a camera walking from the start
                var camGo = new GameObject("Main Camera");
                camGo.tag = "MainCamera";
                var cam = camGo.AddComponent<Camera>();
                cam.clearFlags = RenderSettings.skybox != null ? CameraClearFlags.Skybox : CameraClearFlags.SolidColor;
                cam.backgroundColor = Color.black;
                cam.fieldOfView = 60f;
                cam.nearClipPlane = 0.05f;
                cam.farClipPlane = 800f;
                camGo.AddComponent<AudioListener>();
                var fly = camGo.AddComponent<WorldBuilderFlyCamera>();
                fly.walking = true;
                fly.rayFromEye = true;
                fly.stopAtWalls = true;
                fly.flySpeed = 12f;
                if (d.start != null) camGo.transform.SetPositionAndRotation(V(d.start.position), Facing(V(d.start.look)));

                var path = folder + "/" + d.scene + ".unity";
                EditorSceneManager.SaveScene(scene, path);
                scenes.Add(path);
                report.Add("dungeon scene " + path + " (" + count + " objects, " + (d.lights?.Length ?? 0) + " lights)");
            }
            return scenes;
        }

        /// The world's scene and its dungeons' in the build settings (portals load them by name).
        static void AddToBuild(string worldScene, List<string> dungeons)
        {
            var list = EditorBuildSettings.scenes.ToList();
            foreach (var p in new[] { worldScene }.Concat(dungeons))
                if (!list.Any(s => s.path == p)) list.Add(new EditorBuildSettingsScene(p, true));
            EditorBuildSettings.scenes = list.ToArray();
        }

        static Camera BuildCamera(WorldData w)
        {
            var go = new GameObject("Main Camera");
            go.tag = "MainCamera";
            var cam = go.AddComponent<Camera>();
            cam.fieldOfView = 50f;
            cam.nearClipPlane = 0.1f;
            cam.farClipPlane = w.size * 3f;
            go.AddComponent<AudioListener>();
            go.AddComponent<WorldBuilderFlyCamera>();
            var p = new Vector3(w.camera.position[0], w.camera.position[1], w.camera.position[2]);
            var t = new Vector3(w.camera.target[0], w.camera.target[1], w.camera.target[2]);
            go.transform.SetPositionAndRotation(p, Quaternion.LookRotation((t - p).normalized, Vector3.up));
            return cam;
        }
    }

    // ---------------------------------------------------------------------- data (worldbuilder.json)

    [Serializable] public class WorldData
    {
        public int version;
        public string name;
        public string safeName;
        public float size;
        public float heightMin;
        public float heightMax;
        public float seaLevel;
        public int heightmapResolution;
        public int alphamapResolution;
        public int tilesPerSide;
        public float tileSize;
        public TileData[] tiles;
        public float cellSize;
        public LayerData[] layers;
        public SharedTexture[] textures;
        public float treeDistance;
        public ModelData[] models;
        public int detailResolution;
        public DetailData[] details;
        public WaterData water;
        public string roads;
        public string caves;
        public CaveLightData[] caveLights;
        public DungeonData[] dungeons;
        public WayInData[] waysIn;
        public string portalSheets;
        public FlameSheet[] flames;
        public DungeonFire[] fires;
        public FixtureLightData[] fixtureLights;
        public NavMeshSettingsData navMesh;
        public LightData light;
        public MarkerData[] markers;
        public CameraData camera;
    }

    [Serializable] public class SharedTexture
    {
        public string file;
        /// color, normal or data
        public string role;
    }

    [Serializable] public class TileData
    {
        public int x;
        public int z;
        /// the tile's corner (Unity space)
        public float[] position;
        public string heightmap;
        public string alphamaps;
        public string detailMaps;
        /// cave mouths: (row, column) pairs of the tile's cells
        public int[] holes;
    }

    [Serializable] public class LayerData
    {
        public string name;
        public string key;
        public string albedo;
        public string normal;
        public string mask;
        public float tile;
        public float smoothness;
    }

    [Serializable] public class ModelData
    {
        public string name;
        public string safeName;
        public string file;
        public string category;
        public bool generated;
        public bool tree;
        public bool detail;
        public bool shadows;
        public string collider;
        public bool lightmap;
        public PhysicsData physics;
        public VariantData[] variants;
        public float[] lodEnd;
        /// per instance: variant, x, y, z, qx, qy, qz, qw, scale (Unity space)
        public float[] data;
        /// instances scaled per axis: row, sx, sy, sz (factors on top of scale, model axes)
        public float[] stretch;
    }

    [Serializable] public class VariantData
    {
        public string node;
        public string[] lods;
        public float height;
        public float width;
        public float trunk;
        /// collision proxy node ("" = the render meshes)
        public string collision;
    }

    [Serializable] public class NavMeshSettingsData
    {
        public bool build;
        public float voxelSize;
        public float agentRadius;
        public float agentHeight;
        public float maxSlope;
        public float stepHeight;
    }

    [Serializable] public class DungeonData
    {
        public string name;
        public string safeName;
        public string scene;
        public string glb;
        public DungeonObjects[] objects;
        public DungeonLight[] lights;
        public DungeonMood mood;
        public DungeonSky sky;
        public LookData start;
        public DungeonExit exit;
        public DungeonPiece[] pieces;
        public DungeonFire[] fires;
    }

    [Serializable] public class DungeonFire
    {
        public string fire;
        public float[] position;
        public float[] size;
        public float[] color;
        public bool light;
        public float[] lightPosition;
        public float lightRange;
        public float lightIntensity;
    }

    [Serializable] public class FlameSheet
    {
        public string fire;
        public string file;
        public int columns;
        public int rows;
        public int frames;
        public float fps;
    }

    [Serializable] public class DungeonPiece
    {
        public int id;
        public string kind;
        public string name;
        public float[] position;
        public float[] rotation;
        public float width;
        public float height;
        public bool open;
        public int[] links;
        public string label;
        public bool marker;
        public bool usable;
        public bool blocks;
        public string node;
        public PiecePart[] parts;
        public float seconds;
    }

    [Serializable] public class PiecePart
    {
        public string node;
        public PiecePose closed;
        public PiecePose open;
    }

    [Serializable] public class PiecePose
    {
        public float[] position;
        public float[] rotation;
    }

    [Serializable] public class DungeonSky
    {
        public string mode;
        public float[] lightDirection;
        public float intensity;
        public float[] color;
    }

    [Serializable] public class DungeonObjects
    {
        public string safeName;
        public float[] data;
        public float[] stretch;
    }

    [Serializable] public class DungeonLight
    {
        public float[] position;
        public float[] direction;
        public float[] color;
        public float intensity;
        public float range;
        public bool spot;
        public float spotAngle;
        public bool shadows;
        public float flicker;
        public int seed;
    }

    [Serializable] public class DungeonMood
    {
        public float[] ambient;
        public float ambientIntensity;
        public float[] fogColor;
        public float fogDensity;
        public float exposure;
        public float haze;
    }

    [Serializable] public class LookData
    {
        public float[] position;
        public float[] look;
        public int via;
    }

    [Serializable] public class DungeonExit
    {
        public float[] center;
        public float[] rotation;
        public float[] halfSize;
        public LookData[] arrivals;
    }

    [Serializable] public class WayInData
    {
        public string scene;
        public int via;
        public string label;
        public float[] center;
        public float[] rotation;
        public float[] halfSize;
        public LookData arrive;
    }

    [Serializable] public class FixtureLightData
    {
        public float[] position;
        public bool spot;
        public float[] direction;
        public float angle;
        public float[] color;
        public float intensity;
        public float range;
        public bool shadows;
    }

    [Serializable] public class CaveLightData
    {
        public float[] position;
        public float[] color;
        public float intensity;
        public float range;
    }

    [Serializable] public class PhysicsData
    {
        public string surface;
        public float friction;
        public float bounciness;
    }

    [Serializable] public class WaterVolumeData
    {
        public string kind;
        public string name;
        public float[] center;
        public float[] size;
        public float yawDegrees;
        public float surface;
    }

    [Serializable] public class DetailData
    {
        public string model;
        public int variant;
        public float minScale;
        public float maxScale;
        public float align;
    }

    [Serializable] public class WaterStyle
    {
        public float[] shallow;
        public float[] deep;
        public float clarity;
        public float transparency;
        public float roughness;
        public float shoreFoam;
        /// the editor's optics: extinction per metre and in-scatter albedo (linear)
        public float[] extinct;
        public float[] inscat;
    }

    [Serializable] public class WaterBody
    {
        public string node;
        public string kind;
        public string name;
        public WaterStyle style;
    }

    [Serializable] public class WaterData
    {
        public bool enabled;
        public bool ocean;
        public float seaLevel;
        public WaterStyle oceanStyle;
        public string mesh;
        public string normalMap;
        public WaterBody[] bodies;
        public WaterVolumeData[] volumes;
    }

    [Serializable] public class LightData
    {
        public float hour;
        public float[] sunDirection;
        public float[] sunColor;
        public float intensity;
        public bool fog;
        public float fogDensity;
        public float[] fogColor;
        public float haze;
        public float exposure;
        public float contrast;
        public float saturation;
        /// URP TonemappingMode name ("ACES", "Neutral"); empty in older exports
        public string tonemap;
        /// URP post exposure (EV)
        public float postExposure;
        public float bloom;
        public float vignette;
        /// the post-tonemap grade as a URP Color Lookup strip (sRGB in, sRGB out)
        public string lut;
        /// exponential haze (per m) and its colour (sRGB)
        public float hazeDensity;
        public float[] hazeColor;
        /// sky colours for the water's reflections (sRGB)
        public float[] skyTop;
        public float[] skyHorizon;
        /// the key light and sky irradiance in the editor's display units (water optics)
        public float[] waterSun;
        public float[] waterSky;
    }

    [Serializable] public class MarkerData
    {
        public string kind;
        public int id;
        public string name;
        public string type;
        public float[] position;
        public float yawDegrees;
        public float radius;
        public string notes;
        public float[] polygon;
    }

    [Serializable] public class CameraData
    {
        public float[] position;
        public float[] target;
    }
}
