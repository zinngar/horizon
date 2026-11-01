//! The saved NBT a converted world rewrites: chunks (1.13 to 26.3 layouts),
//! entity chunks, POI chunks, players and `level.dat`. Each function returns
//! whether it changed anything, so untouched chunks keep their bytes.
use super::ids::{Kind, Provided, Remap, State, full, namespace};
use horizon_world::nbt::{Compound, JavaString, Tag};
use std::collections::BTreeMap;

/// The first data version whose items save `count` and `components`
/// (1.20.5), and the first whose text components are NBT, not JSON (1.21.5).
const COMPONENTS: i32 = 3837;
const NBT_TEXT: i32 = 4325;

/// Keys loaders add to chunks, entities and items to hold their own data:
/// nothing in Horizon reads them, and they hold modded ids.
const LOADER_DATA: &[&str] = &[
    "ForgeCaps",
    "ForgeData",
    "NeoForgeData",
    "neoforge:attachments",
    "fabric:attachments",
    "cardinal_components",
];

#[derive(Debug, Clone, Default)]
pub struct Seen {
    /// What it became: `kept`, `removed`, or the vanilla id.
    pub became: String,
    pub count: u64,
}

/// Every modded id met, by kind, and what became of it.
pub type Ledger = BTreeMap<(Kind, String), Seen>;

pub struct Ctx<'a> {
    pub provided: &'a Provided,
    pub remap: &'a Remap,
    pub ledger: &'a mut Ledger,
    /// The data version of what is being converted (its own, or the world's).
    pub data_version: i32,
    /// What an unkept biome becomes in this dimension.
    pub biome: &'static str,
}

impl Ctx<'_> {
    fn note(&mut self, kind: Kind, id: &str, became: impl Into<String>) {
        if namespace(id) == "minecraft" {
            return;
        }
        let seen = self.ledger.entry((kind, full(id))).or_default();
        seen.became = became.into();
        seen.count += 1;
    }

    /// Whether `id` stays, noting a kept modded one.
    fn keeps(&mut self, kind: Kind, id: &str) -> bool {
        let keep = self.provided.keeps(kind, id);
        if keep {
            self.note(kind, id, "kept");
        }
        keep
    }
}

/// An id in a string tag set to its renamed namespace's: the id it holds
/// now, and whether it changed.
fn renamed(tag: &mut Tag, cx: &Ctx) -> Option<(String, bool)> {
    let id = string(Some(&*tag))?;
    match cx.provided.rename(&id) {
        Some(new) => {
            *tag = Tag::String(new.as_str().into());
            Some((new, true))
        }
        None => Some((id, false)),
    }
}

fn key(name: &str) -> JavaString {
    JavaString::from(name)
}

fn string(tag: Option<&Tag>) -> Option<String> {
    match tag {
        Some(Tag::String(s)) => Some(s.to_string_lossy()),
        _ => None,
    }
}

fn compound<'t>(map: &'t mut Compound, name: &str) -> Option<&'t mut Compound> {
    match map.get_mut(&key(name)) {
        Some(Tag::Compound(c)) => Some(c),
        _ => None,
    }
}

fn list<'t>(map: &'t mut Compound, name: &str) -> Option<&'t mut Vec<Tag>> {
    match map.get_mut(&key(name)) {
        Some(Tag::List(l)) => Some(l),
        _ => None,
    }
}

/// A region chunk: blocks, biomes, block entities, ticks, structures, and
/// (before 1.17) its entities. Turns light off on change, so the game lights
/// it again around the blocks that changed.
pub fn chunk(root: &mut Tag, cx: &mut Ctx) -> bool {
    let Tag::Compound(root) = root else {
        return false;
    };
    if let Some(Tag::Int(v)) = root.get(&key("DataVersion")) {
        cx.data_version = *v;
    }
    // 1.18 moved everything from `Level` to the root and renamed it.
    let modern = root.contains_key(&key("sections"));
    let level = if modern {
        root
    } else {
        match compound(root, "Level") {
            Some(level) => level,
            None => return false,
        }
    };
    let names = |modern_name: &'static str, old: &'static str| {
        if modern { modern_name } else { old }
    };
    let mut changed = false;
    if let Some(sections) = list(level, names("sections", "Sections")) {
        for section in sections {
            let Tag::Compound(section) = section else {
                continue;
            };
            let palette = if modern {
                compound(section, "block_states").and_then(|s| list(s, "palette"))
            } else {
                list(section, "Palette")
            };
            if let Some(palette) = palette {
                for entry in palette {
                    changed |= block_state(entry, cx);
                }
            }
            if let Some(biomes) = compound(section, "biomes").and_then(|s| list(s, "palette")) {
                for biome in biomes {
                    let Some((id, r)) = renamed(biome, cx) else {
                        continue;
                    };
                    changed |= r;
                    if !cx.keeps(Kind::Biome, &id) {
                        let to = cx.remap.biome(&full(&id), cx.biome);
                        cx.note(Kind::Biome, &id, to.clone());
                        *biome = Tag::String(to.as_str().into());
                        changed = true;
                    }
                }
            }
        }
    }
    if let Some(entities) = list(level, names("block_entities", "TileEntities")) {
        let before = entities.len();
        entities.retain(|e| {
            let id = string(e.get("id")).unwrap_or_default();
            id.is_empty() || {
                let keep = cx.keeps(Kind::BlockEntity, &id);
                if !keep {
                    cx.note(Kind::BlockEntity, &id, "removed");
                }
                keep
            }
        });
        changed |= entities.len() != before;
        for e in entities {
            changed |= items(e, cx);
        }
    }
    if !modern && let Some(entities) = list(level, "Entities") {
        changed |= entity_list(entities, cx);
    }
    for ticks in [
        names("block_ticks", "TileTicks"),
        names("fluid_ticks", "LiquidTicks"),
    ] {
        if let Some(ticks) = list(level, ticks) {
            let before = ticks.len();
            ticks.retain_mut(|t| {
                let id = t.get_mut("i").and_then(|i| renamed(i, cx));
                match id {
                    Some((id, _)) if !cx.provided.keeps(Kind::Block, &id) => {
                        cx.note(Kind::Tick, &id, "removed");
                        false
                    }
                    Some((_, r)) => {
                        changed |= r;
                        true
                    }
                    None => true,
                }
            });
            changed |= ticks.len() != before;
        }
    }
    if let Some(structures) = compound(level, names("structures", "Structures")) {
        for part in ["starts", "Starts", "References"] {
            if let Some(map) = compound(structures, part) {
                let renames: Vec<_> = map
                    .keys()
                    .filter_map(|k| Some((k.clone(), cx.provided.rename(&k.to_string_lossy())?)))
                    .collect();
                for (old, new) in renames {
                    if let Some(v) = map.remove(&old) {
                        map.insert(key(&new), v);
                        changed = true;
                    }
                }
                let before = map.len();
                map.retain(|id, _| {
                    let id = id.to_string_lossy();
                    let keep = cx.provided.keeps(Kind::Structure, &id);
                    if !keep && part != "References" {
                        cx.note(Kind::Structure, &id, "removed");
                    }
                    keep
                });
                changed |= map.len() != before;
            }
        }
    }
    for k in LOADER_DATA {
        changed |= level.remove(&key(k)).is_some();
    }
    if changed {
        level.insert(key("isLightOn"), Tag::Byte(0));
    }
    changed
}

/// A palette entry (`{Name, Properties}`).
fn block_state(entry: &mut Tag, cx: &mut Ctx) -> bool {
    let Tag::Compound(map) = entry else {
        return false;
    };
    let Some((id, r)) = map.get_mut(&key("Name")).and_then(|n| renamed(n, cx)) else {
        return false;
    };
    if cx.keeps(Kind::Block, &id) {
        return r;
    }
    let from: BTreeMap<String, String> = match map.get(&key("Properties")) {
        Some(Tag::Compound(p)) => p
            .iter()
            .filter_map(|(k, v)| Some((k.to_string_lossy(), string(Some(v))?)))
            .collect(),
        _ => BTreeMap::new(),
    };
    let mut to: State = cx.remap.block(&full(&id));
    to.carry(&from);
    cx.note(Kind::Block, &id, to.text());
    map.clear();
    map.insert(key("Name"), Tag::String(to.name.as_str().into()));
    if !to.properties.is_empty() {
        let props = to
            .properties
            .iter()
            .map(|(k, v)| (key(k), Tag::String(v.as_str().into())))
            .collect();
        map.insert(key("Properties"), Tag::Compound(props));
    }
    true
}

/// An entity list: modded entities go, with their passengers; the rest have
/// their items and passengers converted.
pub fn entity_list(entities: &mut Vec<Tag>, cx: &mut Ctx) -> bool {
    let before = entities.len();
    entities.retain(|e| match string(e.get("id")) {
        Some(id) if !cx.keeps(Kind::Entity, &id) => {
            cx.note(Kind::Entity, &id, "removed");
            false
        }
        _ => true,
    });
    let mut changed = entities.len() != before;
    for e in entities.iter_mut() {
        if let Tag::Compound(map) = e
            && let Some(passengers) = list(map, "Passengers")
        {
            changed |= entity_list(passengers, cx);
        }
        changed |= items(e, cx);
    }
    changed
}

/// An entity chunk (`entities/`, 1.17 on).
pub fn entity_chunk(root: &mut Tag, cx: &mut Ctx) -> bool {
    let Tag::Compound(root) = root else {
        return false;
    };
    if let Some(Tag::Int(v)) = root.get(&key("DataVersion")) {
        cx.data_version = *v;
    }
    list(root, "Entities").is_some_and(|e| entity_list(e, cx))
}

/// A POI chunk (`poi/`): records of modded types go.
pub fn poi_chunk(root: &mut Tag, cx: &mut Ctx) -> bool {
    let Tag::Compound(root) = root else {
        return false;
    };
    let mut changed = false;
    let Some(sections) = compound(root, "Sections") else {
        return false;
    };
    for section in sections.values_mut() {
        let Tag::Compound(section) = section else {
            continue;
        };
        if let Some(records) = list(section, "Records") {
            let before = records.len();
            records.retain(|r| match string(r.get("type")) {
                Some(id) if !cx.keeps(Kind::Poi, &id) => {
                    cx.note(Kind::Poi, &id, "removed");
                    false
                }
                _ => true,
            });
            changed |= records.len() != before;
        }
    }
    changed
}

/// Every item stack under `tag`, wherever it is (containers, inventories,
/// equipment, items in items), and the loaders' own data on the way.
pub fn items(tag: &mut Tag, cx: &mut Ctx) -> bool {
    let mut changed = false;
    match tag {
        Tag::Compound(map) => {
            for k in LOADER_DATA {
                changed |= map.remove(&key(k)).is_some();
            }
            if is_stack(map) {
                match stack(map, cx) {
                    // The replaced item is saved inside its placeholder as it was.
                    None => return true,
                    Some(c) => changed |= c,
                }
            }
            for (k, child) in map.iter_mut() {
                if *k != key(MISSING) {
                    changed |= items(child, cx);
                }
            }
        }
        Tag::List(l) => {
            for child in l {
                changed |= items(child, cx);
            }
        }
        _ => {}
    }
    changed
}

fn is_stack(map: &Compound) -> bool {
    matches!(map.get(&key("id")), Some(Tag::String(_)))
        && (map.contains_key(&key("Count")) || map.contains_key(&key("count")))
}

/// Where a placeholder keeps the item it stands for.
const MISSING: &str = "horizon_missing";

/// One item stack: `None` when it was replaced, else whether it changed.
fn stack(map: &mut Compound, cx: &mut Ctx) -> Option<bool> {
    let (id, r) = map
        .get_mut(&key("id"))
        .and_then(|i| renamed(i, cx))
        .unwrap_or_default();
    if cx.keeps(Kind::Item, &id) {
        // A kept item loses the components of mods that aren't.
        let Some(components) = compound(map, "components") else {
            return Some(r);
        };
        let before = components.len();
        components.retain(|k, _| {
            let k = k.to_string_lossy();
            cx.provided.keeps(Kind::Item, k.trim_start_matches('!'))
                || namespace(k.trim_start_matches('!')) == "minecraft"
        });
        return Some(r || components.len() != before);
    }
    let original = Tag::Compound(map.clone());
    let count = map
        .get(&key("count"))
        .or_else(|| map.get(&key("Count")))
        .cloned()
        .unwrap_or(Tag::Byte(1));
    let count_key = if map.contains_key(&key("count")) {
        "count"
    } else {
        "Count"
    };
    map.clear();
    map.insert(key(count_key), count);
    match cx.remap.item(&full(&id)) {
        Some(to) => {
            cx.note(Kind::Item, &id, to.clone());
            map.insert(key("id"), Tag::String(to.as_str().into()));
        }
        None => {
            // Paper named after the item, the item itself kept inside it:
            // nothing is worth more than it was, and nothing is lost.
            cx.note(Kind::Item, &id, "minecraft:paper (named)");
            map.insert(key("id"), Tag::String("minecraft:paper".into()));
            let name = format!("Missing: {}", full(&id));
            let saved = Tag::compound([(MISSING, original)]);
            if cx.data_version >= COMPONENTS {
                let name = if cx.data_version >= NBT_TEXT {
                    Tag::String(name.as_str().into())
                } else {
                    Tag::String(json_text(&name).as_str().into())
                };
                map.insert(
                    key("components"),
                    Tag::compound([
                        ("minecraft:custom_name", name),
                        ("minecraft:custom_data", saved),
                    ]),
                );
            } else {
                let Tag::Compound(mut tag) = saved else {
                    unreachable!()
                };
                tag.insert(
                    key("display"),
                    Tag::compound([("Name", Tag::String(json_text(&name).as_str().into()))]),
                );
                map.insert(key("tag"), Tag::Compound(tag));
            }
        }
    }
    None
}

fn json_text(text: &str) -> String {
    serde_json::json!({ "text": text, "italic": false }).to_string()
}

/// A player (`playerdata/*.dat`, or `level.dat`'s `Player`): items, and a
/// player in a dimension that goes is moved to the world spawn.
pub fn player(tag: &mut Tag, spawn: Option<[f64; 3]>, cx: &mut Ctx) -> bool {
    let mut changed = items(tag, cx);
    let Tag::Compound(map) = tag else {
        return changed;
    };
    let dimension = string(map.get(&key("Dimension"))).unwrap_or_default();
    if !dimension.is_empty() && !cx.provided.keeps(Kind::Dimension, &dimension) {
        cx.note(Kind::Dimension, &dimension, "removed");
        map.insert(key("Dimension"), Tag::String("minecraft:overworld".into()));
        if let Some(spawn) = spawn {
            let pos = spawn.iter().map(|v| Tag::Double(v.to_bits())).collect();
            map.insert(key("Pos"), Tag::List(pos));
        }
        changed = true;
    }
    changed
}

/// Data packs a loader enables for its mods (`mod:<id>`, `fabric`...): gone
/// with the loader, so the world doesn't ask for them.
fn vanilla_pack(name: &str) -> bool {
    name == "vanilla"
        || name.starts_with("file/")
        || matches!(
            name,
            "bundle"
                | "trade_rebalance"
                | "redstone_experiments"
                | "minecart_improvements"
                | "winter_drop"
                | "update_1_20"
                | "update_1_21"
        )
}

/// `level.dat`: the loaders' data, their data packs, and modded dimensions
/// go; the single player's items are converted. Returns the spawn point.
pub fn level(root: &mut Tag, cx: &mut Ctx) -> Option<[f64; 3]> {
    let Tag::Compound(root) = root else {
        return None;
    };
    // Forge keeps its registries beside `Data` (`FML`, `forge`).
    root.retain(|k, _| k.to_string_lossy() == "Data");
    let data = compound(root, "Data")?;
    for k in ["fml", "FML", "forge", "Forge"].iter().chain(LOADER_DATA) {
        data.remove(&key(k));
    }
    if let Some(packs) = compound(data, "DataPacks") {
        for part in ["Enabled", "Disabled"] {
            if let Some(names) = list(packs, part) {
                names.retain(|n| string(Some(n)).is_none_or(|n| vanilla_pack(&n)));
            }
        }
    }
    if let Some(dimensions) =
        compound(data, "WorldGenSettings").and_then(|w| compound(w, "dimensions"))
    {
        dimensions.retain(|id, _| {
            let id = id.to_string_lossy();
            let keep = cx.provided.keeps(Kind::Dimension, &id);
            if !keep {
                cx.note(Kind::Dimension, &id, "removed");
            }
            keep
        });
    }
    let spawn = spawn(data);
    if let Some(player) = data.get_mut(&key("Player")) {
        self::player(player, spawn, cx);
    }
    spawn
}

/// The world spawn: `SpawnX/Y/Z` until 1.21.5, `spawn.pos` after.
fn spawn(data: &Compound) -> Option<[f64; 3]> {
    let int = |k: &str| match data.get(&key(k)) {
        Some(Tag::Int(v)) => Some(f64::from(*v)),
        _ => None,
    };
    let block = if let (Some(x), Some(y), Some(z)) = (int("SpawnX"), int("SpawnY"), int("SpawnZ")) {
        [x, y, z]
    } else {
        match data.get(&key("spawn")).and_then(|s| s.get("pos")) {
            Some(Tag::IntArray(p)) if p.len() == 3 => {
                [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])]
            }
            _ => return None,
        }
    };
    Some([block[0] + 0.5, block[1], block[2] + 0.5])
}
