//! A character's moves: which library animation it plays for each state (idle, walking,
//! attacking, sitting…). NPCs and the player character both read them; each character can
//! choose its own, the rest come from the bundled library.

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Move {
    Idle,
    Walk,
    Run,
    Sprint,
    Crouch,
    CrouchWalk,
    JumpStart,
    JumpAir,
    JumpLand,
    Swim,
    SwimIdle,
    LightAttack,
    HeavyAttack,
    Block,
    Roll,
    Hit,
    Death,
    Talk,
    Sit,
    Work,
    Wave,
    Sleep,
}

impl Move {
    pub const ALL: [Move; 22] = [
        Move::Idle,
        Move::Walk,
        Move::Run,
        Move::Sprint,
        Move::Crouch,
        Move::CrouchWalk,
        Move::JumpStart,
        Move::JumpAir,
        Move::JumpLand,
        Move::Swim,
        Move::SwimIdle,
        Move::LightAttack,
        Move::HeavyAttack,
        Move::Block,
        Move::Roll,
        Move::Hit,
        Move::Death,
        Move::Talk,
        Move::Sit,
        Move::Work,
        Move::Wave,
        Move::Sleep,
    ];
    pub fn id(self) -> &'static str {
        match self {
            Move::Idle => "idle",
            Move::Walk => "walk",
            Move::Run => "run",
            Move::Sprint => "sprint",
            Move::Crouch => "crouch",
            Move::CrouchWalk => "crouch_walk",
            Move::JumpStart => "jump_start",
            Move::JumpAir => "jump_air",
            Move::JumpLand => "jump_land",
            Move::Swim => "swim",
            Move::SwimIdle => "swim_idle",
            Move::LightAttack => "light_attack",
            Move::HeavyAttack => "heavy_attack",
            Move::Block => "block",
            Move::Roll => "roll",
            Move::Hit => "hit",
            Move::Death => "death",
            Move::Talk => "talk",
            Move::Sit => "sit",
            Move::Work => "work",
            Move::Wave => "wave",
            Move::Sleep => "sleep",
        }
    }
    pub fn from_id(id: &str) -> Option<Move> {
        Move::ALL.into_iter().find(|m| m.id() == id)
    }
    pub fn label(self) -> &'static str {
        match self {
            Move::Idle => "Idle",
            Move::Walk => "Walk",
            Move::Run => "Run",
            Move::Sprint => "Sprint",
            Move::Crouch => "Crouch",
            Move::CrouchWalk => "Crouch walk",
            Move::JumpStart => "Jump: take off",
            Move::JumpAir => "Jump: in the air",
            Move::JumpLand => "Jump: land",
            Move::Swim => "Swim",
            Move::SwimIdle => "Tread water",
            Move::LightAttack => "Light attack",
            Move::HeavyAttack => "Heavy attack",
            Move::Block => "Block",
            Move::Roll => "Dodge roll",
            Move::Hit => "Get hit",
            Move::Death => "Death",
            Move::Talk => "Talk",
            Move::Sit => "Sit",
            Move::Work => "Work",
            Move::Wave => "Wave / yes",
            Move::Sleep => "Sleep",
        }
    }
    /// Which group the Moves tab shows it in.
    pub fn group(self) -> &'static str {
        match self {
            Move::Idle
            | Move::Walk
            | Move::Run
            | Move::Sprint
            | Move::Crouch
            | Move::CrouchWalk
            | Move::JumpStart
            | Move::JumpAir
            | Move::JumpLand
            | Move::Swim
            | Move::SwimIdle => "Moving",
            Move::LightAttack
            | Move::HeavyAttack
            | Move::Block
            | Move::Roll
            | Move::Hit
            | Move::Death => "Fighting",
            _ => "Living",
        }
    }
    /// It plays once (then goes back to what it was doing), not in a loop.
    pub fn one_shot(self) -> bool {
        matches!(
            self,
            Move::JumpStart
                | Move::JumpLand
                | Move::LightAttack
                | Move::HeavyAttack
                | Move::Roll
                | Move::Hit
                | Move::Death
                | Move::Wave
        )
    }
    /// The bundled clip it plays unless the character says otherwise.
    pub fn default_anim(self) -> &'static str {
        match self {
            Move::Idle => "ual1/Idle_Loop",
            Move::Walk => "ual1/Walk_Loop",
            Move::Run => "ual1/Jog_Fwd_Loop",
            Move::Sprint => "ual1/Sprint_Loop",
            Move::Crouch => "ual1/Crouch_Idle_Loop",
            Move::CrouchWalk => "ual1/Crouch_Fwd_Loop",
            Move::JumpStart => "ual1/Jump_Start",
            Move::JumpAir => "ual1/Jump_Loop",
            Move::JumpLand => "ual1/Jump_Land",
            Move::Swim => "ual1/Swim_Fwd_Loop",
            Move::SwimIdle => "ual1/Swim_Idle_Loop",
            Move::LightAttack => "ual2/Sword_Regular_A",
            Move::HeavyAttack => "ual2/Sword_Heavy_Combo",
            Move::Block => "ual2/Sword_Block",
            Move::Roll => "ual1/Roll",
            Move::Hit => "ual1/Hit_Chest",
            Move::Death => "ual1/Death01",
            Move::Talk => "ual1/Idle_Talking_Loop",
            Move::Sit => "ual1/Sitting_Idle_Loop",
            Move::Work => "ual2/TreeChopping_Loop",
            Move::Wave => "ual2/Yes",
            Move::Sleep => "ual2/LayToIdle",
        }
    }
    /// About how fast (m/s) a clip of this move travels: locomotion plays faster or slower to
    /// match the real speed, so feet don't slide.
    pub fn speed(self) -> Option<f32> {
        match self {
            Move::Walk => Some(1.6),
            Move::Run => Some(3.6),
            Move::Sprint => Some(6.2),
            Move::CrouchWalk => Some(1.2),
            Move::Swim => Some(1.6),
            _ => None,
        }
    }
}

/// The clip for `m`: the character's own choice, else the default.
pub fn anim_for(own: &BTreeMap<String, String>, m: Move) -> String {
    own.get(m.id())
        .cloned()
        .unwrap_or_else(|| m.default_anim().to_string())
}
