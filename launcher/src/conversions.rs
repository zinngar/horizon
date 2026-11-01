//! Bringing a Java game to a Horizon instance, on this computer: a mods folder, a world, or a
//! whole instance another launcher keeps ([`crate::other_launchers`]), converted with
//! `horizon-port-pack` (docs/mod-convert.md, docs/world-convert.md).
//!
//! Converted mods go into the instance's `horizon-mods/` and are turned on, as the mod browser
//! installs them; a mod already there is left as it is. Worlds become new folders in its
//! `saves/`, never over one. Nothing a conversion reads is changed. Each run keeps its work and
//! the mods' report in `conversions/<time>/` in the game folder; each world keeps its own
//! report (`horizon-convert-report.md`) in its folder.
use crate::browser::java::{set_enabled, MODS_DIR};
use crate::i18n::tr;
use crate::other_launchers::Found;
use horizon_port_pack::{
    convert_mods::{self, Converted},
    world,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// What a conversion did to the instance.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Done {
    /// Mods installed and turned on, by id.
    pub mods: Vec<String>,
    /// Mods left as they were: one of the same id is in `horizon-mods/` already.
    pub kept: Vec<String>,
    /// Worlds made, by their folder in `saves/`.
    pub worlds: Vec<String>,
    /// What the player should know: jars that don't run, worlds that couldn't be converted.
    pub notes: Vec<String>,
    /// Where the run's work and reports are.
    pub work: PathBuf,
}

impl Done {
    /// One line for the page.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if !self.mods.is_empty() {
            parts.push(count(
                self.mods.len(),
                tr("{n} mod installed"),
                tr("{n} mods installed"),
            ));
        }
        if !self.worlds.is_empty() {
            parts.push(count(
                self.worlds.len(),
                tr("{n} world converted"),
                tr("{n} worlds converted"),
            ));
        }
        if !self.kept.is_empty() {
            parts.push(count(
                self.kept.len(),
                tr("{n} mod already there"),
                tr("{n} mods already there"),
            ));
        }
        if parts.is_empty() {
            return tr("Nothing Horizon can use was found.").to_owned();
        }
        format!("{}.", parts.join(", "))
    }
}

fn count(n: usize, one: &str, many: &str) -> String {
    (if n == 1 { one } else { many }).replace("{n}", &n.to_string())
}

/// A new `conversions/<time>/` in the game folder.
fn work(home: &Path) -> Result<PathBuf, String> {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let root = home.join("conversions");
    let mut n = 0;
    loop {
        let dir = root.join(if n == 0 {
            secs.to_string()
        } else {
            format!("{secs}-{n}")
        });
        if !dir.exists() {
            fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            return Ok(dir);
        }
        n += 1;
    }
}

/// Converts the mods folder `mods` and installs what runs into `home`.
pub fn mods(mods: &Path, home: &Path, log: &dyn Fn(String)) -> Result<Done, String> {
    let mut done = Done {
        work: work(home)?,
        ..Done::default()
    };
    let converted = convert_mods::convert(mods, &done.work.join("mods"), log)?;
    install(&converted, home, &mut done)?;
    Ok(done)
}

/// Converts the Java world `world` into a new world of `home`, with the mods it was played with
/// (`mods`, a mods folder) converted and installed first.
pub fn world(
    world: &Path,
    mods: Option<&Path>,
    home: &Path,
    log: &dyn Fn(String),
) -> Result<Done, String> {
    let mut done = Done {
        work: work(home)?,
        ..Done::default()
    };
    let converted = match mods {
        Some(mods) => Some(convert_mods::convert(mods, &done.work.join("mods"), log)?),
        None => None,
    };
    if let Some(converted) = &converted {
        install(converted, home, &mut done)?;
    }
    let folder = world_into(world, converted.as_ref(), home, log)?;
    done.worlds.push(folder);
    Ok(done)
}

/// An instance another launcher keeps: its mods, then every world in its `saves/`.
pub fn instance(found: &Found, home: &Path, log: &dyn Fn(String)) -> Result<Done, String> {
    let mut done = Done {
        work: work(home)?,
        ..Done::default()
    };
    let mods = found.game.join("mods");
    let converted = if mods.is_dir() {
        log(format!("Converting the mods of {}", found.name));
        let c = convert_mods::convert(&mods, &done.work.join("mods"), log)?;
        install(&c, home, &mut done)?;
        Some(c)
    } else {
        None
    };
    let mut saves: Vec<PathBuf> = fs::read_dir(found.game.join("saves"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("level.dat").is_file())
        .collect();
    saves.sort();
    for save in saves {
        let name = save.file_name().unwrap_or_default().to_string_lossy();
        log(format!("Converting the world {name}"));
        match world_into(&save, converted.as_ref(), home, log) {
            Ok(folder) => done.worlds.push(folder),
            Err(e) => done.notes.push(format!("{name}: {e}")),
        }
    }
    Ok(done)
}

/// The converted mods into `home/horizon-mods/`, turned on; what doesn't run, noted.
fn install(converted: &Converted, home: &Path, done: &mut Done) -> Result<(), String> {
    for row in &converted.rows {
        if row.installed.is_none() {
            done.notes
                .push(format!("{}: {}. {}", row.file, row.result, row.details));
        }
    }
    let dir = home.join(MODS_DIR);
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for path in &converted.installed {
        let file = path.file_name().unwrap_or_default().to_string_lossy();
        let id = file.trim_end_matches(".hmod").to_owned();
        let dest = dir.join(file.as_ref());
        if dest.exists() {
            done.kept.push(id);
            continue;
        }
        if path.is_dir() {
            copy_dir(path, &dest)?;
        } else {
            fs::copy(path, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        }
        set_enabled(home, &id, true)?;
        done.mods.push(id);
    }
    Ok(())
}

/// `world` converted into a new folder of `home/saves/`, named after it.
fn world_into(
    world: &Path,
    converted: Option<&Converted>,
    home: &Path,
    log: &dyn Fn(String),
) -> Result<String, String> {
    let saves = home.join("saves");
    fs::create_dir_all(&saves).map_err(|e| format!("{}: {e}", saves.display()))?;
    let name = world
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| format!("{} isn't a world folder", world.display()))?;
    let folder = (1..)
        .map(|n| {
            if n == 1 {
                name.clone()
            } else {
                format!("{name} ({n})")
            }
        })
        .find(|f| !saves.join(f).exists())
        .expect("some name is free");
    let out = saves.join(&folder);
    let options = world::Options {
        world,
        out: &out,
        mods: None,
        mods_out: None,
        remap: None,
        converted,
    };
    if let Err(e) = world::convert(&options, log) {
        // A world half written would show in the list: none rather than a broken one.
        let _ = fs::remove_dir_all(&out);
        return Err(e);
    }
    Ok(folder)
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    for entry in fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            fs::copy(&src, &dst).map_err(|e| format!("{}: {e}", dst.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
