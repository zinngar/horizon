use super::{
    Kind, Options, Provided, Remap, chunk, convert,
    ids::{State, guess},
};
use horizon_core::position::ChunkPos;
use horizon_world::{
    compression::Compression,
    nbt::{self, Limits, NamedTag, Tag},
    region::{ChunkRecord, RegionFile, RegionWriter},
};
use std::{fs, path::PathBuf};

fn s(v: &str) -> Tag {
    Tag::String(v.into())
}

fn name(t: &Tag) -> String {
    match t.get("Name").or_else(|| t.get("id")) {
        Some(Tag::String(v)) => v.to_string_lossy(),
        other => panic!("no name in {other:?}"),
    }
}

fn scratch(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "horizon-convert-world-{label}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A 1.21.1 chunk (data version 3955) holding a modded block, biome, block
/// entity and tick, one block a converted mod keeps, and a vanilla chest
/// with a modded item and a vanilla one.
fn modded_chunk() -> Tag {
    Tag::compound([
        ("DataVersion", Tag::Int(3955)),
        ("isLightOn", Tag::Byte(1)),
        (
            "sections",
            Tag::List(vec![Tag::compound([
                (
                    "block_states",
                    Tag::compound([(
                        "palette",
                        Tag::List(vec![
                            Tag::compound([("Name", s("minecraft:stone"))]),
                            Tag::compound([("Name", s("create:zinc_ore"))]),
                            Tag::compound([
                                ("Name", s("bop:fir_log")),
                                ("Properties", Tag::compound([("axis", s("x"))])),
                            ]),
                            Tag::compound([("Name", s("kept:block"))]),
                        ]),
                    )]),
                ),
                (
                    "biomes",
                    Tag::compound([(
                        "palette",
                        Tag::List(vec![s("minecraft:plains"), s("bop:redwood_forest")]),
                    )]),
                ),
            ])]),
        ),
        (
            "block_entities",
            Tag::List(vec![
                Tag::compound([("id", s("create:mechanical_press")), ("x", Tag::Int(1))]),
                Tag::compound([
                    ("id", s("minecraft:chest")),
                    (
                        "Items",
                        Tag::List(vec![
                            Tag::compound([
                                ("id", s("create:wrench")),
                                ("count", Tag::Int(1)),
                                ("Slot", Tag::Byte(0)),
                            ]),
                            Tag::compound([
                                ("id", s("minecraft:diamond")),
                                ("count", Tag::Int(3)),
                                (
                                    "components",
                                    Tag::compound([
                                        ("minecraft:custom_name", s("Gem")),
                                        ("create:clipboard", Tag::Compound(Default::default())),
                                    ]),
                                ),
                            ]),
                        ]),
                    ),
                ]),
            ]),
        ),
        (
            "block_ticks",
            Tag::List(vec![
                Tag::compound([("i", s("create:zinc_ore"))]),
                Tag::compound([("i", s("minecraft:water"))]),
            ]),
        ),
        ("neoforge:attachments", Tag::Compound(Default::default())),
    ])
}

fn kept_block() -> Provided {
    let mut p = Provided::default();
    p.blocks.insert("kept:block".into());
    p.items.insert("kept:block".into());
    p
}

fn run(tag: &mut Tag, provided: &Provided, remap: &Remap) -> (bool, super::Ledger) {
    let mut ledger = super::Ledger::new();
    let mut cx = chunk::Ctx {
        provided,
        remap,
        ledger: &mut ledger,
        data_version: 0,
        biome: "minecraft:plains",
    };
    let changed = chunk::chunk(tag, &mut cx);
    (changed, ledger)
}

#[test]
fn guesses_vanilla_blocks_from_modded_names() {
    assert_eq!(guess("create:zinc_ore"), Some("minecraft:stone"));
    assert_eq!(guess("mod:nether_cobalt_ore"), Some("minecraft:netherrack"));
    assert_eq!(guess("bop:fir_log"), Some("minecraft:oak_log"));
    assert_eq!(guess("bop:pink_daffodil_flower"), Some("minecraft:air"));
    assert_eq!(guess("mod:stone_button"), Some("minecraft:air"));
    assert_eq!(guess("mod:steel_block"), Some("minecraft:iron_block"));
    assert_eq!(guess("mekanism:digital_miner"), None);
}

#[test]
fn chunk_keeps_converted_blocks_and_remaps_the_rest() {
    let mut tag = modded_chunk();
    let (changed, ledger) = run(&mut tag, &kept_block(), &Remap::default());
    assert!(changed);
    let section = &match tag.get("sections") {
        Some(Tag::List(l)) => l,
        _ => panic!(),
    }[0];
    let palette = match section.get("block_states").and_then(|b| b.get("palette")) {
        Some(Tag::List(l)) => l,
        _ => panic!(),
    };
    let names: Vec<String> = palette.iter().map(name).collect();
    assert_eq!(
        names,
        [
            "minecraft:stone",
            "minecraft:stone",
            "minecraft:oak_log",
            "kept:block"
        ]
    );
    // The log keeps its axis.
    assert_eq!(
        palette[2].get("Properties").and_then(|p| p.get("axis")),
        Some(&s("x"))
    );
    let biomes = section.get("biomes").and_then(|b| b.get("palette"));
    assert_eq!(
        biomes,
        Some(&Tag::List(vec![
            s("minecraft:plains"),
            s("minecraft:plains")
        ]))
    );
    let entities = match tag.get("block_entities") {
        Some(Tag::List(l)) => l,
        _ => panic!(),
    };
    assert_eq!(
        entities.iter().map(name).collect::<Vec<_>>(),
        ["minecraft:chest"]
    );
    let ticks = match tag.get("block_ticks") {
        Some(Tag::List(l)) => l.len(),
        _ => panic!(),
    };
    assert_eq!(ticks, 1);
    assert_eq!(tag.get("isLightOn"), Some(&Tag::Byte(0)));
    assert!(tag.get("neoforge:attachments").is_none());
    assert_eq!(ledger[&(Kind::Block, "kept:block".into())].became, "kept");
    assert_eq!(
        ledger[&(Kind::BlockEntity, "create:mechanical_press".into())].became,
        "removed"
    );
}

#[test]
fn unkept_items_become_named_paper_holding_them() {
    let mut tag = modded_chunk();
    run(&mut tag, &kept_block(), &Remap::default());
    let chest = match tag.get("block_entities") {
        Some(Tag::List(l)) => l[0].clone(),
        _ => panic!(),
    };
    let Some(Tag::List(items)) = chest.get("Items") else {
        panic!()
    };
    let paper = &items[0];
    assert_eq!(name(paper), "minecraft:paper");
    assert_eq!(paper.get("count"), Some(&Tag::Int(1)));
    let components = paper.get("components").unwrap();
    let Some(Tag::String(custom_name)) = components.get("minecraft:custom_name") else {
        panic!()
    };
    let custom_name: serde_json::Value =
        serde_json::from_str(&custom_name.to_string_lossy()).unwrap();
    assert_eq!(custom_name["text"], "Missing: create:wrench");
    let saved = components
        .get("minecraft:custom_data")
        .and_then(|d| d.get("horizon_missing"))
        .unwrap();
    assert_eq!(name(saved), "create:wrench");
    // The diamond stays, without the component of a mod that goes.
    let diamond = items[1].get("components").unwrap();
    assert_eq!(diamond.get("minecraft:custom_name"), Some(&s("Gem")));
    assert!(diamond.get("create:clipboard").is_none());
}

#[test]
fn converting_twice_changes_nothing_more() {
    let mut tag = modded_chunk();
    run(&mut tag, &kept_block(), &Remap::default());
    let once = tag.clone();
    let (changed, _) = run(&mut tag, &kept_block(), &Remap::default());
    assert!(!changed);
    assert_eq!(tag, once);
}

#[test]
fn remap_file_overrides_the_guess() {
    let dir = scratch("remap");
    let path = dir.join("remap.toml");
    fs::write(
        &path,
        "[blocks]\n\"create:zinc_ore\" = \"minecraft:iron_ore\"\n[items]\n\"create:wrench\" = \"minecraft:stick\"\n",
    )
    .unwrap();
    let remap = Remap::read(&path).unwrap();
    assert_eq!(
        remap.block("create:zinc_ore"),
        State::parse("minecraft:iron_ore")
    );
    assert_eq!(
        remap.item("create:wrench").as_deref(),
        Some("minecraft:stick")
    );
    fs::write(&path, "[blocks]\n\"a:b\" = \"create:c\"\n").unwrap();
    assert!(Remap::read(&path).is_err());
    fs::remove_dir_all(dir).unwrap();
}

fn write_dat(path: &std::path::Path, tag: Tag) {
    let raw = nbt::write_named(
        &NamedTag {
            name: "".into(),
            tag,
        },
        Limits::saved(),
    )
    .unwrap();
    fs::write(path, Compression::Gzip.encode(&raw).unwrap()).unwrap();
}

fn write_region(path: &std::path::Path, tag: Tag) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut writer = RegionWriter::create(path).unwrap();
    let root = NamedTag {
        name: "".into(),
        tag,
    };
    let record = ChunkRecord::from_nbt(&root, Compression::Zlib, 7, Limits::saved()).unwrap();
    writer.write(ChunkPos::new(0, 0), &record).unwrap();
    writer.finish().unwrap();
}

#[test]
fn converts_a_world_folder_with_a_folder_mod() {
    let dir = scratch("world");
    let world = dir.join("Modded");
    fs::create_dir_all(&world).unwrap();
    write_dat(
        &world.join("level.dat"),
        Tag::compound([
            (
                "Data",
                Tag::compound([
                    ("DataVersion", Tag::Int(3955)),
                    ("SpawnX", Tag::Int(10)),
                    ("SpawnY", Tag::Int(70)),
                    ("SpawnZ", Tag::Int(-4)),
                    (
                        "DataPacks",
                        Tag::compound([(
                            "Enabled",
                            Tag::List(vec![s("vanilla"), s("mod:create"), s("file/mine.zip")]),
                        )]),
                    ),
                ]),
            ),
            ("fml", Tag::Compound(Default::default())),
        ]),
    );
    write_region(&world.join("region/r.0.0.mca"), modded_chunk());
    write_region(
        &world.join("dimensions/aether/the_aether/region/r.0.0.mca"),
        modded_chunk(),
    );
    fs::create_dir_all(world.join("playerdata")).unwrap();
    write_dat(
        &world.join("playerdata/p.dat"),
        Tag::compound([
            ("Dimension", s("aether:the_aether")),
            (
                "Inventory",
                Tag::List(vec![Tag::compound([
                    ("id", s("aether:zanite_gemstone")),
                    ("count", Tag::Int(5)),
                ])]),
            ),
        ]),
    );
    let mods = dir.join("mods");
    let kept = mods.join("kept");
    fs::create_dir_all(kept.join("data/kept/horizon/block")).unwrap();
    fs::write(
        kept.join("horizon-mod.toml"),
        "id = \"kept\"\nname = \"Kept\"\nversion = \"1.0.0\"\napi = \"0.2\"\nside = \"both\"\ncapabilities = [\"registries\"]\n",
    )
    .unwrap();
    fs::write(
        kept.join("data/kept/horizon/block/block.json"),
        r#"{ "template": "minecraft:stone" }"#,
    )
    .unwrap();

    let out = dir.join("Modded (Horizon)");
    let options = Options {
        world: &world,
        out: &out,
        mods: Some(&mods),
        mods_out: None,
        remap: None,
    };
    let summary = convert(&options, &|_| {}).unwrap();
    assert_eq!((summary.chunks, summary.changed), (1, 1));
    assert_eq!(
        summary.ledger[&(Kind::Block, "kept:block".into())].became,
        "kept"
    );
    assert_eq!(
        summary.ledger[&(Kind::Dimension, "aether:the_aether".into())].became,
        "removed"
    );
    assert!(!out.join("dimensions").exists());

    let mut region = RegionFile::open(out.join("region/r.0.0.mca")).unwrap();
    let record = region.read(ChunkPos::new(0, 0)).unwrap().unwrap();
    assert_eq!(record.timestamp, 7);
    let tag = record.nbt(Limits::saved()).unwrap().tag;
    let Some(Tag::List(entities)) = tag.get("block_entities") else {
        panic!()
    };
    assert_eq!(entities.len(), 1);

    let read = |p: PathBuf| {
        let raw = Compression::Gzip
            .decode(&fs::read(p).unwrap(), 1 << 20)
            .unwrap();
        nbt::read_named(&raw, Limits::saved()).unwrap().0.tag
    };
    let level = read(out.join("level.dat"));
    assert!(level.get("fml").is_none());
    let enabled = level
        .get("Data")
        .and_then(|d| d.get("DataPacks"))
        .and_then(|p| p.get("Enabled"));
    assert_eq!(
        enabled,
        Some(&Tag::List(vec![s("vanilla"), s("file/mine.zip")]))
    );
    let player = read(out.join("playerdata/p.dat"));
    assert_eq!(player.get("Dimension"), Some(&s("minecraft:overworld")));
    assert_eq!(
        player.get("Pos"),
        Some(&Tag::List(
            [10.5, 70.0, -3.5]
                .map(|v: f64| Tag::Double(v.to_bits()))
                .to_vec()
        ))
    );
    let Some(Tag::List(inventory)) = player.get("Inventory") else {
        panic!()
    };
    assert_eq!(name(&inventory[0]), "minecraft:paper");
    assert!(out.join("horizon-convert-report.md").is_file());
    let suggested = fs::read_to_string(out.join("remap-suggested.toml")).unwrap();
    assert!(toml::from_str::<Remap>(&suggested).is_ok());

    // The copy is never written over.
    assert!(convert(&options, &|_| {}).is_err());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn refuses_forge_worlds_from_before_the_flattening() {
    let dir = scratch("legacy");
    let world = dir.join("Old");
    fs::create_dir_all(&world).unwrap();
    write_dat(
        &world.join("level.dat"),
        Tag::compound([
            ("Data", Tag::compound([("DataVersion", Tag::Int(1343))])),
            ("FML", Tag::Compound(Default::default())),
        ]),
    );
    let options = Options {
        world: &world,
        out: &dir.join("out"),
        mods: None,
        mods_out: None,
        remap: None,
    };
    let e = convert(&options, &|_| {}).unwrap_err();
    assert!(e.contains("before Minecraft 1.13"), "{e}");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn ids_of_a_renamed_mod_take_its_new_namespace() {
    let mut provided = Provided::default();
    provided.aliases.insert("gem-mod".into(), "gem_mod".into());
    provided.blocks.insert("gem_mod:ruby_block".into());
    provided.items.insert("gem_mod:ruby".into());
    let mut tag = Tag::compound([
        (
            "sections",
            Tag::List(vec![Tag::compound([(
                "block_states",
                Tag::compound([(
                    "palette",
                    Tag::List(vec![Tag::compound([("Name", s("gem-mod:ruby_block"))])]),
                )]),
            )])]),
        ),
        (
            "block_entities",
            Tag::List(vec![Tag::compound([
                ("id", s("minecraft:chest")),
                (
                    "Items",
                    Tag::List(vec![Tag::compound([
                        ("id", s("gem-mod:ruby")),
                        ("count", Tag::Int(2)),
                    ])]),
                ),
            ])]),
        ),
    ]);
    let (changed, ledger) = run(&mut tag, &provided, &Remap::default());
    assert!(changed);
    let Some(Tag::List(sections)) = tag.get("sections") else {
        panic!()
    };
    let Some(Tag::List(palette)) = sections[0]
        .get("block_states")
        .and_then(|b| b.get("palette"))
    else {
        panic!()
    };
    assert_eq!(name(&palette[0]), "gem_mod:ruby_block");
    let Some(Tag::List(entities)) = tag.get("block_entities") else {
        panic!()
    };
    let Some(Tag::List(items)) = entities[0].get("Items") else {
        panic!()
    };
    assert_eq!(name(&items[0]), "gem_mod:ruby");
    assert_eq!(
        ledger[&(Kind::Block, "gem_mod:ruby_block".into())].became,
        "kept"
    );
}
