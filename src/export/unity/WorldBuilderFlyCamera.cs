// Explore an imported Skyfold Studio world: WASD move, Q / E down / up, right mouse to look,
// Shift fast, F switches between flying and walking on the ground. Works with the old Input
// Manager and with the Input System package.

using UnityEngine;
#if ENABLE_INPUT_SYSTEM && !ENABLE_LEGACY_INPUT_MANAGER
using UnityEngine.InputSystem;
#endif

namespace WorldBuilder
{
    public class WorldBuilderFlyCamera : MonoBehaviour
    {
        public float flySpeed = 40f;
        public float walkSpeed = 4.5f;
        public float eyeHeight = 1.7f;
        public bool walking;
        // dungeons: find the floor from the eye down (not from the sky: there are ceilings and
        // levels above) and stop at walls
        public bool rayFromEye;
        public bool stopAtWalls;
        float yaw, pitch, fall;

        void Start()
        {
            SyncHeading();
        }

        /// <summary>Takes up the transform's heading (after something else turned the camera).</summary>
        public void SyncHeading()
        {
            var e = transform.eulerAngles;
            yaw = e.y;
            pitch = e.x > 180f ? e.x - 360f : e.x;
        }

#if ENABLE_INPUT_SYSTEM && !ENABLE_LEGACY_INPUT_MANAGER
        static bool Down(Key k) => Keyboard.current != null && Keyboard.current[k].isPressed;
        static bool Look() => Mouse.current != null && Mouse.current.rightButton.isPressed;
        static Vector2 MouseDelta() => Mouse.current != null ? Mouse.current.delta.ReadValue() * 0.1f : Vector2.zero;
        static bool Toggle() => Keyboard.current != null && Keyboard.current.fKey.wasPressedThisFrame;
        Vector3 Move() => new Vector3((Down(Key.D) ? 1 : 0) - (Down(Key.A) ? 1 : 0), (Down(Key.E) ? 1 : 0) - (Down(Key.Q) ? 1 : 0), (Down(Key.W) ? 1 : 0) - (Down(Key.S) ? 1 : 0));
        static bool Fast() => Down(Key.LeftShift);
#else
        static bool Look() => Input.GetMouseButton(1);
        static Vector2 MouseDelta() => new Vector2(Input.GetAxis("Mouse X"), Input.GetAxis("Mouse Y"));
        static bool Toggle() => Input.GetKeyDown(KeyCode.F);
        Vector3 Move() => new Vector3((Input.GetKey(KeyCode.D) ? 1 : 0) - (Input.GetKey(KeyCode.A) ? 1 : 0), (Input.GetKey(KeyCode.E) ? 1 : 0) - (Input.GetKey(KeyCode.Q) ? 1 : 0), (Input.GetKey(KeyCode.W) ? 1 : 0) - (Input.GetKey(KeyCode.S) ? 1 : 0));
        static bool Fast() => Input.GetKey(KeyCode.LeftShift);
#endif

        void Update()
        {
            if (Look())
            {
                var d = MouseDelta();
                yaw += d.x * 2.5f;
                pitch = Mathf.Clamp(pitch - d.y * 2.5f, -89f, 89f);
                transform.rotation = Quaternion.Euler(pitch, yaw, 0f);
            }
            if (Toggle())
            {
                walking = !walking;
                fall = 0f;
            }
            var move = Move();
            float fast = Fast() ? 5f : 1f;
            if (!walking)
            {
                transform.position += transform.rotation * move * flySpeed * fast * Time.deltaTime;
                return;
            }
            var flat = Quaternion.Euler(0f, yaw, 0f) * new Vector3(move.x, 0f, move.z);
            if (flat.sqrMagnitude > 0f)
            {
                var step = flat.normalized * walkSpeed * (fast > 1f ? 2.5f : 1f) * Time.deltaTime;
                var chest = transform.position + Vector3.down * (eyeHeight * 0.45f);
                if (!(stopAtWalls && Physics.Raycast(chest, step.normalized, step.magnitude + 0.35f)))
                    transform.position += step;
            }
            var p = transform.position;
            var top = p + Vector3.up * (rayFromEye ? 0.4f : 400f);
            float ground = Physics.Raycast(top, Vector3.down, out var hit, 4000f) ? hit.point.y : p.y - eyeHeight;
            float target = ground + eyeHeight;
            if (p.y > target + 0.05f)
            {
                fall += 9.8f * Time.deltaTime;
                p.y = Mathf.Max(target, p.y - fall * Time.deltaTime);
            }
            else
            {
                fall = 0f;
                p.y = target;
            }
            transform.position = p;
        }
    }
}
