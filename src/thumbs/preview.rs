//! The turntable: a live render of the asset (or fish) being placed, turning slowly in the studio
//! on its own render layer, for the big preview card. It exists only while the UI shows it: a
//! frame without a request takes the subject, camera and render target down again, so a closed
//! tab costs nothing.

use bevy::prelude::*;
use bevy_egui::{EguiTextureHandle, EguiUserTextures, egui};

use super::{Previews, Stage, Staged, Subject, studio};

/// The card's picture (px): twice its size in the panel, for high-DPI screens.
pub const SIZE: UVec2 = UVec2::new(520, 340);
/// Where the turntable stands, away from the thumbnail shoots.
const ORIGIN: Vec3 = Vec3::new(0.0, -20_000.0, 3_000.0);

#[derive(Default)]
pub(super) struct Turntable {
    /// asked for by the UI since the last update
    want: Option<Subject>,
    subject: Option<Subject>,
    staged: Option<Staged>,
    camera: Option<Entity>,
    target: Option<Handle<Image>>,
    egui: Option<egui::TextureId>,
    /// the turning axis (subject space, xz)
    axis: Vec3,
    /// frames rendered so far
    frames: u32,
    angle: f32,
}

impl Turntable {
    pub(super) fn ask(&mut self, s: &Subject) -> Option<egui::TextureId> {
        self.want = Some(s.clone());
        if self.subject.as_ref() == Some(s) && self.frames > 3 {
            self.egui
        } else {
            None
        }
    }
    pub(super) fn active(&self) -> bool {
        self.subject.is_some()
    }
    pub(super) fn publish(&mut self, textures: &mut EguiUserTextures) {
        if self.egui.is_none()
            && let Some(t) = &self.target
        {
            self.egui = Some(textures.add_image(EguiTextureHandle::Weak(t.id())));
        }
    }
    fn clear(&mut self, commands: &mut Commands) {
        if let Some(s) = self.staged.take() {
            commands.entity(s.root).despawn();
        }
        if let Some(c) = self.camera.take() {
            commands.entity(c).despawn();
        }
        self.subject = None;
        self.frames = 0;
    }
}

/// Stages what the UI asked for, frames it once it's ready and turns it.
pub(super) fn turntable(
    mut p: ResMut<Previews>,
    mut stage: Stage,
    time: Res<Time<Real>>,
    mut tfs: Query<&mut Transform>,
    mut textures: ResMut<EguiUserTextures>,
) {
    let t = &mut p.turntable;
    let want = t.want.take();
    if want != t.subject {
        t.clear(&mut stage.commands);
        let Some(s) = want else {
            // nobody looking: free the picture too
            if let Some(img) = t.target.take() {
                textures.remove_image(&img);
            }
            t.egui = None;
            return;
        };
        t.staged = stage.spawn(&s, ORIGIN, studio::PREVIEW_LAYER, true);
        t.subject = Some(s);
        t.angle = 0.0;
    }
    let Some(s) = t.subject.clone() else { return };
    let Some(staged) = &t.staged else { return };
    if t.camera.is_none() {
        // the studio's environment first, so the first frames are lit like the rest
        if stage.studio.env_light.is_none() {
            return;
        }
        let Some(pts) = stage.ready(staged, studio::PREVIEW_LAYER) else {
            return;
        };
        let (mut min, mut max) = (Vec3::MAX, Vec3::MIN);
        for q in &pts {
            (min, max) = (min.min(*q), max.max(*q));
        }
        t.axis = Vec3::new((min.x + max.x) * 0.5, 0.0, (min.z + max.z) * 0.5);
        let outline: Vec<Vec3> = if s.fish().is_some() {
            pts
        } else {
            studio::swept(min, max, t.axis.xz())
        };
        let outline: Vec<Vec3> = outline.iter().map(|q| *q + ORIGIN).collect();
        let (at, look, near, far) = studio::frame(
            &outline,
            studio::view_dir(s.fish()),
            SIZE.x as f32 / SIZE.y as f32,
            if s.fish().is_some() { 0.1 } else { 0.06 },
        );
        let target = t
            .target
            .get_or_insert_with(|| studio::target(&mut stage.images, SIZE))
            .clone();
        t.camera = Some(
            stage
                .commands
                .spawn(studio::camera(
                    &stage.studio,
                    target,
                    -90,
                    at,
                    look,
                    near,
                    far,
                    studio::PREVIEW_LAYER,
                ))
                .id(),
        );
    }
    t.frames += 1;
    // objects turn round; fish sway either side of their profile, swimming on the spot
    let dt = time.delta_secs().min(0.1);
    t.angle += dt;
    let yaw = if s.fish().is_some() {
        (t.angle * 0.4).sin() * 0.5
    } else {
        t.angle * 0.45
    };
    if let Ok(mut tf) = tfs.get_mut(staged.root) {
        let rot = Quat::from_rotation_y(yaw);
        tf.rotation = rot;
        tf.translation = ORIGIN + t.axis - rot * t.axis;
    }
}
