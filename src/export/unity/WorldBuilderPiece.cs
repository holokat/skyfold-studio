// A door, double door, portcullis, set of bars, secret door, lever, pressure plate or spike trap
// from a Skyfold Studio dungeon, created by the Skyfold Studio importer on the piece's node of the
// dungeon's model. Its parts (the node's children) swing, slide or rise between their shut and
// open poses. On foot, E works the one looked at within reach; stepping on a plate works it; a
// lever or a plate works its links too. Shut doors, gates and bars block the way (bars always).
// `isOpen` is how it stands now; call Work() from your own game code to work it.

using System.Collections.Generic;
using UnityEngine;
#if ENABLE_INPUT_SYSTEM && !ENABLE_LEGACY_INPUT_MANAGER
using UnityEngine.InputSystem;
#endif

namespace WorldBuilder
{
    public class WorldBuilderPiece : MonoBehaviour
    {
        /// door, double_door, portcullis, bars, secret_door, lever, pressure_plate, spike_trap
        public string kind = "door";
        public int id;
        public bool startOpen;
        public bool blocks;
        public bool usable;
        /// width, height, depth (a plate's or trap's depth is its width)
        public Vector3 size = new Vector3(1.6f, 2.6f, 0.24f);
        public Transform[] parts = new Transform[0];
        public Vector3[] closedPosition = new Vector3[0];
        public Quaternion[] closedRotation = new Quaternion[0];
        public Vector3[] openPosition = new Vector3[0];
        public Quaternion[] openRotation = new Quaternion[0];
        /// what a lever or plate works
        public WorldBuilderPiece[] links = new WorldBuilderPiece[0];
        public float seconds = 0.9f;
        public string label;
        public bool isOpen;

        public event System.Action<bool> Worked;

        static readonly List<WorldBuilderPiece> all = new List<WorldBuilderPiece>();
        float t;
        BoxCollider box;
        bool onPlate;

        void OnEnable() => all.Add(this);
        void OnDisable() => all.Remove(this);

        void Start()
        {
            isOpen = startOpen;
            t = isOpen ? 1f : 0f;
            Pose();
            if (blocks)
            {
                box = gameObject.AddComponent<BoxCollider>();
                box.center = new Vector3(0f, size.y * 0.5f, 0f);
                box.size = size;
                box.enabled = !isOpen || kind == "bars";
            }
        }

        public void Work()
        {
            isOpen = !isOpen;
            Worked?.Invoke(isOpen);
            if (kind == "lever" || kind == "pressure_plate")
                foreach (var l in links)
                    if (l != null)
                    {
                        l.isOpen = !l.isOpen;
                        l.Worked?.Invoke(l.isOpen);
                    }
        }

        void Pose()
        {
            float k = t * t * (3f - 2f * t);
            for (int i = 0; i < parts.Length && i < openPosition.Length; i++)
                if (parts[i] != null)
                {
                    parts[i].localPosition = Vector3.Lerp(closedPosition[i], openPosition[i], k);
                    parts[i].localRotation = Quaternion.Slerp(closedRotation[i], openRotation[i], k);
                }
        }

#if ENABLE_INPUT_SYSTEM && !ENABLE_LEGACY_INPUT_MANAGER
        static bool UseKey() => Keyboard.current != null && Keyboard.current.eKey.wasPressedThisFrame;
#else
        static bool UseKey() => Input.GetKeyDown(KeyCode.E);
#endif

        /// How near the piece the eye is, when it's within reach and looked at (else infinity).
        public float Score(Transform eye)
        {
            if (!usable) return float.PositiveInfinity;
            var c = transform.TransformPoint(new Vector3(0f, Mathf.Min(size.y * 0.5f, 1.2f), 0f));
            var to = c - eye.position;
            float d = to.magnitude;
            if (d > 2.8f || Vector3.Dot(eye.forward, to / Mathf.Max(d, 0.001f)) < 0.55f) return float.PositiveInfinity;
            return d;
        }

        bool Focused(Transform eye)
        {
            float mine = Score(eye);
            if (float.IsInfinity(mine)) return false;
            foreach (var p in all)
                if (p != this && p.Score(eye) < mine) return false;
            return true;
        }

        void Update()
        {
            float target = isOpen ? 1f : 0f;
            if (t != target)
            {
                t = Mathf.MoveTowards(t, target, Time.deltaTime / Mathf.Max(seconds, 0.05f));
                Pose();
            }
            if (box != null) box.enabled = t <= 0.5f || kind == "bars";
            var cam = Camera.main;
            if (cam == null) return;
            var fly = cam.GetComponent<WorldBuilderFlyCamera>();
            bool onFoot = fly == null || fly.walking;
            if (kind == "pressure_plate")
            {
                float eye = fly != null ? fly.eyeHeight : 1.7f;
                var l = transform.InverseTransformPoint(cam.transform.position - Vector3.up * eye);
                bool on = onFoot && Mathf.Abs(l.x) < size.x * 0.5f && Mathf.Abs(l.z) < size.x * 0.5f && Mathf.Abs(l.y) < 0.6f;
                if (on && !onPlate) Work();
                onPlate = on;
            }
            if (onFoot && usable && UseKey() && Focused(cam.transform)) Work();
        }

        void OnGUI()
        {
            var cam = Camera.main;
            if (cam == null || !usable || !Focused(cam.transform)) return;
            var fly = cam.GetComponent<WorldBuilderFlyCamera>();
            if (fly != null && !fly.walking) return;
            string what = kind == "lever" ? (isOpen ? "Push the lever back" : "Pull the lever")
                : kind == "secret_door" ? (isOpen ? "Close the passage" : "Push the loose stone")
                : (isOpen ? "Close the " : "Open the ") + kind.Replace('_', ' ');
            var style = new GUIStyle(GUI.skin.label) { alignment = TextAnchor.MiddleCenter, fontSize = 16 };
            GUI.Label(new Rect(Screen.width * 0.5f - 200f, Screen.height * 0.5f + 46f, 400f, 30f), "[E]  " + what, style);
        }

        void OnDrawGizmosSelected()
        {
            Gizmos.color = new Color(1f, 0.62f, 0.2f);
            foreach (var l in links)
                if (l != null) Gizmos.DrawLine(transform.position + Vector3.up, l.transform.position + Vector3.up);
        }
    }
}
