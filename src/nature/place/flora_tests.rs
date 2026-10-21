//! Deterministic coverage for the independent flora abundance controls.

use super::*;
use crate::art_style::ArtStyle;
use crate::nature::field::{NatureField, PaintMode, Target};
use crate::nature::{NatureAppearance, Rules};
use crate::objects::{Library, LibraryEntry, PHOTOREAL, builtin_id, kind_category};
use crate::procgen::meshes::AssetKind;
use crate::terrain::Terrain;
use crate::terrain::layers::Surface;
use crate::water::WaterBodies;
use crate::world::{Biome, Zones};

const HALF: f32 = 128.0;
const SEED: u32 = 0x1a2b_3c4d;
const FLORA_KINDS: [AssetKind; 4] = [
    AssetKind::Fern,
    AssetKind::Flowers,
    AssetKind::Mushrooms,
    AssetKind::Blackberry,
];
const PINE_FOREST_FLORA: [AssetKind; 4] = [
    AssetKind::Flowers,
    AssetKind::WildStrawberry,
    AssetKind::Blueberry,
    AssetKind::Blackberry,
];

struct Fixture {
    terrain: Terrain,
    water: WaterBodies,
    zones: Zones,
    library: Library,
    rules: Rules,
    masks: Masks,
}

fn fixture() -> Fixture {
    let mut terrain = Terrain::new(64, 4.0, 0.0);
    terrain.heights.fill(20.0);

    let mut library = Library::empty_for_tests();
    for kind in FLORA_KINDS
        .into_iter()
        .chain([AssetKind::WildStrawberry, AssetKind::Blueberry])
    {
        library.add(LibraryEntry {
            id: builtin_id(kind),
            name: format!("{kind:?}"),
            category: kind_category(kind),
            group: String::new(),
            kind: Some(kind),
            import_path: None,
            surface: Surface::Grass,
            radius: 0.5,
            align: false,
            walkable: false,
            whole_scene: false,
            biomes: Biome::ALL.to_vec(),
            sway: true,
            default_scale: 1.0,
            meshes: vec![],
            scene: None,
            height: 1.0,
            tree: None,
            textured: false,
            pack: PHOTOREAL.into(),
        });
    }
    let rules = Rules::build(&library, ArtStyle::Realistic);
    Fixture {
        terrain,
        water: WaterBodies::default(),
        zones: Zones::default(),
        library,
        rules,
        masks: Masks::default(),
    }
}

fn painted_field(kinds: &[AssetKind]) -> NatureField {
    let mut field = NatureField {
        // With rule growth disabled, every instance in this fixture proves the explicit species
        // path uses the same family dial as ordinary rule-grown flora.
        rules_off: true,
        ..default()
    };
    field.paint_area(
        Vec2::splat(-HALF),
        Vec2::splat(HALF),
        |_| true,
        PaintMode::Grow,
        &Target::Species(kinds.to_vec()),
        1.0,
    );
    field
}

fn ruled_field(rules: &Rules) -> NatureField {
    let mut field = NatureField::default();
    let mixed_forest = rules
        .preset_index("Mixed Forest")
        .expect("built-in preset is available");
    field.paint_area(
        Vec2::splat(-HALF),
        Vec2::splat(HALF),
        |_| true,
        PaintMode::Grow,
        &Target::Preset(mixed_forest),
        1.0,
    );
    field
}

fn pine_forest_field(rules: &Rules) -> NatureField {
    let mut field = NatureField::default();
    let pine_forest = rules
        .preset_index("Pine Forest")
        .expect("built-in preset is available");
    field.paint_area(
        Vec2::splat(-HALF),
        Vec2::splat(HALF),
        |_| true,
        PaintMode::Grow,
        &Target::Preset(pine_forest),
        1.0,
    );
    field
}

fn placed(f: &Fixture, field: &NatureField, flora_amounts: [f32; 4]) -> Vec<Instance> {
    let ctx = Ctx {
        terrain: &f.terrain,
        water: &f.water,
        zones: &f.zones,
        lib: &f.library,
        field,
        rules: &f.rules,
        masks: &f.masks,
        amounts: [1.0; 4],
        flora_amounts,
        appearance: NatureAppearance::default(),
        seed: SEED,
    };
    place_rect(&ctx, Vec2::splat(-HALF), Vec2::splat(HALF), &[2])
}

fn family_count(instances: &[Instance], group: usize) -> usize {
    instances
        .iter()
        .filter(|i| crate::nature::rules::flora_group(i.kind) == Some(group))
        .count()
}

#[test]
fn rule_grown_flora_families_are_zero_one_two_monotonic() {
    let f = fixture();
    let field = ruled_field(&f.rules);
    for (group, kind) in FLORA_KINDS.into_iter().enumerate() {
        let mut zero = [1.0; 4];
        zero[group] = 0.0;
        let mut two = [1.0; 4];
        two[group] = 2.0;

        let none = family_count(&placed(&f, &field, zero), group);
        let natural_instances = placed(&f, &field, [1.0; 4]);
        let natural = family_count(&natural_instances, group);
        let doubled = family_count(&placed(&f, &field, two), group);

        assert_eq!(none, 0, "{kind:?} remained at zero abundance");
        assert!(natural > 0, "{kind:?} did not grow at natural abundance");
        assert!(
            doubled as f32 > natural as f32 * 1.6,
            "{kind:?} count fell from {natural} to {doubled} when abundance increased"
        );
    }
}

#[test]
fn explicit_painted_flora_families_are_zero_one_two_monotonic() {
    let f = fixture();
    for (group, kind) in FLORA_KINDS.into_iter().enumerate() {
        let field = painted_field(&[kind]);
        let mut zero = [1.0; 4];
        zero[group] = 0.0;
        let mut two = [1.0; 4];
        two[group] = 2.0;

        let none = family_count(&placed(&f, &field, zero), group);
        let natural_instances = placed(&f, &field, [1.0; 4]);
        let natural = family_count(&natural_instances, group);
        let doubled = family_count(&placed(&f, &field, two), group);

        assert_eq!(none, 0, "{kind:?} remained at zero abundance");
        assert!(natural > 0, "{kind:?} did not grow at natural abundance");
        assert!(
            doubled as f32 > natural as f32 * 1.6,
            "{kind:?} count fell from {natural} to {doubled} when abundance increased"
        );
        assert!(
            natural_instances
                .iter()
                .all(|instance| instance.kind == kind),
            "painted {kind:?} was replaced with another species"
        );
    }
}

#[test]
fn flowers_do_not_move_other_flora_instances() {
    let f = fixture();
    let field = painted_field(&FLORA_KINDS);
    let without_flowers = placed(&f, &field, [1.0, 0.0, 1.0, 1.0]);
    let with_flowers = placed(&f, &field, [1.0, 2.0, 1.0, 1.0]);
    let stable = |instances: Vec<Instance>| {
        instances
            .into_iter()
            .filter(|i| crate::nature::rules::flora_group(i.kind) != Some(1))
            .collect::<Vec<_>>()
    };

    assert_eq!(stable(without_flowers), stable(with_flowers));
}

#[test]
fn rules_off_painted_flora_honors_exclusions() {
    let f = fixture();
    let mut field = painted_field(&[AssetKind::Flowers]);
    let grown = placed(&f, &field, [1.0, 2.0, 1.0, 1.0]);
    assert!(field.rules_off);
    assert!(!grown.is_empty());

    let removed = grown[0].id;
    field.excluded.insert(removed);
    let after = placed(&f, &field, [1.0, 2.0, 1.0, 1.0]);
    assert_eq!(after.len(), grown.len() - 1);
    assert!(!after.iter().any(|i| i.id == removed));
}

#[test]
fn pine_forest_keeps_flower_and_berry_dials_independent_on_a_hillside() {
    let mut f = fixture();
    // Match the inspected taiga floor: 40.2 m above sea with a usable 21.6° incline.
    let grade = 21.6_f32.to_radians().tan();
    for z in 0..=f.terrain.res {
        for x in 0..=f.terrain.res {
            let world_x = x as f32 * f.terrain.cell - f.terrain.half();
            f.terrain.heights[z * (f.terrain.res + 1) + x] = 40.2 + world_x * grade;
        }
    }
    let slope = f.terrain.normal_at(0.0, 0.0).y.acos().to_degrees();
    assert!(
        (slope - 21.6).abs() < 0.1,
        "expected a 21.6° hillside, got {slope:.2}°"
    );

    let field = pine_forest_field(&f.rules);
    let full = placed(&f, &field, [1.0, 2.0, 1.0, 2.0]);
    for kind in PINE_FOREST_FLORA {
        assert!(
            full.iter().any(|instance| instance.kind == kind),
            "Pine Forest did not place {kind:?} at maximum flora abundance"
        );
    }
    assert!(
        full.iter()
            .filter(|instance| instance.kind == AssetKind::Flowers)
            .all(|instance| {
                let ground = f.terrain.height_at(instance.pos.x, instance.pos.z);
                (ground - instance.pos.y - 0.015 * instance.scale).abs() < 0.001
            }),
        "woodland flowers were not seated at their root origin"
    );

    let without_flowers = placed(&f, &field, [1.0, 0.0, 1.0, 2.0]);
    assert_eq!(family_count(&without_flowers, 1), 0);
    assert_eq!(
        full.iter()
            .filter(|instance| crate::nature::rules::flora_group(instance.kind) != Some(1))
            .collect::<Vec<_>>(),
        without_flowers
            .iter()
            .filter(|instance| crate::nature::rules::flora_group(instance.kind) != Some(1))
            .collect::<Vec<_>>(),
        "flower amount changed stable non-flower candidates"
    );

    let without_berries = placed(&f, &field, [1.0, 2.0, 1.0, 0.0]);
    assert_eq!(family_count(&without_berries, 3), 0);
    assert_eq!(
        full.iter()
            .filter(|instance| crate::nature::rules::flora_group(instance.kind) != Some(3))
            .collect::<Vec<_>>(),
        without_berries
            .iter()
            .filter(|instance| crate::nature::rules::flora_group(instance.kind) != Some(3))
            .collect::<Vec<_>>(),
        "berry amount changed stable non-berry candidates"
    );

    // Flowers tolerate a steeper wooded hillside than the berry layers.
    let flower_grade = 28.0_f32.to_radians().tan();
    for z in 0..=f.terrain.res {
        for x in 0..=f.terrain.res {
            let world_x = x as f32 * f.terrain.cell - f.terrain.half();
            f.terrain.heights[z * (f.terrain.res + 1) + x] = 40.2 + world_x * flower_grade;
        }
    }
    let flower_slope = f.terrain.normal_at(0.0, 0.0).y.acos().to_degrees();
    assert!(
        (flower_slope - 28.0).abs() < 0.1,
        "expected a 28° flower hillside, got {flower_slope:.2}°"
    );
    let flowers_on_steep_hillside = placed(&f, &field, [1.0, 2.0, 1.0, 2.0]);
    assert!(
        flowers_on_steep_hillside
            .iter()
            .any(|instance| instance.kind == AssetKind::Flowers),
        "Pine Forest flowers did not grow on a 28° wooded hillside"
    );
}
