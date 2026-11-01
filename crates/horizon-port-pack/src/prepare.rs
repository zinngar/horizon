//! Before a jar is checked and carried over: a copy of it the carry can
//! read, when the jar itself can't be. The jar is never changed.
//!
//! - **A mod id with dashes** (`cloth-config`, Fabric allows them): Horizon's
//!   mod ids are `[a-z0-9_]`, and blocks and items are declared under the
//!   mod id, so the namespace is renamed with it (`cloth_config`): paths,
//!   every `cloth-config:` id the files name, and translation keys.
//!   `convert-world` renames the ids in a world the same way.
//! - **Made for 1.20 or older** (`data/<ns>/recipes/`, `loot_tables/`): the
//!   data folders get 1.21's names, and recipes, loot tables and
//!   advancements 1.21.1's shapes, which the carry upgrades from.
use crate::jar;
use serde_json::{Map, Value, json};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub struct Prepared {
    /// The jar to check: the original, or the rewritten copy.
    pub jar: PathBuf,
    /// The dashed id and the one it became.
    pub renamed: Option<(String, String)>,
    /// Whether its data was upgraded from 1.20 or older.
    pub upgraded: bool,
}

/// The id a dashed mod id becomes, when that makes it a valid one.
pub fn renamed_id(id: &str) -> Option<String> {
    let new = id.replace('-', "_");
    let valid = !new.is_empty()
        && new.len() <= 64
        && new
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    (new != id && valid).then_some(new)
}

/// Data folders named as before 1.21 (`recipes/`, `loot_tables/`) and none
/// as 1.21 names them. (Some jars keep a stale plural folder beside the
/// new one: those are 1.21 jars.)
pub fn old_layout(names: &[String]) -> bool {
    let has = |kinds: [&str; 2]| {
        names.iter().any(|n| {
            let p: Vec<&str> = n.split('/').collect();
            p.len() > 3 && p[0] == "data" && kinds.contains(&p[2]) && n.ends_with(".json")
        })
    };
    has(["recipes", "loot_tables"]) && !has(["recipe", "loot_table"])
}

/// 1.21's data folder names (`data/<ns>/<old>/` to `data/<ns>/<new>/`).
const FOLDERS: [(&str, &str); 13] = [
    ("recipes/", "recipe/"),
    ("loot_tables/", "loot_table/"),
    ("advancements/", "advancement/"),
    ("predicates/", "predicate/"),
    ("item_modifiers/", "item_modifier/"),
    ("functions/", "function/"),
    ("structures/", "structure/"),
    ("tags/items/", "tags/item/"),
    ("tags/blocks/", "tags/block/"),
    ("tags/entity_types/", "tags/entity_type/"),
    ("tags/fluids/", "tags/fluid/"),
    ("tags/game_events/", "tags/game_event/"),
    ("tags/functions/", "tags/function/"),
];

/// Vanilla ids renamed between 1.20 and 1.21.1 (1.20.3's short grass,
/// 1.20.5's turtle scute).
const RENAMED_1_21: [(&str, &str); 2] = [
    ("minecraft:grass", "minecraft:short_grass"),
    ("minecraft:scute", "minecraft:turtle_scute"),
];

/// `jar` as it is, or a rewritten copy in `work/prepared.jar`.
pub fn prepare(jar_path: &Path, work: &Path) -> Result<Prepared, String> {
    let at = |e: String| format!("{}: {e}", jar_path.display());
    let info = jar::read(jar_path)?;
    let file = fs::File::open(jar_path).map_err(|e| at(e.to_string()))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| at(e.to_string()))?;
    let names: Vec<String> = zip
        .file_names()
        .map(|name| name.map(|name| name.into_owned()))
        .collect::<Result<_, _>>()
        .map_err(|e| at(e.to_string()))?;
    let renamed = renamed_id(&info.mod_id).map(|new| (info.mod_id.clone(), new));
    let upgraded = old_layout(&names);
    if renamed.is_none() && !upgraded {
        return Ok(Prepared {
            jar: jar_path.to_owned(),
            renamed,
            upgraded,
        });
    }
    fs::create_dir_all(work).map_err(|e| format!("{}: {e}", work.display()))?;
    let dest = work.join("prepared.jar");
    let out = fs::File::create(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let mut writer = zip::ZipWriter::new(out);
    let options = zip::write::SimpleFileOptions::default();
    let mut written = std::collections::BTreeSet::new();
    for name in &names {
        if name.ends_with('/') {
            continue;
        }
        let mut bytes = Vec::new();
        zip.by_name(name)
            .map_err(|e| at(e.to_string()))?
            .read_to_end(&mut bytes)
            .map_err(|e| at(e.to_string()))?;
        let mut new_name = name.clone();
        if let Some((old, new)) = &renamed {
            new_name = rename_path(&new_name, old, new);
            bytes = rename_refs(&new_name, bytes, old, new);
        }
        if upgraded {
            new_name = upgrade_path(&new_name);
            bytes = upgrade_file(&new_name, bytes);
        }
        // A stale new-style file beside the old one: the first kept.
        if !written.insert(new_name.clone()) {
            continue;
        }
        writer
            .start_file(new_name.as_str(), options)
            .and_then(|()| writer.write_all(&bytes).map_err(Into::into))
            .map_err(|e| format!("{}: {e}", dest.display()))?;
    }
    writer
        .finish()
        .map_err(|e| format!("{}: {e}", dest.display()))?;
    Ok(Prepared {
        jar: dest,
        renamed,
        upgraded,
    })
}

/// `assets/old/...` and `data/old/...` under the new namespace.
fn rename_path(name: &str, old: &str, new: &str) -> String {
    for root in ["assets/", "data/"] {
        if let Some(rest) = name.strip_prefix(root)
            && let Some(rest) = rest.strip_prefix(old)
            && rest.starts_with('/')
        {
            return format!("{root}{new}{rest}");
        }
    }
    name.to_owned()
}

/// The ids and translation keys of a text file in the new namespace.
fn rename_refs(name: &str, bytes: Vec<u8>, old: &str, new: &str) -> Vec<u8> {
    if name == "fabric.mod.json" {
        return match serde_json::from_slice::<Value>(&bytes) {
            Ok(mut v) => {
                v["id"] = json!(new);
                serde_json::to_vec_pretty(&v).unwrap_or(bytes)
            }
            Err(_) => bytes,
        };
    }
    let text_file = [".json", ".mcmeta", ".mcfunction"]
        .iter()
        .any(|ext| name.ends_with(ext));
    let Some(text) = String::from_utf8(bytes.clone()).ok().filter(|_| text_file) else {
        return bytes;
    };
    let mut text = text
        .replace(&format!("\"{old}:"), &format!("\"{new}:"))
        .replace(&format!("#{old}:"), &format!("#{new}:"));
    if name.contains("/lang/") {
        text = text.replace(&format!(".{old}."), &format!(".{new}."));
    }
    text.into_bytes()
}

fn upgrade_path(name: &str) -> String {
    let parts: Vec<&str> = name.splitn(3, '/').collect();
    if let [root @ "data", ns, rest] = parts[..] {
        for (old, new) in FOLDERS {
            if let Some(tail) = rest.strip_prefix(old) {
                return format!("{root}/{ns}/{new}{tail}");
            }
        }
    }
    name.to_owned()
}

/// A data file in 1.21.1's shape, by its (1.21) folder.
fn upgrade_file(name: &str, bytes: Vec<u8>) -> Vec<u8> {
    let parts: Vec<&str> = name.splitn(4, '/').collect();
    let (["data", _, kind, _], true) = (&parts[..], name.ends_with(".json")) else {
        return bytes;
    };
    let Ok(mut v) = serde_json::from_slice::<Value>(&bytes) else {
        return bytes;
    };
    match *kind {
        "recipe" => upgrade_recipe(&mut v),
        "loot_table" | "predicate" | "item_modifier" => upgrade_loot(&mut v),
        "advancement" => upgrade_advancement(&mut v),
        _ => {}
    }
    renamed_ids(&mut v);
    serde_json::to_vec_pretty(&v).unwrap_or(bytes)
}

fn renamed_ids(v: &mut Value) {
    match v {
        Value::String(s) => {
            let tagless = s.trim_start_matches('#');
            if let Some((_, new)) = RENAMED_1_21.iter().find(|(old, _)| *old == tagless) {
                *s = (*new).to_owned();
            }
        }
        Value::Array(a) => a.iter_mut().for_each(renamed_ids),
        Value::Object(o) => o.values_mut().for_each(renamed_ids),
        _ => {}
    }
}

/// 1.20.5's item stack: `{"id", "count"}` for `{"item", "count", "nbt"}`
/// or a bare id. NBT has no sure translation to components, so it goes.
fn stack(v: &Value, count: Option<Value>) -> Value {
    let mut out = Map::new();
    match v {
        Value::String(id) => {
            out.insert("id".into(), json!(id));
        }
        Value::Object(o) => {
            for (k, x) in o {
                match k.as_str() {
                    "item" => {
                        out.insert("id".into(), x.clone());
                    }
                    "nbt" => {}
                    _ => {
                        out.insert(k.clone(), x.clone());
                    }
                }
            }
        }
        _ => return v.clone(),
    }
    if let Some(count) = count
        && !out.contains_key("count")
    {
        out.insert("count".into(), count);
    }
    Value::Object(out)
}

fn upgrade_recipe(v: &mut Value) {
    let Some(o) = v.as_object_mut() else {
        return;
    };
    // Stonecutting kept its count beside the result.
    let count = if o.get("result").is_some_and(Value::is_string) {
        o.remove("count")
    } else {
        None
    };
    if let Some(result) = o.get("result").cloned() {
        o.insert("result".into(), stack(&result, count));
    }
}

fn upgrade_advancement(v: &mut Value) {
    if let Some(icon) = v.pointer_mut("/display/icon") {
        *icon = stack(icon, None);
    }
}

/// Loot functions and conditions as 1.21.1 wrote them: NBT functions name
/// custom data, and a tool match's item predicate has 1.20.5's shape.
fn upgrade_loot(v: &mut Value) {
    match v {
        Value::Array(a) => a.iter_mut().for_each(upgrade_loot),
        Value::Object(o) => {
            if let Some(Value::String(f)) = o.get_mut("function") {
                match f.trim_start_matches("minecraft:") {
                    "set_nbt" => *f = "minecraft:set_custom_data".into(),
                    "copy_nbt" => *f = "minecraft:copy_custom_data".into(),
                    _ => {}
                }
            }
            let tool = o
                .get("condition")
                .and_then(Value::as_str)
                .is_some_and(|c| c.trim_start_matches("minecraft:") == "match_tool");
            if tool && let Some(Value::Object(p)) = o.get_mut("predicate") {
                item_predicate(p);
            }
            o.values_mut().for_each(upgrade_loot);
        }
        _ => {}
    }
}

/// An item predicate in 1.20.5's shape: `items` for `item` and `tag`, and
/// enchantments under `predicates`.
fn item_predicate(p: &mut Map<String, Value>) {
    if let Some(tag) = p.remove("tag").and_then(|t| t.as_str().map(str::to_owned)) {
        p.insert("items".into(), json!(format!("#{tag}")));
    }
    if let Some(item) = p.remove("item") {
        p.insert("items".into(), json!([item]));
    }
    if let Some(Value::Array(list)) = p.remove("enchantments") {
        let list: Vec<Value> = list
            .into_iter()
            .map(|mut e| {
                if let Some(o) = e.as_object_mut()
                    && let Some(id) = o.remove("enchantment")
                {
                    o.insert("enchantments".into(), id);
                }
                e
            })
            .collect();
        let predicates = p
            .entry("predicates")
            .or_insert_with(|| json!({}))
            .as_object_mut();
        if let Some(predicates) = predicates {
            predicates.insert("minecraft:enchantments".into(), Value::Array(list));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("port-pack-prepare-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn jar(dir: &Path, files: &[(&str, &str)]) -> PathBuf {
        let path = dir.join("m.jar");
        let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
        for (name, text) in files {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(text.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        path
    }

    fn read(path: &Path, name: &str) -> Option<String> {
        let mut zip = zip::ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
        let mut text = String::new();
        zip.by_name(name).ok()?.read_to_string(&mut text).unwrap();
        Some(text)
    }

    #[test]
    fn a_dashed_mod_id_renames_its_namespace() {
        let dir = temp("dashed");
        let src = jar(
            &dir,
            &[
                (
                    "fabric.mod.json",
                    r#"{"id": "gem-craft", "name": "Gem Craft", "version": "1"}"#,
                ),
                (
                    "assets/gem-craft/blockstates/ruby_block.json",
                    r#"{"variants": {"": {"model": "gem-craft:block/ruby_block"}}}"#,
                ),
                (
                    "assets/gem-craft/lang/en_us.json",
                    r#"{"block.gem-craft.ruby_block": "Ruby"}"#,
                ),
                (
                    "data/minecraft/tags/block/mineable/pickaxe.json",
                    r##"{"values": ["gem-craft:ruby_block", "#gem-craft:gems"]}"##,
                ),
            ],
        );
        let p = prepare(&src, &dir.join("work")).unwrap();
        assert_eq!(p.renamed, Some(("gem-craft".into(), "gem_craft".into())));
        let blockstate = read(&p.jar, "assets/gem_craft/blockstates/ruby_block.json").unwrap();
        assert!(
            blockstate.contains("\"gem_craft:block/ruby_block\""),
            "{blockstate}"
        );
        let lang = read(&p.jar, "assets/gem_craft/lang/en_us.json").unwrap();
        assert!(lang.contains("block.gem_craft.ruby_block"), "{lang}");
        let tag = read(&p.jar, "data/minecraft/tags/block/mineable/pickaxe.json").unwrap();
        assert!(
            tag.contains("\"gem_craft:ruby_block\"") && tag.contains("\"#gem_craft:gems\""),
            "{tag}"
        );
        assert_eq!(jar::read(&p.jar).unwrap().mod_id, "gem_craft");
        // The original is untouched.
        assert!(read(&src, "assets/gem-craft/blockstates/ruby_block.json").is_some());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_old_mod_gets_1_21_folders_and_shapes() {
        let dir = temp("old");
        let src = jar(
            &dir,
            &[
                (
                    "data/ruby/recipes/ruby_block.json",
                    r#"{"type": "minecraft:crafting_shapeless", "ingredients": [{"item": "ruby:ruby"}], "result": {"item": "ruby:ruby_block", "count": 1}}"#,
                ),
                (
                    "data/ruby/recipes/cut.json",
                    r#"{"type": "minecraft:stonecutting", "ingredient": {"item": "ruby:ruby_block"}, "result": "ruby:ruby_slab", "count": 2}"#,
                ),
                (
                    "data/ruby/loot_tables/blocks/ruby_ore.json",
                    r#"{"pools": [{"rolls": 1, "entries": [{"type": "minecraft:item", "name": "ruby:ruby_ore", "conditions": [{"condition": "minecraft:match_tool", "predicate": {"enchantments": [{"enchantment": "minecraft:silk_touch", "levels": {"min": 1}}]}}]}, {"type": "minecraft:item", "name": "minecraft:grass"}]}]}"#,
                ),
                (
                    "data/ruby/tags/items/gems.json",
                    r#"{"values": ["ruby:ruby"]}"#,
                ),
            ],
        );
        let p = prepare(&src, &dir.join("work")).unwrap();
        assert!(p.upgraded && p.renamed.is_none());
        let json = |name: &str| -> Value {
            serde_json::from_str(&read(&p.jar, name).unwrap_or_else(|| panic!("{name}"))).unwrap()
        };
        let recipe = json("data/ruby/recipe/ruby_block.json");
        assert_eq!(
            recipe["result"],
            json!({"id": "ruby:ruby_block", "count": 1})
        );
        let cut = json("data/ruby/recipe/cut.json");
        assert_eq!(cut["result"], json!({"id": "ruby:ruby_slab", "count": 2}));
        assert!(cut.get("count").is_none());
        let loot = json("data/ruby/loot_table/blocks/ruby_ore.json");
        let entries = &loot["pools"][0]["entries"];
        assert_eq!(
            entries[0]["conditions"][0]["predicate"],
            json!({"predicates": {"minecraft:enchantments": [{"enchantments": "minecraft:silk_touch", "levels": {"min": 1}}]}})
        );
        assert_eq!(entries[1]["name"], "minecraft:short_grass");
        assert!(read(&p.jar, "data/ruby/tags/item/gems.json").is_some());
        assert!(read(&p.jar, "data/ruby/recipes/cut.json").is_none());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_current_jar_is_used_as_it_is() {
        let dir = temp("current");
        let src = jar(&dir, &[("data/ruby/recipe/a.json", "{}")]);
        let p = prepare(&src, &dir.join("work")).unwrap();
        assert_eq!(p.jar, src);
        assert!(!dir.join("work").exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
