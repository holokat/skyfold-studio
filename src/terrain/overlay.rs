//! A world-aligned colour image drawn over the terrain: analysis heatmaps, walkability, spawn
//! regions — anything a tool wants to show on the ground. One owner at a time; a tool shows its
//! overlay while its tab is open and clears only its own.

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use super::material::{TerrainGpu, TerrainMaterial};

#[derive(Resource, Default)]
pub struct TerrainOverlay {
    /// RGBA8 sRGB image covering the whole terrain: u along +x, v along +z, from the −x/−z
    /// corner (`Terrain::half()`); alpha = coverage. Any resolution; sampled bilinearly.
    pub image: Option<Handle<Image>>,
    /// 0 hides the overlay.
    pub opacity: f32,
    /// Who set it (e.g. "analyze"), so tools only clear their own overlay.
    pub owner: &'static str,
}

impl TerrainOverlay {
    pub fn show(&mut self, owner: &'static str, image: Handle<Image>, opacity: f32) {
        self.owner = owner;
        self.image = Some(image);
        self.opacity = opacity;
    }
    pub fn clear(&mut self, owner: &'static str) {
        if self.owner == owner && self.image.is_some() {
            self.image = None;
            self.opacity = 0.0;
        }
    }
    pub fn is_shown_by(&self, owner: &'static str) -> bool {
        self.owner == owner && self.image.is_some() && self.opacity > 0.0
    }
}

/// Editor-authored terrain visuals belong to the editing view. Keep their state intact while the
/// player or a camera path owns the viewport so they return when editing resumes.
pub(crate) fn editor_terrain_visuals_visible(walk: &crate::walk::WalkState) -> bool {
    !walk.active && !walk.cinematic
}

/// An overlay image to fill (`w × h` RGBA8 sRGB, transparent); write texels with
/// `image.data` (row-major from the −z edge, 4 bytes each) and mark it changed.
pub fn new_overlay_image(w: u32, h: u32) -> Image {
    Image::new_fill(
        Extent3d {
            width: w.max(1),
            height: h.max(1),
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[0, 0, 0, 0],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    )
}

#[derive(Resource)]
pub(crate) struct OverlayPlaceholder(pub Handle<Image>);

pub(crate) fn init_placeholder(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    commands.insert_resource(OverlayPlaceholder(images.add(new_overlay_image(1, 1))));
}

/// Pushes the overlay into the terrain material (only when it changed).
pub(crate) fn apply_overlay(
    overlay: Res<TerrainOverlay>,
    placeholder: Option<Res<OverlayPlaceholder>>,
    gpu: Option<Res<TerrainGpu>>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
    walk: Res<crate::walk::WalkState>,
    mcp: Option<Res<crate::mcp::McpServer>>,
) {
    let (Some(gpu), Some(ph)) = (gpu, placeholder) else {
        return;
    };
    // Walk mode and flythrough captures show the world only. Clean screenshots retain the
    // selection exception for its explicit outline option.
    let hidden = !editor_terrain_visuals_visible(&walk)
        || (!crate::mcp::ui_visible(mcp) && overlay.owner != "selection");
    let want = overlay
        .image
        .clone()
        .filter(|_| !hidden)
        .unwrap_or_else(|| ph.0.clone());
    let opacity = if overlay.image.is_some() && !hidden {
        overlay.opacity
    } else {
        0.0
    };
    let Some(m) = materials.get(&gpu.material) else {
        return;
    };
    if m.extension.overlay == want && (m.extension.params.overlay_image.x - opacity).abs() < 1e-4 {
        return;
    }
    if let Some(mut m) = materials.get_mut(&gpu.material) {
        m.extension.overlay = want;
        m.extension.params.overlay_image.x = opacity;
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::{Assets, Handle, Image, StandardMaterial, World};

    use super::{
        OverlayPlaceholder, TerrainGpu, TerrainMaterial, TerrainOverlay, apply_overlay,
        editor_terrain_visuals_visible, new_overlay_image,
    };
    use crate::terrain::material::{TerrainExt, TerrainParams};
    use crate::walk::WalkState;

    #[test]
    fn editor_terrain_visuals_return_after_walk_or_cinematic_mode() {
        let mut walk = WalkState::default();
        assert!(editor_terrain_visuals_visible(&walk));

        walk.active = true;
        assert!(!editor_terrain_visuals_visible(&walk));

        walk.active = false;
        walk.cinematic = true;
        assert!(!editor_terrain_visuals_visible(&walk));

        walk.cinematic = false;
        assert!(editor_terrain_visuals_visible(&walk));
    }

    #[test]
    fn selection_overlay_hides_for_walk_and_restores_without_changing_its_state() {
        let mut world = World::new();
        let mut images = Assets::<Image>::default();
        let placeholder = images.add(new_overlay_image(1, 1));
        let selection = images.add(new_overlay_image(1, 1));
        world.insert_resource(images);

        let mut materials = Assets::<TerrainMaterial>::default();
        let material = materials.add(TerrainMaterial {
            base: StandardMaterial::default(),
            extension: TerrainExt {
                params: TerrainParams::default(),
                normal_map: Handle::default(),
                splat0: Handle::default(),
                splat1: Handle::default(),
                splat2: Handle::default(),
                height: Handle::default(),
                zones: Handle::default(),
                albedo_array: Handle::default(),
                detail_array: Handle::default(),
                overlay: Handle::default(),
                shore: Handle::default(),
                cloud_cover: Handle::default(),
                rule0: Handle::default(),
                rule1: Handle::default(),
                reference: false,
                per_pixel_rules: false,
            },
        });
        world.insert_resource(materials);
        world.insert_resource(TerrainGpu {
            material: material.clone(),
            height: Handle::default(),
            normal: Handle::default(),
            splat: std::array::from_fn(|_| Handle::default()),
            zones: Handle::default(),
            rules: std::array::from_fn(|_| Handle::default()),
            generation: None,
        });
        world.insert_resource(OverlayPlaceholder(placeholder.clone()));
        world.insert_resource(TerrainOverlay {
            image: Some(selection.clone()),
            opacity: 0.65,
            owner: "selection",
        });
        world.insert_resource(WalkState::default());

        // The selection owner is intentionally visible in a clean screenshot while editing.
        let mut mcp = crate::mcp::McpServer::default();
        mcp.hide_ui = 1;
        world.insert_resource(mcp);

        run_apply_overlay(&mut world);
        assert_material_overlay(&world, &material, &selection, 0.65);

        world.resource_mut::<WalkState>().active = true;
        run_apply_overlay(&mut world);
        assert_material_overlay(&world, &material, &placeholder, 0.0);

        let mut walk = world.resource_mut::<WalkState>();
        walk.active = false;
        walk.cinematic = true;
        drop(walk);
        run_apply_overlay(&mut world);
        assert_material_overlay(&world, &material, &placeholder, 0.0);

        world.resource_mut::<WalkState>().cinematic = false;
        run_apply_overlay(&mut world);
        assert_material_overlay(&world, &material, &selection, 0.65);

        let overlay = world.resource::<TerrainOverlay>();
        assert_eq!(overlay.owner, "selection");
        assert_eq!(overlay.image.as_ref(), Some(&selection));
        assert_eq!(overlay.opacity, 0.65);
    }

    fn run_apply_overlay(world: &mut World) {
        world.run_system_once(apply_overlay).unwrap();
    }

    fn assert_material_overlay(
        world: &World,
        material: &Handle<TerrainMaterial>,
        expected: &Handle<Image>,
        opacity: f32,
    ) {
        let materials = world.resource::<Assets<TerrainMaterial>>();
        let extension = &materials.get(material).unwrap().extension;
        assert_eq!(&extension.overlay, expected);
        assert_eq!(extension.params.overlay_image.x, opacity);
    }
}
