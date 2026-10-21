// A way between a Skyfold Studio world and its dungeons. Walking into this box (the main
// camera's position in this object's frame) loads `targetScene`. Players come out where they
// should: in a dungeon at its start; back in the world in front of the way in they came through
// (`arrivalVia` / `arrivalPosition` / `arrivalLook`), else at `arrivePosition`.
using UnityEngine;
using UnityEngine.SceneManagement;

namespace WorldBuilder
{
    public class WorldBuilderPortal : MonoBehaviour
    {
        public string targetScene;
        public Vector3 halfSize = new Vector3(1.6f, 2.2f, 0.6f);
        // the way in this is (a world's portal); 0 for a dungeon's way out
        public int via;
        // where players come out in the target (eye position) and the way they look
        public Vector3 arrivePosition;
        public Vector3 arriveLook = Vector3.forward;
        // a dungeon's way out: per way in, where players come back out in the world
        public int[] arrivalVia = new int[0];
        public Vector3[] arrivalPosition = new Vector3[0];
        public Vector3[] arrivalLook = new Vector3[0];

        static bool pending;
        static Vector3 pendingPosition, pendingLook;
        static bool pendingWalking;
        static int lastVia;
        bool armed;

        void Start()
        {
            // just came through a portal: put the camera where it should be (once per scene)
            if (!pending) return;
            pending = false;
            var cam = Camera.main;
            if (cam == null) return;
            cam.transform.position = pendingPosition;
            if (pendingLook.sqrMagnitude > 0.0001f) cam.transform.rotation = Quaternion.LookRotation(pendingLook, Vector3.up);
            var fly = cam.GetComponent<WorldBuilderFlyCamera>();
            if (fly != null)
            {
                fly.walking = pendingWalking;
                fly.SyncHeading();
            }
        }

        void Update()
        {
            var cam = Camera.main;
            if (cam == null || string.IsNullOrEmpty(targetScene)) return;
            var p = Quaternion.Inverse(transform.rotation) * (cam.transform.position - transform.position);
            bool inside = Mathf.Abs(p.x) <= halfSize.x && Mathf.Abs(p.y) <= halfSize.y && Mathf.Abs(p.z) <= halfSize.z;
            // (a camera that starts inside has to step out first)
            if (!armed)
            {
                armed = !inside;
                return;
            }
            if (inside) Go(cam);
        }

        void Go(Camera cam)
        {
            enabled = false;
            pendingPosition = arrivePosition;
            pendingLook = arriveLook;
            if (arrivalVia.Length > 0)
            {
                int k = System.Array.IndexOf(arrivalVia, lastVia);
                if (k < 0) k = 0;
                pendingPosition = arrivalPosition[k];
                pendingLook = arrivalLook[k];
            }
            if (via != 0) lastVia = via;
            var fly = cam.GetComponent<WorldBuilderFlyCamera>();
            pendingWalking = fly == null || fly.walking;
            pending = true;
            SceneManager.LoadScene(targetScene);
        }

        void OnDrawGizmos()
        {
            Gizmos.color = new Color(0.4f, 0.6f, 1f, 0.6f);
            Gizmos.matrix = Matrix4x4.TRS(transform.position, transform.rotation, Vector3.one);
            Gizmos.DrawWireCube(Vector3.zero, halfSize * 2f);
        }
    }
}
