//! `convert-world`: a Java world played with mods, copied into a world that
//! opens in Horizon (`docs/world-convert.md`).
//!
//! The mods the world was played with are converted first ([`crate::check`]
//! on each jar), so their blocks, items, biomes and structures stay. Every
//! other modded id is remapped to a vanilla one or removed ([`ids`]), in
//! every chunk, entity, item stack, player and `level.dat` ([`chunk`]). The
//! game's own upgrade (`horizon-world`'s datafix) then takes the copy from
//! its version to 26.3 when it opens, as it does any vanilla world.
//!
//! The world itself is never written: the copy goes to a new folder, with
//! `horizon-convert-report.md` and `remap-suggested.toml` in it.
mod chunk;
pub mod ids;
#[cfg(test)]
mod tests;

pub use chunk::{Ledger, Seen};
pub use ids::{Kind, Provided, Remap};

use crate::pipeline::Log;
use horizon_world::{
    compression::Compression,
    nbt::{self, Limits, NamedTag, Tag},
    region::{ChunkRecord, RegionFile, RegionWriter},
};
use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

/// The first data version with flattened, named block states (1.13).
/// Before it, a modded world stores blocks as numbers only its loader's
/// registry in `level.dat` can name.
const FLATTENING: i32 = 1451;

pub struct Options<'a> {
    pub world: &'a Path,
    pub out: &'a Path,
    /// A folder of the world's mods: jars, `.hmod` files, folder mods.
    pub mods: Option<&'a Path>,
    /// Where converted jars go; default `<out>-horizon-mods` beside `out`.
    pub mods_out: Option<&'a Path>,
    pub remap: Option<&'a Path>,
}

#[derive(Debug, Default)]
pub struct Summary {
    pub ledger: Ledger,
    /// One line per mod: converted, or why not.
    pub mods: Vec<String>,
    /// The folder of converted mods to install, when jars were converted.
    pub mods_folder: Option<PathBuf>,
    pub chunks: u64,
    pub changed: u64,
    /// What was left out of the copy, and why.
    pub skipped: Vec<String>,
}

pub fn convert(o: &Options, log: Log) -> Result<Summary, String> {
    let level_path = o.world.join("level.dat");
    if !level_path.is_file() {
        return Err(format!("{}: no level.dat, not a world", o.world.display()));
    }
    let world = o.world.canonicalize().map_err(|e| e.to_string())?;
    if o.out.exists() {
        let empty = fs::read_dir(o.out).is_ok_and(|mut d| d.next().is_none());
        if !empty {
            return Err(format!("{} exists: give a new folder", o.out.display()));
        }
        if o.out
            .canonicalize()
            .is_ok_and(|out| out.starts_with(&world))
        {
            return Err("the converted world can't go inside the world".into());
        }
    }
    let mut level = read_dat(&level_path)?;
    let data_version = match level.tag.get("Data").and_then(|d| d.get("DataVersion")) {
        Some(Tag::Int(v)) => *v,
        _ => 0,
    };
    let modded_registry = ["FML", "fml", "forge"].iter().any(|k| {
        level.tag.get(k).is_some() || level.tag.get("Data").and_then(|d| d.get(k)).is_some()
    });
    if data_version < FLATTENING && modded_registry {
        return Err(
            "this world is from before Minecraft 1.13 and was played with Forge: its \
                    blocks are numbers only Forge's registry names, which isn't converted yet"
                .into(),
        );
    }

    let mut summary = Summary::default();
    let mut provided = Provided::default();
    if let Some(mods) = o.mods {
        let default_out;
        let mods_out = match o.mods_out {
            Some(m) => m,
            None => {
                let name = o.out.file_name().unwrap_or_default().to_string_lossy();
                default_out = o.out.with_file_name(format!("{name}-horizon-mods"));
                &default_out
            }
        };
        let converted = crate::convert_mods::convert(mods, mods_out, log)?;
        for row in &converted.rows {
            let line = format!("{}: {}. {}", row.file, row.result, row.details);
            summary.mods.push(line.trim_end_matches(". ").to_owned());
        }
        for path in &converted.installed {
            if path.is_dir() {
                provided.add_folder(path)?;
            } else {
                provided.add_zip(path)?;
            }
        }
        provided.aliases = converted.renamed;
        if !converted.installed.is_empty() {
            summary.mods_folder = Some(mods_out.join("horizon-mods"));
        }
    }
    let remap = match o.remap {
        Some(path) => Remap::read(path)?,
        None => Remap::default(),
    };

    let mut ledger = Ledger::new();
    let mut cx = chunk::Ctx {
        provided: &provided,
        remap: &remap,
        ledger: &mut ledger,
        data_version,
        biome: "minecraft:plains",
    };
    let spawn = chunk::level(&mut level.tag, &mut cx);
    fs::create_dir_all(o.out).map_err(|e| format!("{}: {e}", o.out.display()))?;
    write_dat(&o.out.join("level.dat"), &level)?;

    let mut files = Vec::new();
    walk(&world, &mut files).map_err(|e| format!("{}: {e}", world.display()))?;
    files.sort();
    let mut skipped_dims = std::collections::BTreeSet::new();
    for path in files {
        let rel = path.strip_prefix(&world).unwrap_or(&path);
        let parts: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let name = parts.last().cloned().unwrap_or_default();
        if matches!(
            name.as_str(),
            "session.lock" | "level.dat" | "level.dat_old"
        ) {
            continue;
        }
        if let Some(dim) = removed_dimension(&parts, &provided) {
            if skipped_dims.insert(dim.clone()) {
                cx.ledger
                    .entry((Kind::Dimension, dim.clone()))
                    .or_default()
                    .became = "removed".into();
                summary
                    .skipped
                    .push(format!("{dim}: a modded dimension no converted mod adds"));
            }
            continue;
        }
        let dest = o.out.join(rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let parent = parts.len().checked_sub(2).map(|i| parts[i].as_str());
        let in_players = parts.iter().any(|p| p == "playerdata" || p == "players");
        if name.ends_with(".mca") && matches!(parent, Some("region" | "entities" | "poi")) {
            cx.data_version = data_version;
            cx.biome = dimension_biome(&parts);
            let kind = parent.unwrap_or_default();
            if let Err(e) = region(&path, &dest, kind, &mut cx, &mut summary) {
                summary.skipped.push(format!("{}: {e}", rel.display()));
            }
        } else if in_players && name.ends_with(".dat") {
            let mut player = read_dat(&path)?;
            cx.data_version = data_version;
            chunk::player(&mut player.tag, spawn, &mut cx);
            write_dat(&dest, &player)?;
        } else if in_players && name.ends_with(".dat_old") {
            // A backup of the unconverted player: the game would fall back to it.
        } else {
            fs::copy(&path, &dest).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    log(format!(
        "{} chunks read, {} rewritten",
        summary.chunks, summary.changed
    ));
    summary.ledger = ledger;
    fs::write(
        o.out.join("horizon-convert-report.md"),
        report(o, data_version, &summary),
    )
    .map_err(|e| e.to_string())?;
    fs::write(
        o.out.join("remap-suggested.toml"),
        suggested(&summary.ledger),
    )
    .map_err(|e| e.to_string())?;
    Ok(summary)
}

/// The dimension a world file belongs to, when it is a modded one that goes:
/// `dimensions/<ns>/<name>/` (1.16 on), or a `DIM<n>` folder other than the
/// Nether's and the End's (before).
fn removed_dimension(parts: &[String], provided: &Provided) -> Option<String> {
    match parts {
        [first, ns, name, _, ..] if first == "dimensions" => {
            let id = format!("{ns}:{name}");
            (!provided.keeps(Kind::Dimension, &id)).then_some(id)
        }
        [first, _, ..] if first.starts_with("DIM") && first != "DIM-1" && first != "DIM1" => {
            Some(first.clone())
        }
        _ => None,
    }
}

/// What a biome no mod adds becomes, by the dimension its chunk is in.
fn dimension_biome(parts: &[String]) -> &'static str {
    let first = parts.first().map(String::as_str);
    let second = parts.get(2).map(String::as_str);
    match (first, second) {
        (Some("DIM-1"), _) | (Some("dimensions"), Some("the_nether")) => "minecraft:nether_wastes",
        (Some("DIM1"), _) | (Some("dimensions"), Some("the_end")) => "minecraft:the_end",
        _ => "minecraft:plains",
    }
}

/// One region file: each chunk converted, the ones that don't change kept
/// as their bytes, and the ones that don't read copied as they are.
fn region(
    src: &Path,
    dest: &Path,
    kind: &str,
    cx: &mut chunk::Ctx,
    summary: &mut Summary,
) -> Result<(), String> {
    let mut file = RegionFile::open(src).map_err(|e| e.to_string())?;
    let positions: Vec<_> = file.chunks().map(|(p, _)| p).collect();
    let mut writer = RegionWriter::create(dest).map_err(|e| e.to_string())?;
    let version = cx.data_version;
    for pos in positions {
        let record = match file.read(pos) {
            Ok(Some(r)) => r,
            Ok(None) => continue,
            Err(e) => {
                summary
                    .skipped
                    .push(format!("{} chunk {} {}: {e}", src.display(), pos.x, pos.z));
                continue;
            }
        };
        summary.chunks += 1;
        let mut root = match record.nbt(Limits::saved()) {
            Ok(root) => root,
            Err(e) => {
                summary.skipped.push(format!(
                    "{} chunk {} {}: copied unconverted: {e}",
                    src.display(),
                    pos.x,
                    pos.z
                ));
                writer.write(pos, &record).map_err(|e| e.to_string())?;
                continue;
            }
        };
        cx.data_version = version;
        let changed = match kind {
            "entities" => chunk::entity_chunk(&mut root.tag, cx),
            "poi" => chunk::poi_chunk(&mut root.tag, cx),
            _ => chunk::chunk(&mut root.tag, cx),
        };
        if changed {
            summary.changed += 1;
            let new =
                ChunkRecord::from_nbt(&root, record.compression, record.timestamp, Limits::saved())
                    .map_err(|e| e.to_string())?;
            writer.write(pos, &new).map_err(|e| e.to_string())?;
        } else {
            writer.write(pos, &record).map_err(|e| e.to_string())?;
        }
    }
    writer.finish().map_err(|e| e.to_string())
}

fn walk(dir: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            walk(&path, files)?;
        } else {
            files.push(path);
        }
    }
    Ok(())
}

/// A gzipped NBT file (`level.dat`, players).
fn read_dat(path: &Path) -> Result<NamedTag, String> {
    let at = |e: String| format!("{}: {e}", path.display());
    let bytes = fs::read(path).map_err(|e| at(e.to_string()))?;
    let raw = Compression::Gzip
        .decode(&bytes, Limits::saved().max_bytes)
        .map_err(|e| at(e.to_string()))?;
    nbt::read_named(&raw, Limits::saved())
        .map(|(root, _)| root)
        .map_err(|e| at(e.to_string()))
}

fn write_dat(path: &Path, root: &NamedTag) -> Result<(), String> {
    let at = |e: String| format!("{}: {e}", path.display());
    let raw = nbt::write_named(root, Limits::saved()).map_err(|e| at(e.to_string()))?;
    let bytes = Compression::Gzip
        .encode(&raw)
        .map_err(|e| at(e.to_string()))?;
    fs::write(path, bytes).map_err(|e| at(e.to_string()))
}

fn report(o: &Options, data_version: i32, s: &Summary) -> String {
    let mut out = String::new();
    let name = o.world.file_name().unwrap_or_default().to_string_lossy();
    let _ = writeln!(out, "# {name}, converted for Horizon\n");
    let _ = writeln!(
        out,
        "From `{}` (data version {data_version}). The game upgrades it to 26.3 when it opens.\n",
        o.world.display()
    );
    if !s.mods.is_empty() {
        let _ = writeln!(out, "## Mods\n");
        for line in &s.mods {
            let _ = writeln!(out, "- {line}");
        }
        if let Some(folder) = &s.mods_folder {
            let _ = writeln!(
                out,
                "\nInstall the converted mods before you open the world: copy everything in `{}` \
                 into the game folder's `horizon-mods/` (beside `options.txt`), then turn them on in Options > Mods.",
                folder.display()
            );
        }
        out.push('\n');
    }
    let _ = writeln!(out, "## Changes\n");
    let _ = writeln!(out, "{} chunks read, {} rewritten.\n", s.chunks, s.changed);
    if s.ledger.is_empty() {
        let _ = writeln!(out, "No modded ids: nothing to convert.");
    } else {
        let _ = writeln!(out, "| Kind | Id | Became | Seen |\n|---|---|---|---:|");
        for ((kind, id), seen) in &s.ledger {
            let _ = writeln!(
                out,
                "| {} | `{id}` | {} | {} |",
                kind.label(),
                seen.became,
                seen.count
            );
        }
        let _ = writeln!(
            out,
            "\nBlocks and biomes count palette entries (once per chunk section), not blocks. \
             Edit `remap-suggested.toml`, then convert again with `--remap` to choose others."
        );
    }
    if !s.skipped.is_empty() {
        let _ = writeln!(out, "\n## Left out\n");
        for line in &s.skipped {
            let _ = writeln!(out, "- {line}");
        }
    }
    out
}

/// The remap that would give this same result, for the player to edit.
fn suggested(ledger: &Ledger) -> String {
    let mut out = String::from(
        "# What each modded id became. Edit, then convert again with --remap <this file>.\n",
    );
    for (kind, table) in [
        (Kind::Block, "blocks"),
        (Kind::Item, "items"),
        (Kind::Biome, "biomes"),
    ] {
        let rows: Vec<_> = ledger
            .iter()
            .filter(|((k, _), seen)| *k == kind && seen.became != "kept")
            .collect();
        if rows.is_empty() {
            continue;
        }
        let _ = writeln!(out, "\n[{table}]");
        for ((_, id), seen) in rows {
            if seen.became.starts_with("minecraft:paper (") {
                let _ = writeln!(
                    out,
                    "# \"{id}\" = \"minecraft:paper\"  # now named paper holding the item"
                );
            } else {
                let _ = writeln!(out, "\"{id}\" = \"{}\"", seen.became);
            }
        }
    }
    out
}
