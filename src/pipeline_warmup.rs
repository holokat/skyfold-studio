//! Moves the one-off PBR pipeline variants used by walk-mode bounce light into
//! the editing/loading period.  Adding an [`IrradianceVolume`] changes Bevy's
//! `MeshPipelineKey`; on Metal, compiling those variants on the first walk
//! frame blocks synchronously in the driver.
//!
//! This never creates a probe or changes a camera's lighting. It uses prepared
//! main-view materials, plus the existing grass assets, to populate Bevy's
//! shared material cache through its public specialization callbacks. Targets
//! cover the main view's bounce-light variants and the normal/bounce variants
//! of the planar reflection camera, even before that camera exists.

use std::collections::{HashMap, HashSet, VecDeque};

use bevy::camera::Camera3d;
use bevy::ecs::system::{SystemParam, SystemState};
use bevy::material::key::{ErasedMaterialPipelineKey, ErasedMeshPipelineKey};
use bevy::material::specialize::{BaseSpecializeFn, PrepassSpecializeFn};
use bevy::material::{AlphaMode, MaterialProperties, OpaqueRendererMethod};
use bevy::mesh::Mesh3d;
use bevy::pbr::Lightmap;
use bevy::pbr::{
    MeshPipelineKey, PreparedMaterial, PrepassFragmentShader, PrepassVertexShader,
    RenderMaterialInstances, RenderMeshInstanceFlags, RenderMeshInstances,
    SpecializedMaterialPipelineCache, ViewKeyCache, alpha_mode_pipeline_key,
};
use bevy::platform::sync::Arc;
use bevy::prelude::*;
use bevy::render::extract_component::{ExtractComponent, ExtractComponentPlugin};
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::mesh::RenderMesh;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{CachedRenderPipelineId, PipelineCache};
use bevy::render::sync_world::MainEntity;
use bevy::render::view::{ExtractedView, Msaa, RenderVisibilityRanges, RenderVisibleEntities};
use bevy::render::{
    GpuResourceAppExt, Render, RenderApp, RenderSystems, erased_render_asset::ErasedRenderAssets,
};

use crate::grass::GrassPipelineCandidates;

/// A small extracted mirror of `Lightmap`: Bevy keeps its lightmap table
/// private, but the two bits it adds to a material pipeline key are public
/// scene data.  Keeping this beside the source mesh lets prewarming preserve
/// those bits without touching the scene's lighting.
#[derive(Component, Clone, Copy, ExtractComponent)]
#[extract_app(RenderApp)]
struct WarmupLightmap {
    bicubic: bool,
}

fn mirror_lightmaps(
    mut commands: Commands,
    changed: Query<(Entity, &Lightmap), Or<(Added<Lightmap>, Changed<Lightmap>)>>,
    mut removed: RemovedComponents<Lightmap>,
) {
    for (entity, lightmap) in &changed {
        commands.entity(entity).insert(WarmupLightmap {
            bicubic: lightmap.bicubic_sampling,
        });
    }
    for entity in removed.read() {
        commands.entity(entity).remove::<WarmupLightmap>();
    }
}

/// Main-world state extracted into the renderer.  Once walking begins, all
/// warmup work stops so it cannot introduce a new hitch into play.
#[derive(Resource, Clone, Default, ExtractResource)]
#[extract_app(RenderApp)]
struct PipelineWarmupGate {
    enabled: bool,
    main_camera: Option<Entity>,
}

fn update_warmup_gate(
    mut gate: ResMut<PipelineWarmupGate>,
    lighting: Res<crate::sky::Lighting>,
    walk: Res<crate::walk::WalkState>,
    main_camera: Query<Entity, With<crate::camera::MainCamera>>,
) {
    gate.enabled = lighting.bounce_light && !walk.active;
    gate.main_camera = main_camera.single().ok();
}

#[derive(Resource)]
struct PipelineWarmup {
    /// Every attempted material-key/layout pair.  A changing view/material
    /// naturally produces a different key, while pending or unsupported
    /// variants never consume a frame repeatedly.
    attempted: HashSet<(
        WarmupPass,
        ErasedMaterialPipelineKey,
        bevy::mesh::MeshVertexBufferLayoutRef,
    )>,
    /// Normal source pipelines already considered for each target view key.
    /// A source ID uniquely identifies a material key and compatible vertex
    /// layout, but a reflected camera can need a distinct view-key variant
    /// even when that material was not visible in its own frustum yet.
    seen_sources: HashSet<(CachedRenderPipelineId, MeshPipelineKey)>,
    requests: VecDeque<WarmupRequest>,
    pending: HashSet<CachedRenderPipelineId>,
    queued: usize,
    finished: usize,
    enabled: bool,
    verbose: bool,
}

impl Default for PipelineWarmup {
    fn default() -> Self {
        Self {
            attempted: default(),
            seen_sources: default(),
            requests: default(),
            pending: default(),
            queued: 0,
            finished: 0,
            enabled: std::env::var("WB_PIPELINE_WARMUP").map_or(true, |value| value != "0"),
            verbose: std::env::var_os("WB_FPS").is_some(),
        }
    }
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
enum WarmupPass {
    Base,
    Prepass,
}

enum WarmupRequest {
    Base {
        specialize: BaseSpecializeFn,
        key: ErasedMaterialPipelineKey,
        layout: bevy::mesh::MeshVertexBufferLayoutRef,
        properties: Arc<MaterialProperties>,
    },
    Prepass {
        specialize: PrepassSpecializeFn,
        key: ErasedMaterialPipelineKey,
        layout: bevy::mesh::MeshVertexBufferLayoutRef,
        properties: Arc<MaterialProperties>,
    },
}

fn enqueue_base(
    warmup: &mut PipelineWarmup,
    key: ErasedMaterialPipelineKey,
    layout: bevy::mesh::MeshVertexBufferLayoutRef,
    properties: Arc<MaterialProperties>,
) {
    if !warmup
        .attempted
        .insert((WarmupPass::Base, key.clone(), layout.clone()))
    {
        return;
    }
    if let Some(specialize) = properties.base_specialize {
        warmup.requests.push_back(WarmupRequest::Base {
            specialize,
            key,
            layout,
            properties,
        });
    }
}

fn enqueue_prepass(
    warmup: &mut PipelineWarmup,
    key: ErasedMaterialPipelineKey,
    layout: bevy::mesh::MeshVertexBufferLayoutRef,
    properties: Arc<MaterialProperties>,
) {
    if !warmup
        .attempted
        .insert((WarmupPass::Prepass, key.clone(), layout.clone()))
    {
        return;
    }
    if let Some(specialize) = properties.prepass_specialize {
        warmup.requests.push_back(WarmupRequest::Prepass {
            specialize,
            key,
            layout,
            properties,
        });
    }
}

/// Queue at most one material specialization request per render frame.
///
/// Source materials come from the prepared main-view pipelines and the
/// existing grass asset candidates. The latter cover grass that is hidden
/// while the editor camera is above its visibility range.
#[derive(SystemParam)]
struct WarmupParams<'w, 's> {
    warmup: ResMut<'w, PipelineWarmup>,
    gate: Res<'w, PipelineWarmupGate>,
    pipeline_cache: Res<'w, PipelineCache>,
    view_keys: Res<'w, ViewKeyCache>,
    specialized: Res<'w, SpecializedMaterialPipelineCache>,
    render_meshes: Res<'w, RenderAssets<RenderMesh>>,
    render_materials: Res<'w, ErasedRenderAssets<PreparedMaterial>>,
    mesh_instances: Res<'w, RenderMeshInstances>,
    material_instances: Res<'w, RenderMaterialInstances>,
    grass_candidates: Res<'w, GrassPipelineCandidates>,
    visibility_ranges: Res<'w, RenderVisibilityRanges>,
    views: Query<
        'w,
        's,
        (
            &'static Camera3d,
            &'static ExtractedView,
            &'static RenderVisibleEntities,
        ),
    >,
    lightmaps: Query<'w, 's, (&'static MainEntity, &'static WarmupLightmap)>,
}

/// This must be an exclusive system because Bevy exposes the correct material
/// specialization through a public callback that receives `&mut World`.
fn warm_irradiance_volume_pipelines(world: &mut World, state: &mut SystemState<WarmupParams>) {
    let item = {
        let WarmupParams {
            mut warmup,
            pipeline_cache,
            view_keys,
            specialized,
            render_meshes,
            render_materials,
            mesh_instances,
            material_instances,
            grass_candidates,
            visibility_ranges,
            views,
            lightmaps: lightmap_components,
            gate,
        } = state.get_mut(world).expect("pipeline warmup system state");

        if !warmup.enabled || !gate.enabled {
            return;
        }

        let completed = warmup
            .pending
            .extract_if(|id| pipeline_cache.get_render_pipeline(*id).is_some())
            .count();
        if completed != 0 {
            warmup.finished += completed;
            if warmup.verbose {
                info!(
                    target: "world_builder::pipeline_warmup",
                    queued = warmup.queued,
                    finished = warmup.finished,
                    pending = warmup.pending.len(),
                    "bounce-light PBR warmup progress"
                );
            }
        }

        if let Some(request) = warmup.requests.pop_front() {
            Some(request)
        } else {
            // Build this only once a source has passed the cheap pipeline-ID
            // filter below.  In steady state every source is already seen.
            let mut lightmaps: Option<HashMap<MainEntity, bool>> = None;

            // The planar camera is spawned only once water enters the view.
            // Derive its fixed key from the extracted main-camera key so its
            // first use cannot compile a reflection-only material variant in
            // the Walk transition. Do not use temporary atlas Camera3ds here.
            let Some(main_camera) = gate.main_camera else {
                return;
            };
            let Some(main_view_key) = views.iter().find_map(|(_camera, view, _)| {
                (view.retained_view_entity.main_entity.id() == main_camera)
                    .then(|| view_keys.get(&view.retained_view_entity).copied())
                    .flatten()
            }) else {
                return;
            };
            let active_view_keys = [
                main_view_key,
                crate::water_render::reflection_pipeline_key(main_view_key),
            ];

            'views: for (_camera, view, visible) in &views {
                if view.retained_view_entity.main_entity.id() != main_camera {
                    continue;
                }
                let Some(view_key) = view_keys.get(&view.retained_view_entity).copied() else {
                    continue;
                };
                let Some(source_pipelines) = specialized.get(&view.retained_view_entity) else {
                    continue;
                };
                let Some(meshes) = visible.get::<Mesh3d>() else {
                    continue;
                };

                for visible_entity in meshes
                    .entities_cpu_culling
                    .iter()
                    .map(|(_, entity)| *entity)
                    .chain(meshes.entities_gpu_culling.keys().copied())
                {
                    // Only clone a key after Bevy has used the ordinary variant
                    // for this exact entity in this exact view.
                    let Some(source_id) = source_pipelines.get(&visible_entity) else {
                        continue;
                    };
                    if pipeline_cache.get_render_pipeline(*source_id).is_none() {
                        continue;
                    }
                    if active_view_keys.iter().all(|target_view_key| {
                        warmup
                            .seen_sources
                            .contains(&(*source_id, *target_view_key))
                    }) {
                        continue;
                    }
                    let Some(material_instance) = material_instances.instances.get(&visible_entity)
                    else {
                        continue;
                    };
                    let Some(mesh_instance) = mesh_instances.render_mesh_queue_data(visible_entity)
                    else {
                        continue;
                    };
                    let Some(mesh) = render_meshes.get(mesh_instance.mesh_asset_id()) else {
                        continue;
                    };
                    let Some(material) = render_materials.get(material_instance.asset_id) else {
                        continue;
                    };

                    let lightmap = lightmaps
                        .get_or_insert_with(|| {
                            lightmap_components
                                .iter()
                                .map(|(entity, marker)| (*entity, marker.bicubic))
                                .collect()
                        })
                        .get(&visible_entity)
                        .copied();
                    for target_view_key in active_view_keys.iter().copied() {
                        // Do not cache an unprepared mesh/material above: it may
                        // become ready on a later frame. From here candidates are
                        // complete, including a duplicate target key.
                        if !warmup.seen_sources.insert((*source_id, target_view_key)) {
                            continue;
                        }

                        let mut material_key_bits: MeshPipelineKey =
                            material.properties.mesh_pipeline_key_bits.downcast();
                        material_key_bits.insert(alpha_mode_pipeline_key(
                            material.properties.alpha_mode,
                            &Msaa::from_samples(target_view_key.msaa_samples()),
                        ));
                        let mut mesh_key = target_view_key
                            | MeshPipelineKey::from_bits_retain(mesh.key_bits.bits())
                            | material_key_bits;

                        if let Some(bicubic) = lightmap {
                            mesh_key |= MeshPipelineKey::LIGHTMAPPED;
                            if bicubic {
                                mesh_key |= MeshPipelineKey::LIGHTMAP_BICUBIC_SAMPLING;
                            }
                        }
                        if visibility_ranges
                            .entity_has_crossfading_visibility_ranges(visible_entity)
                        {
                            mesh_key |= MeshPipelineKey::VISIBILITY_RANGE_DITHER;
                        }
                        if target_view_key.contains(MeshPipelineKey::MOTION_VECTOR_PREPASS) {
                            if mesh_instance
                                .flags()
                                .contains(RenderMeshInstanceFlags::HAS_PREVIOUS_SKIN)
                            {
                                mesh_key |= MeshPipelineKey::HAS_PREVIOUS_SKIN;
                            }
                            if mesh_instance
                                .flags()
                                .contains(RenderMeshInstanceFlags::HAS_PREVIOUS_MORPH)
                            {
                                mesh_key |= MeshPipelineKey::HAS_PREVIOUS_MORPH;
                            }
                        }
                        // The ordinary main-view pipeline is the ready source.
                        // The dormant reflection view has no source yet, so
                        // warm both its normal and bounce-light variants.
                        if target_view_key != view_key {
                            enqueue_base(
                                &mut warmup,
                                ErasedMaterialPipelineKey {
                                    type_id: material_instance.asset_id.type_id(),
                                    mesh_key: ErasedMeshPipelineKey::new(mesh_key),
                                    material_key: material.properties.material_key.clone(),
                                },
                                mesh.layout.clone(),
                                material.properties.clone(),
                            );
                        }
                        mesh_key |= MeshPipelineKey::IRRADIANCE_VOLUME;
                        enqueue_base(
                            &mut warmup,
                            ErasedMaterialPipelineKey {
                                type_id: material_instance.asset_id.type_id(),
                                mesh_key: ErasedMeshPipelineKey::new(mesh_key),
                                material_key: material.properties.material_key.clone(),
                            },
                            mesh.layout.clone(),
                            material.properties.clone(),
                        );

                        // The prepass has a distinct key from the forward material
                        // pipeline. Bevy still propagates IRRADIANCE_VOLUME into it,
                        // so prewarm the exact prepass variant too.
                        let alpha_mode = material.properties.alpha_mode;
                        if material.properties.prepass_enabled
                            && !material.properties.reads_view_transmission_texture
                            && target_view_key.intersects(
                                MeshPipelineKey::NORMAL_PREPASS
                                    | MeshPipelineKey::DEPTH_PREPASS
                                    | MeshPipelineKey::MOTION_VECTOR_PREPASS
                                    | MeshPipelineKey::DEFERRED_PREPASS,
                            )
                            && matches!(
                                alpha_mode,
                                AlphaMode::Opaque | AlphaMode::AlphaToCoverage | AlphaMode::Mask(_)
                            )
                        {
                            let mut prepass_key = target_view_key
                                | MeshPipelineKey::from_bits_retain(mesh.key_bits.bits())
                                | alpha_mode_pipeline_key(
                                    alpha_mode,
                                    &Msaa::from_samples(target_view_key.msaa_samples()),
                                );
                            let forward = match material.properties.render_method {
                                OpaqueRendererMethod::Forward => true,
                                OpaqueRendererMethod::Deferred => false,
                                OpaqueRendererMethod::Auto => continue,
                            };
                            let deferred = target_view_key
                                .contains(MeshPipelineKey::DEFERRED_PREPASS)
                                && !forward;
                            if deferred {
                                prepass_key |= MeshPipelineKey::DEFERRED_PREPASS;
                            }
                            if let Some(bicubic) = lightmap {
                                prepass_key |= MeshPipelineKey::LIGHTMAPPED;
                                if bicubic && deferred {
                                    prepass_key |= MeshPipelineKey::LIGHTMAP_BICUBIC_SAMPLING;
                                }
                            }
                            if visibility_ranges
                                .entity_has_crossfading_visibility_ranges(visible_entity)
                            {
                                prepass_key |= MeshPipelineKey::VISIBILITY_RANGE_DITHER;
                            }
                            if material
                                .properties
                                .get_shader(PrepassVertexShader)
                                .is_some()
                                || material
                                    .properties
                                    .get_shader(PrepassFragmentShader)
                                    .is_some()
                            {
                                prepass_key |= MeshPipelineKey::PREPASS_READS_MATERIAL;
                            }
                            if target_view_key.contains(MeshPipelineKey::MOTION_VECTOR_PREPASS) {
                                if mesh_instance
                                    .flags()
                                    .contains(RenderMeshInstanceFlags::HAS_PREVIOUS_SKIN)
                                {
                                    prepass_key |= MeshPipelineKey::HAS_PREVIOUS_SKIN;
                                }
                                if mesh_instance
                                    .flags()
                                    .contains(RenderMeshInstanceFlags::HAS_PREVIOUS_MORPH)
                                {
                                    prepass_key |= MeshPipelineKey::HAS_PREVIOUS_MORPH;
                                }
                            }
                            prepass_key |= MeshPipelineKey::IRRADIANCE_VOLUME;
                            enqueue_prepass(
                                &mut warmup,
                                ErasedMaterialPipelineKey {
                                    type_id: material_instance.asset_id.type_id(),
                                    mesh_key: ErasedMeshPipelineKey::new(prepass_key),
                                    material_key: material.properties.material_key.clone(),
                                },
                                mesh.layout.clone(),
                                material.properties.clone(),
                            );
                        }
                    }
                    break 'views;
                }
            }

            // Grass is culled from high editor views, but it is the first
            // material encountered after entering walk mode. Seed its actual
            // mesh/material pairs for the main and planned planar-reflection
            // views without creating a hidden entity or changing the scene.
            if warmup.requests.is_empty() {
                'grass_views: for view_key in active_view_keys {
                    for (mesh_id, material_id) in &grass_candidates.pairs {
                        let Some(mesh) = render_meshes.get(mesh_id.typed()) else {
                            continue;
                        };
                        let Some(material) = render_materials.get(*material_id) else {
                            continue;
                        };
                        let mut material_bits: MeshPipelineKey =
                            material.properties.mesh_pipeline_key_bits.downcast();
                        material_bits.insert(alpha_mode_pipeline_key(
                            material.properties.alpha_mode,
                            &Msaa::from_samples(view_key.msaa_samples()),
                        ));
                        let normal_key = view_key
                            | MeshPipelineKey::from_bits_retain(mesh.key_bits.bits())
                            | material_bits;
                        for mesh_key in
                            [normal_key, normal_key | MeshPipelineKey::IRRADIANCE_VOLUME]
                        {
                            enqueue_base(
                                &mut warmup,
                                ErasedMaterialPipelineKey {
                                    type_id: material_id.type_id(),
                                    mesh_key: ErasedMeshPipelineKey::new(mesh_key),
                                    material_key: material.properties.material_key.clone(),
                                },
                                mesh.layout.clone(),
                                material.properties.clone(),
                            );
                        }
                        if !warmup.requests.is_empty() {
                            break 'grass_views;
                        }
                    }
                }
            }
            warmup.requests.pop_front()
        }
    };

    let Some(request) = item else {
        return;
    };
    let result = match request {
        WarmupRequest::Base {
            specialize,
            key,
            layout,
            properties,
        } => specialize(world, key, &layout, &properties),
        WarmupRequest::Prepass {
            specialize,
            key,
            layout,
            properties,
        } => specialize(world, &key, &layout, &properties),
    };
    match result {
        Ok(id) => {
            let mut warmup = world.resource_mut::<PipelineWarmup>();
            warmup.pending.insert(id);
            warmup.queued += 1;
            if warmup.verbose {
                info!(
                    target: "world_builder::pipeline_warmup",
                    queued = warmup.queued,
                    pending = warmup.pending.len(),
                    "queued bounce-light PBR variant"
                );
            }
        }
        Err(error) => warn!(
            target: "world_builder::pipeline_warmup",
            ?error,
            "could not warm bounce-light PBR variant"
        ),
    }
}

pub struct PipelineWarmupPlugin;

impl Plugin for PipelineWarmupPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ExtractComponentPlugin::<WarmupLightmap>::default())
            .add_plugins(ExtractResourcePlugin::<PipelineWarmupGate>::default())
            .add_plugins(ExtractResourcePlugin::<GrassPipelineCandidates>::default())
            .init_resource::<PipelineWarmupGate>()
            .add_systems(PostUpdate, (mirror_lightmaps, update_warmup_gate));
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .init_gpu_resource::<PipelineWarmup>()
            .add_systems(
                Render,
                warm_irradiance_volume_pipelines
                    .after(RenderSystems::Specialize)
                    .before(RenderSystems::QueueMeshes),
            );
    }
}
