//! Which namespaced ids a converted world keeps, and what the others become.
//!
//! An id is kept when it is vanilla's (`minecraft:`) or a content mod being
//! converted with the world declares it. Every other block, item and biome is
//! remapped: first by the player's `remap.toml`, then by a guess from its
//! name (a modded ore becomes stone, a modded log an oak log). Entities,
//! block entities and POI from mods are removed: content mods have no code,
//! so nothing would run them.
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::Path,
};

/// The namespace of `id`, `minecraft` when it has none.
pub fn namespace(id: &str) -> &str {
    id.split_once(':').map_or("minecraft", |(ns, _)| ns)
}

/// `id` with its namespace: `stone` is `minecraft:stone`.
pub fn full(id: &str) -> String {
    if id.contains(':') {
        id.to_owned()
    } else {
        format!("minecraft:{id}")
    }
}

/// What the converted mods add to the registries a world refers to.
#[derive(Debug, Default, Clone)]
pub struct Provided {
    pub blocks: BTreeSet<String>,
    pub items: BTreeSet<String>,
    pub biomes: BTreeSet<String>,
    pub structures: BTreeSet<String>,
    pub dimensions: BTreeSet<String>,
    /// Dashed namespaces and the ones their mods were converted under
    /// (`gem-mod` to `gem_mod`, [`crate::prepare`]).
    pub aliases: BTreeMap<String, String>,
}

impl Provided {
    /// A folder mod (one holding `horizon-mod.toml`).
    pub fn add_folder(&mut self, root: &Path) -> Result<(), String> {
        let mut stack = vec![root.join("data")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if let Ok(rel) = path.strip_prefix(root) {
                    let name = rel.to_string_lossy().replace('\\', "/");
                    self.add(&name, &mut || fs::read_to_string(&path).ok());
                }
            }
        }
        Ok(())
    }

    /// A packaged content mod (`.hmod`, a zip).
    pub fn add_zip(&mut self, path: &Path) -> Result<(), String> {
        let file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("{}: {e}", path.display()))?;
        let names: Vec<String> = zip
            .file_names()
            .map(|name| name.map(|name| name.into_owned()))
            .collect::<Result<_, _>>()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        for name in names {
            let mut read = || {
                let mut text = String::new();
                zip.by_name(&name).ok()?.read_to_string(&mut text).ok()?;
                Some(text)
            };
            self.add(&name, &mut read);
        }
        Ok(())
    }

    /// One file of a content mod, by its path from the mod's root.
    fn add(&mut self, name: &str, read: &mut dyn FnMut() -> Option<String>) {
        let Some(rest) = name.strip_prefix("data/") else {
            return;
        };
        let Some((ns, rest)) = rest.split_once('/') else {
            return;
        };
        let Some(rest) = rest.strip_suffix(".json") else {
            return;
        };
        let id = |path: &str| format!("{ns}:{path}");
        if let Some(path) = rest.strip_prefix("horizon/block/") {
            self.blocks.insert(id(path));
            // A block brings its item unless it says `"item": false`.
            let no_item = read()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .is_some_and(|v| v.get("item") == Some(&Value::Bool(false)));
            if !no_item {
                self.items.insert(id(path));
            }
        } else if let Some(path) = rest.strip_prefix("horizon/item/") {
            self.items.insert(id(path));
        } else if let Some(path) = rest.strip_prefix("worldgen/biome/") {
            self.biomes.insert(id(path));
        } else if let Some(path) = rest.strip_prefix("worldgen/structure/") {
            self.structures.insert(id(path));
        } else if let Some(path) = rest.strip_prefix("dimension/") {
            self.dimensions.insert(id(path));
        }
    }

    /// `id` in the namespace its mod was converted under, when renamed.
    pub fn rename(&self, id: &str) -> Option<String> {
        let (ns, path) = id.split_once(':')?;
        self.aliases.get(ns).map(|new| format!("{new}:{path}"))
    }

    pub fn keeps(&self, kind: Kind, id: &str) -> bool {
        let id = full(id);
        if namespace(&id) == "minecraft" {
            return true;
        }
        match kind {
            Kind::Block => self.blocks.contains(&id),
            Kind::Item => self.items.contains(&id),
            Kind::Biome => self.biomes.contains(&id),
            Kind::Structure => self.structures.contains(&id),
            Kind::Dimension => self.dimensions.contains(&id),
            // Content mods add none of these: they need code.
            Kind::Entity | Kind::BlockEntity | Kind::Poi | Kind::Tick => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Block,
    Item,
    Biome,
    Entity,
    BlockEntity,
    Structure,
    Poi,
    Tick,
    Dimension,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Block => "block",
            Kind::Item => "item",
            Kind::Biome => "biome",
            Kind::Entity => "entity",
            Kind::BlockEntity => "block entity",
            Kind::Structure => "structure",
            Kind::Poi => "point of interest",
            Kind::Tick => "scheduled tick",
            Kind::Dimension => "dimension",
        }
    }
}

/// The player's overrides: `remap.toml`, with a table per registry. A value
/// is a block state as commands write it (`minecraft:oak_log[axis=x]`), an
/// item id, or a biome id.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Remap {
    #[serde(default)]
    pub blocks: BTreeMap<String, String>,
    #[serde(default)]
    pub items: BTreeMap<String, String>,
    #[serde(default)]
    pub biomes: BTreeMap<String, String>,
}

impl Remap {
    pub fn read(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let remap: Self = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        for target in remap.blocks.values() {
            if namespace(&State::parse(target).name) != "minecraft" {
                return Err(format!(
                    "{}: {target}: remap to a vanilla block",
                    path.display()
                ));
            }
        }
        Ok(remap)
    }

    /// The block state an unkept block becomes.
    pub fn block(&self, id: &str) -> State {
        match self.blocks.get(id) {
            Some(target) => State::parse(target),
            None => State::parse(guess(id).unwrap_or("minecraft:stone")),
        }
    }

    /// The item an unkept item becomes; `None`: a named placeholder.
    pub fn item(&self, id: &str) -> Option<String> {
        match self.items.get(id) {
            Some(target) => Some(full(target)),
            None => guess(id)
                .map(|g| State::parse(g).name)
                .filter(|n| n != "minecraft:air"),
        }
    }

    pub fn biome(&self, id: &str, default: &str) -> String {
        self.biomes
            .get(id)
            .map_or_else(|| default.to_owned(), |b| full(b))
    }
}

/// A block state as a palette holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub name: String,
    pub properties: BTreeMap<String, String>,
}

impl State {
    /// `minecraft:oak_leaves[persistent=true]`.
    pub fn parse(text: &str) -> Self {
        let (name, props) = match text.split_once('[') {
            Some((name, rest)) => (name, rest.trim_end_matches(']')),
            None => (text, ""),
        };
        let properties = props
            .split(',')
            .filter_map(|kv| kv.split_once('='))
            .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
            .collect();
        Self {
            name: full(name.trim()),
            properties,
        }
    }

    /// The properties of the block it replaces that this one has too, with
    /// values it accepts: a modded log keeps its axis, a slab its half.
    pub fn carry(&mut self, from: &BTreeMap<String, String>) {
        let horizontal = ["north", "south", "east", "west"];
        let accepted: &[(&str, &[&str])] = match self.name.as_str() {
            "minecraft:oak_log" => &[("axis", &["x", "y", "z"])],
            "minecraft:cobblestone_stairs" => {
                &[("facing", &horizontal), ("half", &["top", "bottom"])]
            }
            "minecraft:cobblestone_slab" => &[("type", &["top", "bottom", "double"])],
            "minecraft:oak_fence_gate" => &[("facing", &horizontal)],
            _ => &[],
        };
        for (key, values) in accepted {
            if let Some(v) = from.get(*key)
                && values.contains(&v.as_str())
                && !self.properties.contains_key(*key)
            {
                self.properties.insert((*key).to_owned(), v.clone());
            }
        }
    }

    pub fn text(&self) -> String {
        if self.properties.is_empty() {
            return self.name.clone();
        }
        let props: Vec<String> = self
            .properties
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        format!("{}[{}]", self.name, props.join(","))
    }
}

/// A vanilla block for a modded id, from the words of its path. Every
/// target exists from 1.13 on, so the game's upgrade of older chunks still
/// knows it. `None` when no word says what it is.
pub fn guess(id: &str) -> Option<&'static str> {
    let path = id.split_once(':').map_or(id, |(_, p)| p);
    let words: Vec<&str> = path.split(['_', '/', '.', '-']).collect();
    let has = |set: &[&str]| words.iter().any(|w| set.contains(w));
    if has(&["ore", "ores"]) {
        return Some(if has(&["nether"]) {
            "minecraft:netherrack"
        } else if has(&["end"]) {
            "minecraft:end_stone"
        } else {
            "minecraft:stone"
        });
    }
    if has(&["grass"]) && has(&["block"]) {
        return Some("minecraft:grass_block");
    }
    let metals = [
        "iron",
        "steel",
        "copper",
        "tin",
        "lead",
        "silver",
        "bronze",
        "aluminum",
        "aluminium",
        "nickel",
        "zinc",
        "brass",
        "osmium",
        "uranium",
        "titanium",
        "platinum",
        "electrum",
        "invar",
        "constantan",
    ];
    let rules: &[(&[&str], &str)] = &[
        // Thin, growing, flowing or two-block things: nothing beats a hole
        // where a wrong full block would be.
        (
            &[
                "flower", "flowers", "plant", "plants", "crop", "crops", "sapling", "bush", "vine",
                "vines", "mushroom", "fungus", "seed", "seeds", "roots", "sprout", "reeds",
                "petals", "fern", "grass", "door", "trapdoor", "sign", "button", "lever", "rail",
                "wire", "cable", "pipe", "conduit", "torch", "fluid", "liquid", "molten",
                "flowing", "banner", "bed", "candle", "ladder", "chain", "rope", "pressure",
                "portal",
            ],
            "minecraft:air",
        ),
        (
            &["log", "logs", "stem", "hyphae", "wood"],
            "minecraft:oak_log",
        ),
        (&["planks", "plank"], "minecraft:oak_planks"),
        (&["leaves", "leaf"], "minecraft:oak_leaves[persistent=true]"),
        (&["stairs", "stair"], "minecraft:cobblestone_stairs"),
        (&["slab", "slabs"], "minecraft:cobblestone_slab"),
        (&["wall", "walls"], "minecraft:cobblestone_wall"),
        (&["gate"], "minecraft:oak_fence_gate"),
        (&["fence"], "minecraft:oak_fence"),
        (&["pane", "panes"], "minecraft:glass_pane"),
        (&["glass"], "minecraft:glass"),
        (&["wool"], "minecraft:white_wool"),
        (&["carpet"], "minecraft:white_carpet"),
        (&["concrete"], "minecraft:white_concrete"),
        (&["terracotta"], "minecraft:terracotta"),
        (&["sandstone"], "minecraft:sandstone"),
        (&["sand"], "minecraft:sand"),
        (&["gravel"], "minecraft:gravel"),
        (
            &["dirt", "soil", "mud", "farmland", "loam"],
            "minecraft:dirt",
        ),
        (&["clay"], "minecraft:clay"),
        (&["ice"], "minecraft:packed_ice"),
        (&["snow"], "minecraft:snow_block"),
        (&["netherrack"], "minecraft:netherrack"),
        (
            &["bricks", "brick", "tiles", "tile"],
            "minecraft:stone_bricks",
        ),
        (&["cobblestone", "cobble"], "minecraft:cobblestone"),
        (
            &[
                "stone",
                "rock",
                "basalt",
                "granite",
                "marble",
                "limestone",
                "slate",
                "shale",
                "andesite",
                "diorite",
                "jasper",
                "deepslate",
            ],
            "minecraft:stone",
        ),
        (&["lamp", "lantern"], "minecraft:glowstone"),
    ];
    if let Some((_, target)) = rules.iter().find(|(set, _)| has(set)) {
        return Some(target);
    }
    if has(&metals) && has(&["block"]) {
        return Some("minecraft:iron_block");
    }
    None
}
