//! Particle storage: a fixed number of slots shared by every material.
//!
//! Port of `core/particles.ts` as structure-of-arrays on the CPU. Slots that
//! hold no particle are [`Kind::Free`]; they are skipped by every pass and
//! handed out again by runtime emission.

use glam::{Quat, Vec3A};

/// What a slot holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Kind {
    /// Unused slot, available to emission.
    Free = 0,
    /// A liquid particle (position based fluids).
    Liquid = 1,
    /// A cloth vertex.
    Cloth = 2,
    /// A soft body particle.
    Softbody = 3,
    /// An air particle of a smoke simulation (also a position based fluid).
    Air = 4,
}

impl Kind {
    /// Liquids and air are fluids: they keep each other apart by pressure and
    /// never get particle–particle contacts with one another.
    #[inline(always)]
    pub fn is_fluid(self) -> bool {
        matches!(self, Kind::Liquid | Kind::Air)
    }
}

/// Sentinel for "not a boundary particle".
pub(crate) const NO_BOUNDARY: u32 = u32::MAX;

/// Structure-of-arrays particle state. Every vector has `capacity` entries.
pub(crate) struct Particles {
    pub radius: f32,
    pub pos: Vec<Vec3A>,
    /// Positions being solved during a substep (`x*`).
    pub pred: Vec<Vec3A>,
    pub vel: Vec<Vec3A>,
    /// Inverse mass, 1/kg. `0` pins the particle.
    pub inv_mass: Vec<f32>,
    /// Particles sharing a non-zero group never get contacts with each other.
    pub group: Vec<u32>,
    /// Akinci boundary volume, m³ (non-zero for fluid boundary particles).
    pub boundary_volume: Vec<f32>,
    /// Which boundary range a boundary particle belongs to, or [`NO_BOUNDARY`].
    pub boundary_id: Vec<u32>,
    pub kind: Vec<Kind>,
    /// Index of the owning material within its kind's list.
    pub material: Vec<u16>,
    pub rot: Vec<Quat>,
    pub pred_rot: Vec<Quat>,
    pub ang_vel: Vec<Vec3A>,
    /// Free slots; popped from the back (lowest index first).
    free: Vec<u32>,
}

impl Particles {
    pub fn new(capacity: usize, radius: f32) -> Self {
        Self {
            radius,
            pos: vec![Vec3A::ZERO; capacity],
            pred: vec![Vec3A::ZERO; capacity],
            vel: vec![Vec3A::ZERO; capacity],
            inv_mass: vec![0.0; capacity],
            group: vec![0; capacity],
            boundary_volume: vec![0.0; capacity],
            boundary_id: vec![NO_BOUNDARY; capacity],
            kind: vec![Kind::Free; capacity],
            material: vec![0; capacity],
            rot: vec![Quat::IDENTITY; capacity],
            pred_rot: vec![Quat::IDENTITY; capacity],
            ang_vel: vec![Vec3A::ZERO; capacity],
            free: (0..capacity as u32).rev().collect(),
        }
    }

    pub fn capacity(&self) -> usize {
        self.pos.len()
    }

    /// Number of free slots.
    pub fn free_count(&self) -> usize {
        self.free.len()
    }

    /// Claim the lowest free slot.
    pub fn alloc(&mut self) -> Option<u32> {
        self.free.pop()
    }

    /// Claim `count` consecutive slots from the start of the free space. Only
    /// valid while the free list is still the untouched tail of the storage
    /// (during the initial layout).
    pub fn alloc_range(&mut self, count: usize) -> Option<u32> {
        if count > self.free.len() {
            return None;
        }
        let start = *self.free.last()?;
        for k in 0..count as u32 {
            let slot = self.free.pop()?;
            debug_assert_eq!(slot, start + k, "alloc_range on a fragmented free list");
        }
        Some(start)
    }

    /// Write a new particle into slot `i`.
    #[allow(clippy::too_many_arguments)]
    pub fn init(
        &mut self,
        i: u32,
        kind: Kind,
        material: u16,
        pos: Vec3A,
        vel: Vec3A,
        inv_mass: f32,
        group: u32,
    ) {
        let i = i as usize;
        self.kind[i] = kind;
        self.material[i] = material;
        self.pos[i] = pos;
        self.pred[i] = pos;
        self.vel[i] = vel;
        self.inv_mass[i] = inv_mass;
        self.group[i] = group;
        self.boundary_volume[i] = 0.0;
        self.boundary_id[i] = NO_BOUNDARY;
        self.rot[i] = Quat::IDENTITY;
        self.pred_rot[i] = Quat::IDENTITY;
        self.ang_vel[i] = Vec3A::ZERO;
    }

    /// Return slot `i` to the free pool.
    pub fn release(&mut self, i: u32) {
        let s = i as usize;
        debug_assert_ne!(self.kind[s], Kind::Free);
        self.kind[s] = Kind::Free;
        self.inv_mass[s] = 0.0;
        self.vel[s] = Vec3A::ZERO;
        self.boundary_volume[s] = 0.0;
        self.boundary_id[s] = NO_BOUNDARY;
        self.free.push(i);
    }

    /// Keep the free list sorted so the lowest slot is handed out first
    /// (deterministic, and keeps live particles packed toward the front).
    pub fn sort_free(&mut self) {
        self.free.sort_unstable_by(|a, b| b.cmp(a));
    }

    /// Indices of every occupied slot, ascending.
    pub fn active_indices(&self, out: &mut Vec<u32>) {
        out.clear();
        out.extend(
            self.kind
                .iter()
                .enumerate()
                .filter(|(_, k)| **k != Kind::Free)
                .map(|(i, _)| i as u32),
        );
    }
}
