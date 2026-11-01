//! `convert-mods`: a folder of Java mods (a launcher's `mods/`) as Horizon
//! mods, ready to install (`docs/mod-convert.md`).
//!
//! Each jar is checked and carried over as the mod browser does it
//! ([`crate::check`], dashed ids and 1.20 jars included through
//! [`crate::prepare`]); the ones with content go into `<out>/horizon-mods/` as
//! folder mods, the first of two that claim one namespace kept, as the game
//! keeps it. Horizon mods already in the folder (`.hmod`, folder mods) go
//! along. `<out>/work/` keeps each jar's check and carry, and
//! `convert-mods-report.md` says what became of every file.
use crate::{check::Verdict, classify::Known, pipeline::Log};
use std::{
    collections::BTreeMap,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

/// What became of one file of the mods folder.
#[derive(Debug, Clone)]
pub struct Row {
    pub file: String,
    /// `Works`, `Partly`, `Doesn't run`, `Horizon mod`, `Not converted`.
    pub result: String,
    pub details: String,
    /// The Horizon mod id it was installed as.
    pub installed: Option<String>,
}

#[derive(Debug, Default)]
pub struct Converted {
    pub rows: Vec<Row>,
    /// `<out>/horizon-mods/<id>` folders and copied `.hmod` files, ready to install.
    pub installed: Vec<PathBuf>,
    /// Dashed mod ids and what they became: worlds name their ids the old way.
    pub renamed: BTreeMap<String, String>,
    /// `<out>/horizon-mods/`: what to copy into the game folder's.
    pub folder: PathBuf,
}

pub fn convert(dir: &Path, out: &Path, log: Log) -> Result<Converted, String> {
    let at = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
    if fs::read_dir(out).is_ok_and(|mut d| d.next().is_some()) {
        return Err(format!("{} exists: give a new folder", out.display()));
    }
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| at(dir, e))?
        .flatten()
        .map(|e| e.path())
        .collect();
    entries.sort();
    let (work, mods) = (out.join("work"), out.join("horizon-mods"));
    fs::create_dir_all(&mods).map_err(|e| at(&mods, e))?;
    let known = Known::atm10();
    let manifest = horizon_mods::manifest::MANIFEST;
    let mut converted = Converted {
        folder: mods.clone(),
        ..Converted::default()
    };
    // (row index, folder mod) in the order the game would meet them.
    let mut folders: Vec<(usize, PathBuf)> = Vec::new();
    for path in entries {
        let file = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let mut row = Row {
            file: file.clone(),
            result: String::new(),
            details: String::new(),
            installed: None,
        };
        if file.ends_with(".jar") {
            let dest = work.join(path.file_stem().unwrap_or_default());
            match crate::check::check(&path, &dest) {
                Ok(c) => {
                    row.result = c.verdict.label().into();
                    row.details = if c.verdict == Verdict::No {
                        why_not(&known, &c)
                    } else {
                        c.summary.clone()
                    };
                    if let Some(old) = &c.renamed_from {
                        converted.renamed.insert(old.clone(), c.mod_id.clone());
                    }
                    let carry = dest.join("carry");
                    if c.verdict != Verdict::No && carry.join(manifest).is_file() {
                        folders.push((converted.rows.len(), carry));
                    }
                }
                Err(e) => {
                    row.result = "Not converted".into();
                    row.details = e;
                }
            }
            log(format!("{file}: {}", row.result));
        } else if path.join(manifest).is_file() {
            row.result = "Horizon mod".into();
            folders.push((converted.rows.len(), path.clone()));
        } else if file.ends_with(".hmod") {
            row.result = "Horizon mod".into();
            let dest = mods.join(&file);
            fs::copy(&path, &dest).map_err(|e| at(&path, e))?;
            row.installed = Some(file.trim_end_matches(".hmod").to_owned());
            converted.installed.push(dest);
        } else {
            continue;
        }
        converted.rows.push(row);
    }
    let paths: Vec<PathBuf> = folders.iter().map(|(_, p)| p.clone()).collect();
    let (on, off) = crate::pipeline::loadable(&paths);
    for (i, folder) in folders {
        let row = &mut converted.rows[i];
        let text = fs::read_to_string(folder.join(manifest)).map_err(|e| at(&folder, e))?;
        let id = horizon_mods::Manifest::parse(&text)?.id;
        if !on.contains(&folder) {
            row.details = off
                .iter()
                .find(|line| line.starts_with(&format!("{id} not loaded")))
                .cloned()
                .unwrap_or_else(|| format!("{id}: left off"));
            row.result = "Not converted".into();
            continue;
        }
        let dest = mods.join(&id);
        copy_dir(&folder, &dest).map_err(|e| at(&folder, e))?;
        row.installed = Some(id);
        converted.installed.push(dest);
    }
    fs::write(
        out.join("convert-mods-report.md"),
        report(dir, out, &converted),
    )
    .map_err(|e| at(out, e))?;
    Ok(converted)
}

/// Why a jar doesn't run, in words for the player: what kind of mod it is
/// when the reviewed table knows it, else the check's own reason.
fn why_not(known: &Known, c: &crate::check::Check) -> String {
    let class = known
        .get(&[c.renamed_from.as_deref().unwrap_or(&c.mod_id), &c.name])
        .map(|(class, _)| class.as_str());
    match class {
        Some("library") => "A library other mods use: nothing to install without them.".into(),
        Some("performance") => "A performance mod: Horizon is its own.".into(),
        Some("client-ui") => {
            "Screens, HUD or maps written in Java: it needs a rewrite as a native mod.".into()
        }
        _ => c.summary.clone(),
    }
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let dest = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &dest)?;
        } else {
            fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}

fn report(dir: &Path, out: &Path, c: &Converted) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "# Mods converted for Horizon\n");
    let _ = writeln!(
        s,
        "From `{}`: {} of {} installable. Copy everything in `{}` into the game folder's \
         `horizon-mods/` (beside `options.txt`), then turn them on in Options > Mods.\n",
        dir.display(),
        c.installed.len(),
        c.rows.len(),
        out.join("horizon-mods").display()
    );
    let _ = writeln!(
        s,
        "| File | Result | Installed as | Details |\n|---|---|---|---|"
    );
    for r in &c.rows {
        let details = r.details.replace('|', "\\|");
        let id = r.installed.as_deref().unwrap_or("");
        let _ = writeln!(s, "| `{}` | {} | {id} | {details} |", r.file, r.result);
    }
    let _ = writeln!(
        s,
        "\nJava code doesn't run in Horizon: what a mod does in code (machines, entities, \
         screens, Mixins) is left out. Each jar's check and carry are in `{}`.",
        out.join("work").display()
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn jar(path: &Path, files: &[(&str, &str)]) {
        let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
        for (name, text) in files {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(text.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    fn fabric(id: &str) -> String {
        format!(r#"{{"id": "{id}", "name": "{id}", "version": "1"}}"#)
    }

    fn block(ns: &str) -> String {
        format!(r#"{{"variants": {{"": {{"model": "{ns}:block/gem"}}}}}}"#)
    }

    #[test]
    fn converts_a_mods_folder_into_installable_mods() {
        let dir =
            std::env::temp_dir().join(format!("port-pack-convert-mods-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let src = dir.join("mods");
        fs::create_dir_all(&src).unwrap();
        jar(
            &src.join("a-gems.jar"),
            &[
                ("fabric.mod.json", &fabric("gem-mod")),
                ("assets/gem-mod/blockstates/gem.json", &block("gem-mod")),
            ],
        );
        let class = crate::classes::tests_support::class_file("x/A", "java/lang/Object");
        let mut zip = zip::ZipWriter::new(fs::File::create(src.join("b-code.jar")).unwrap());
        zip.start_file("fabric.mod.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(fabric("codeonly").as_bytes()).unwrap();
        zip.start_file("x/A.class", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&class).unwrap();
        zip.finish().unwrap();
        // A second mod claiming the same namespace: left off, as the game would.
        jar(
            &src.join("c-copy.jar"),
            &[
                ("fabric.mod.json", &fabric("gem_mod")),
                ("assets/gem_mod/blockstates/gem.json", &block("gem_mod")),
            ],
        );
        fs::write(src.join("notes.txt"), "not a mod").unwrap();

        let out = dir.join("out");
        let c = convert(&src, &out, &|_| {}).unwrap();
        let results: Vec<(&str, &str)> = c
            .rows
            .iter()
            .map(|r| (r.file.as_str(), r.result.as_str()))
            .collect();
        assert_eq!(
            results,
            [
                ("a-gems.jar", "Works"),
                ("b-code.jar", "Doesn't run"),
                ("c-copy.jar", "Not converted")
            ]
        );
        assert_eq!(
            c.renamed.get("gem-mod").map(String::as_str),
            Some("gem_mod")
        );
        assert_eq!(c.installed, [out.join("horizon-mods/gem_mod")]);
        assert!(
            out.join("horizon-mods/gem_mod/data/gem_mod/horizon/block/gem.json")
                .is_file()
        );
        assert!(out.join("convert-mods-report.md").is_file());
        assert!(convert(&src, &out, &|_| {}).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
