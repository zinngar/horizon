use super::*;
use horizon_world::nbt::{self, Limits, NamedTag, Tag};
use std::io::Write;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "horizon-launcher-conversions-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A Fabric jar with one block, under a dashed id.
fn gem_jar(path: &Path) {
    let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
    let files = [
        (
            "fabric.mod.json",
            r#"{"id": "gem-mod", "name": "Gems", "version": "1"}"#,
        ),
        (
            "assets/gem-mod/blockstates/ruby_block.json",
            r#"{"variants": {"": {"model": "gem-mod:block/ruby_block"}}}"#,
        ),
    ];
    for (name, text) in files {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(text.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
}

fn world(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    let root = NamedTag {
        name: "".into(),
        tag: Tag::compound([(
            "Data",
            Tag::compound([
                ("DataVersion", Tag::Int(4189)),
                ("LevelName", Tag::String("Base".into())),
            ]),
        )]),
    };
    let raw = nbt::write_named(&root, Limits::default()).unwrap();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&raw).unwrap();
    fs::write(dir.join("level.dat"), gz.finish().unwrap()).unwrap();
}

#[test]
fn brings_an_instance_mods_and_worlds_without_overwriting() {
    let dir = temp("instance");
    let game = dir.join("prism/instances/Gems/minecraft");
    fs::create_dir_all(game.join("mods")).unwrap();
    gem_jar(&game.join("mods/gems.jar"));
    world(&game.join("saves/Base"));
    fs::create_dir_all(game.join("saves/not-a-world")).unwrap();
    let found = Found {
        launcher: "Prism Launcher",
        name: "Gems".into(),
        minecraft: Some("1.21.1".into()),
        loader: None,
        game,
        shared: false,
    };
    let home = dir.join("home");

    let done = instance(&found, &home, &|_| {}).unwrap();
    assert_eq!(done.mods, ["gem_mod"]);
    assert_eq!(done.worlds, ["Base"]);
    assert!(home
        .join("horizon-mods/gem_mod/data/gem_mod/horizon/block/ruby_block.json")
        .is_file());
    assert!(crate::browser::java::is_enabled(&home, "gem_mod"));
    assert!(home.join("saves/Base/level.dat").is_file());
    assert!(done.work.join("mods/convert-mods-report.md").is_file());

    // Again: the installed mod stays as it is, the world comes as a second one.
    let again = instance(&found, &home, &|_| {}).unwrap();
    assert_eq!(
        (again.mods.len(), again.kept.as_slice()),
        (0, ["gem_mod".to_owned()].as_slice())
    );
    assert_eq!(again.worlds, ["Base (2)"]);
    assert_eq!(again.summary(), "1 world converted, 1 mod already there.");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_world_that_fails_leaves_no_folder() {
    let dir = temp("broken");
    let broken = dir.join("Broken");
    fs::create_dir_all(&broken).unwrap();
    fs::write(broken.join("level.dat"), b"not nbt").unwrap();
    let home = dir.join("home");
    assert!(super::world(&broken, None, &home, &|_| {}).is_err());
    assert_eq!(fs::read_dir(home.join("saves")).unwrap().count(), 0);
    fs::remove_dir_all(dir).unwrap();
}
