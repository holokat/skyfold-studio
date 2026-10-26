//! Ground palettes (`terrain::alien_ground`): the whole ground of the world at once.

use bevy::prelude::*;
use serde_json::{Value, json};

use crate::mcp_tools::{McpAppExt, McpContent, McpTool};
use crate::terrain::alien_ground::{self, GroundPalette, SCIFI_LAYERS};

pub fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "terrain_ground_palette",
        description: "Swap the whole world's ground look at once. `palette` scifi: an alien desert canyonland with no green anywhere — red desert dust and ochre sand on the flats, red-brown layered sandstone on slopes and cliffs, dark basalt grit and basalt, a cracked clay pan in hollows, red soil under woods, only sparse grass patches (crimson with world_kind_apply flora / alien_flora); also no snow caps, no damp mossy gullies, grass amount 0.3. `palette` fantasy: the default meadow ground of a new world. \
It sets the texture of each paint layer (materials_set ground), the automatic ground style and the grass amount, so every rule, biome and brush follows; nothing is painted and everything stays editable. Without `palette` it reports the current one.",
        input_schema: || {
            json!({ "type": "object", "properties": {
                "palette": { "type": "string", "enum": GroundPalette::ALL.map(|p| p.id()) }
            } })
        },
        run: |w, a: Value| {
            let layers: serde_json::Map<String, Value> = SCIFI_LAYERS
                .iter()
                .map(|(l, id)| (l.name().to_string(), json!(id)))
                .collect();
            let Some(p) = a.get("palette").and_then(|v| v.as_str()) else {
                let c = w.resource::<crate::terrain::textures::TerrainLayerChoices>();
                return Ok(vec![McpContent::Json(json!({
                    "palette": alien_ground::current(c).map_or("custom", |p| p.id()),
                    "scifi_layers": layers,
                }))]);
            };
            let palette = GroundPalette::from_id(p)
                .ok_or_else(|| format!("unknown palette `{p}` (fantasy or scifi)"))?;
            let done = alien_ground::apply(w, palette);
            Ok(vec![McpContent::Json(json!({ "palette": palette.id(), "done": done }))])
        },
    });
}
