use super::*;

fn outline() -> Vec<Vec2> {
    vec![
        Vec2::new(-15.0, -15.0),
        Vec2::new(0.0, -15.0),
        Vec2::new(15.0, -15.0),
        Vec2::new(15.0, 0.0),
        Vec2::new(15.0, 15.0),
        Vec2::new(0.0, 15.0),
        Vec2::new(-15.0, 15.0),
        Vec2::new(-15.0, 0.0),
    ]
}

#[test]
fn authored_lake_carves_to_its_final_water_level() {
    let mut terrain = Terrain::new(64, 1.0, 0.0);
    terrain.heights.fill(20.0);
    let mut history = History::default();
    let mut store = ObjectStore::default();
    let mut water = WaterBodies::default();
    let raw: Vec<Vec2> = outline().into_iter().step_by(2).collect();
    let automatic = lakes::level_for(&terrain, &lakes::smooth_outline(&outline()).unwrap());
    assert!(automatic > 10.0);
    create_drawn_lake_at_level(
        &mut terrain,
        &mut history,
        &mut store,
        None,
        &mut water,
        &raw,
        lakes::shape_by_id("pond"),
        1.0,
        0.0,
        Some(5.0),
    )
    .unwrap();
    assert_eq!(water.lakes[0].level, 5.0);
    assert!(terrain.height_at(0.0, 0.0) < 4.5);
    assert_eq!(terrain.height_at(31.0, 31.0), 20.0);
}

#[test]
fn invalid_authored_level_does_not_mutate_the_world() {
    let mut terrain = Terrain::new(64, 1.0, 0.0);
    terrain.heights.fill(20.0);
    let before = terrain.heights.clone();
    let mut history = History::default();
    let mut store = ObjectStore::default();
    let mut water = WaterBodies::default();
    assert!(
        create_drawn_lake_at_level(
            &mut terrain,
            &mut history,
            &mut store,
            None,
            &mut water,
            &outline(),
            lakes::shape_by_id("pond"),
            1.0,
            0.0,
            Some(f32::NAN),
        )
        .is_err()
    );
    assert_eq!(terrain.heights, before);
    assert!(water.lakes.is_empty());
    assert_eq!(water.next_id, 0);
    assert!(store.items.is_empty());
}
