//! Saving without stalling the editor. The main thread only takes a snapshot: the document
//! (everything but the big arrays), copies of the terrain grids and the objects encoded into
//! their binary table (in parallel), a few milliseconds even for 750 000 objects. Compressing,
//! hashing and writing happen on a background thread; only files whose content changed are
//! rewritten, each atomically (temp + rename). One background save runs at a time.

use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use super::*;

/// Everything a save writes, owned (no borrows of the world).
pub(super) struct Snapshot {
    dir: PathBuf,
    doc: WorldDoc,
    heights: Vec<f32>,
    splat: Vec<[u8; LAYER_COUNT]>,
    zones: Vec<u8>,
    biomes: Vec<u8>,
    /// the raw (uncompressed) object table
    objects: Arc<Vec<u8>>,
    /// procedural nature's density maps (`nature.bin`), when anything is painted
    nature: Option<Arc<Vec<u8>>>,
    /// main-thread time spent taking it (ms)
    pub ms: f32,
}

impl Snapshot {
    pub(super) fn take(dir: &Path, data: &WorldData) -> Snapshot {
        let t0 = Instant::now();
        let t = &data.terrain;
        let imports: Vec<ImportedAsset> = data
            .lib
            .as_ref()
            .map(|l| {
                // (models the user imported: the bundled libraries are rebuilt at startup)
                l.entries
                    .iter()
                    .filter(|e| e.category == crate::objects::Category::Imported)
                    .filter_map(|e| {
                        e.import_path.as_ref().map(|p| ImportedAsset {
                            id: e.id.clone(),
                            name: e.name.clone(),
                            path: p.clone(),
                            surface: e.surface,
                            walkable: e.walkable,
                            radius: e.radius,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let objects = object_table(&data.store);
        let (procedural, nature) = crate::nature::snapshot(&data.nature, *data.appearance);
        let doc = WorldDoc {
            version: 1,
            name: data.meta.name.clone(),
            seed: data.meta.seed,
            res: t.res,
            cell: t.cell,
            sea_level: t.sea_level,
            style: t.style.clone(),
            lighting: data.map.lighting_to_save(&data.lighting),
            water_style: data.water_style.clone(),
            wind: vec![
                data.wind.dir.x,
                data.wind.dir.y,
                data.wind.speed,
                data.wind.gustiness,
            ],
            graphics: data.map.graphics_to_save(&data.graphics),
            nature: data.amounts.clone(),
            building_materials: data.surface_choices.clone(),
            terrain_layers: data.layer_choices.clone(),
            art_style: Some(data.meta.art_style),
            zones: data.zones.list.clone(),
            rivers: data.water.rivers.clone(),
            lakes: data.water.lakes.clone(),
            fish: data.water.fish.clone(),
            waterfalls: data.water.waterfalls.clone(),
            gameplay: data.gameplay.clone(),
            roads: data.roads.clone(),
            fields: data.fields.clone(),
            foundations: data.fit.found.clone(),
            analysis: data.analysis.settings.clone(),
            motion: data.motion.clone(),
            caves: (*data.caves).clone(),
            dungeons: (*data.dungeons).clone(),
            dungeon_map: data.dungeon_map.0.clone(),
            map: data.map.settings.clone(),
            reference: crate::refmap::doc_to_save(&data.reference, dir),
            reference_effects: data.legacy_effects.reference_effects.clone(),
            effects_studio: data.legacy_effects.effects_studio.clone(),
            // (older versions of the editor read an empty list here)
            objects: vec![],
            objects_file: Some("objects.bin".to_string()),
            procedural,
            imports,
            layer_surface: data.ed.layer_surface.to_vec(),
            camera: CameraDoc {
                focus: data.orbit.focus.to_array(),
                yaw: data.orbit.yaw,
                pitch: data.orbit.pitch,
                distance: data.orbit.distance,
            },
        };
        Snapshot {
            dir: dir.to_path_buf(),
            doc,
            heights: t.heights.clone(),
            splat: t.splat.clone(),
            zones: t.zones.clone(),
            biomes: t.biomes.clone(),
            objects,
            nature,
            ms: t0.elapsed().as_secs_f32() * 1000.0,
        }
    }

    /// Writes the files that changed (any thread).
    pub(super) fn write(self) -> Result<(), String> {
        let dir = &self.dir;
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        // the table is compressed only when its content changed
        let path = dir.join("objects.bin");
        let raw_hash = content_hash(&self.objects);
        let unchanged = RAW_WRITTEN
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(|m| m.get(&path))
            == Some(&raw_hash)
            && path.exists();
        if !unchanged {
            write_if_changed(&path, &crate::bigworld::pack_objects(&self.objects))
                .map_err(|e| e.to_string())?;
            RAW_WRITTEN
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_or_insert_with(HashMap::new)
                .insert(path, raw_hash);
        }
        match &self.nature {
            Some(b) => write_if_changed(&dir.join("nature.bin"), b).map_err(|e| e.to_string())?,
            None => {
                let _ = fs::remove_file(dir.join("nature.bin"));
            }
        }
        let json = serde_json::to_string_pretty(&self.doc).map_err(|e| e.to_string())?;
        write_if_changed(&dir.join("world.json"), json.as_bytes()).map_err(|e| e.to_string())?;
        write_raw(&dir.join("heights.f32"), &self.heights).map_err(|e| e.to_string())?;
        write_if_changed(&dir.join("splat.u8"), bytemuck::cast_slice(&self.splat))
            .map_err(|e| e.to_string())?;
        write_if_changed(&dir.join("zones.u8"), &self.zones).map_err(|e| e.to_string())?;
        write_if_changed(&dir.join("biomes.u8"), &self.biomes).map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// The raw object table of the store at a revision: objects that didn't change since the last
/// save cost nothing to snapshot again.
static TABLE: Mutex<Option<(u64, Arc<Vec<u8>>)>> = Mutex::new(None);

fn object_table(store: &ObjectStore) -> Arc<Vec<u8>> {
    let mut cache = TABLE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((rev, t)) = cache.as_ref()
        && *rev == store.revision
    {
        return t.clone();
    }
    // in the map's memory order (a linear scan, ~2 ms for 750 000); encoded in parallel and
    // sorted by id later, on the save thread
    let items: Vec<&Placed> = store.items.values().collect();
    let t = Arc::new(crate::bigworld::encode_objects(&items));
    *cache = Some((store.revision, t.clone()));
    t
}

/// Hashes of the raw object tables this session wrote (skips compressing an unchanged table).
static RAW_WRITTEN: Mutex<Option<HashMap<PathBuf, u64>>> = Mutex::new(None);

/// A finished background save.
pub(super) struct Done {
    pub dir: PathBuf,
    pub in_place: bool,
    /// `WorldMeta::revision` when the snapshot was taken
    pub revision: u64,
    pub why: &'static str,
    pub snapshot_ms: f32,
    pub write_ms: f32,
    pub result: Result<(), String>,
}

struct Running {
    done: Done,
    started: Instant,
    thread: JoinHandle<Result<(), String>>,
}

static RUNNING: Mutex<Option<Running>> = Mutex::new(None);

/// Writes a snapshot on a background thread.
pub(super) fn start(snap: Snapshot, in_place: bool, revision: u64, why: &'static str) {
    let done = Done {
        dir: snap.dir.clone(),
        in_place,
        revision,
        why,
        snapshot_ms: snap.ms,
        write_ms: 0.0,
        result: Ok(()),
    };
    let thread = match std::thread::Builder::new()
        .name("world-save".into())
        .spawn(move || snap.write())
    {
        Ok(t) => t,
        Err(e) => {
            warn!("could not start a background save: {e}");
            return;
        }
    };
    *RUNNING.lock().unwrap_or_else(|e| e.into_inner()) = Some(Running {
        done,
        started: Instant::now(),
        thread,
    });
}

fn join(r: Running) -> Done {
    let mut d = r.done;
    d.result = r
        .thread
        .join()
        .unwrap_or_else(|_| Err("the save thread crashed".into()));
    d.write_ms = r.started.elapsed().as_secs_f32() * 1000.0;
    d
}

/// The background save, if it has finished.
pub(super) fn finished() -> Option<Done> {
    let mut g = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    if g.as_ref().is_some_and(|r| r.thread.is_finished()) {
        g.take().map(join)
    } else {
        None
    }
}

/// Waits for the background save (if any) to finish.
pub(super) fn wait() -> Option<Done> {
    let r = RUNNING.lock().unwrap_or_else(|e| e.into_inner()).take();
    r.map(join)
}

#[cfg(test)]
mod tests {
    use crate::bigworld::{decode_objects, encode_objects, pack_objects};
    use crate::objects::Placed;

    fn sample(n: u64) -> Vec<Placed> {
        (1..=n)
            .map(|id| Placed {
                group: 0,
                id,
                asset: ["oak", "pine", "rock"][(id % 3) as usize].into(),
                variant: (id % 4) as u32,
                pos: [id as f32 * 1.5, 10.0 + (id % 7) as f32, -(id as f32)],
                rot: [0.0, (id as f32 * 0.1).sin(), 0.0, (id as f32 * 0.1).cos()],
                scale: 1.0 + (id % 5) as f32 * 0.1,
                stretch: (id % 10 == 0).then_some([1.0, 0.5, 0.8]),
                locked: false,
            })
            .collect()
    }

    #[test]
    fn object_table_round_trips_with_stretch_and_reads_v1() {
        let objs = sample(50_000);
        // in any order (as a hash map hands them out): the packed table is sorted by id
        let mut refs: Vec<&Placed> = objs.iter().collect();
        refs.reverse();
        let raw = encode_objects(&refs);
        let packed = pack_objects(&raw);
        assert!(
            packed.len() * 3 < raw.len(),
            "packed {} vs raw {}",
            packed.len(),
            raw.len()
        );
        for bytes in [&raw, &packed] {
            let mut back = decode_objects(bytes).unwrap();
            assert_eq!(back.len(), objs.len());
            back.sort_by_key(|o| o.id);
            for (a, b) in objs.iter().zip(&back) {
                assert_eq!(
                    (a.id, &a.asset, a.variant, a.pos, a.rot, a.scale, a.stretch),
                    (b.id, &b.asset, b.variant, b.pos, b.rot, b.scale, b.stretch)
                );
            }
        }
        // the same objects in another order give the same bytes (autosave skips unchanged files)
        refs.reverse();
        assert_eq!(pack_objects(&encode_objects(&refs)), packed);
        // version 1 (48-byte rows, no stretch)
        let mut v1 = b"WBO1".to_vec();
        v1.extend_from_slice(&1u32.to_le_bytes());
        v1.extend_from_slice(&3u32.to_le_bytes());
        v1.extend_from_slice(b"oak");
        v1.extend_from_slice(&1u32.to_le_bytes());
        v1.extend_from_slice(&42u64.to_le_bytes());
        v1.extend_from_slice(&0u32.to_le_bytes());
        v1.extend_from_slice(&2u32.to_le_bytes());
        for v in [1.0f32, 2.0, 3.0, 0.0, 0.0, 0.0, 1.0, 1.5] {
            v1.extend_from_slice(&v.to_le_bytes());
        }
        let o = &decode_objects(&v1).unwrap()[0];
        assert_eq!(
            (o.id, o.asset.as_str(), o.variant, o.pos, o.scale, o.stretch),
            (42, "oak", 2, [1.0, 2.0, 3.0], 1.5, None)
        );
        assert!(decode_objects(&packed[..packed.len() / 2]).is_err());
    }

    /// Sizes and timings of the table for a real world's objects (an old world.json):
    /// `WB_BENCH_WORLD=<dir.world> cargo test --bin world_builder objects_bench -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn objects_bench() {
        let Ok(dir) = std::env::var("WB_BENCH_WORLD") else {
            return;
        };
        #[derive(serde::Deserialize)]
        struct Doc {
            objects: Vec<Placed>,
        }
        let t = std::time::Instant::now();
        let json = std::fs::read(std::path::Path::new(&dir).join("world.json")).unwrap();
        let doc: Doc = serde_json::from_slice(&json).unwrap();
        println!(
            "world.json {:.1} MB, {} objects, parsed in {:?}",
            json.len() as f64 / 1e6,
            doc.objects.len(),
            t.elapsed()
        );
        let mut store = crate::objects::ObjectStore::default();
        for o in doc.objects.iter().cloned() {
            store.insert(o);
        }
        let t = std::time::Instant::now();
        let refs: Vec<&Placed> = store.items.values().collect();
        let seq = t.elapsed();
        let gather = seq;
        let t = std::time::Instant::now();
        let raw = encode_objects(&refs);
        let enc = t.elapsed();
        let t = std::time::Instant::now();
        let packed = pack_objects(&raw);
        let pack = t.elapsed();
        let t = std::time::Instant::now();
        let back = decode_objects(&packed).unwrap();
        let dec = t.elapsed();
        let plain = miniz_oxide::deflate::compress_to_vec(&raw, 2);
        println!(
            "gather {gather:?}, encode {enc:?} ({:.1} MB raw), pack {pack:?} ({:.1} MB; deflate without the shuffle {:.1} MB), decode {dec:?}",
            raw.len() as f64 / 1e6,
            packed.len() as f64 / 1e6,
            plain.len() as f64 / 1e6
        );
        assert_eq!(back.len(), doc.objects.len());
    }
}
