# Converting a folder of Java mods

Java mod jars don't run in Horizon, but the content inside many of them does: blocks, items, recipes, loot,
tags, world generation, textures and models. `horizon-port-pack convert-mods` takes a launcher's mods folder
and makes Horizon mods of every jar that has content:

```sh
cargo run --release -p horizon-port-pack -- convert-mods ~/.minecraft/mods ~/Desktop/converted-mods
```

- Each jar is checked and carried over as [the mod browser](mod-browser.md) does it: **Works** (content only),
  **Partly** (the content; the row lists what its Java code did that is left out), **Doesn't run** (nothing but
  code: libraries, performance mods, screens and HUDs). Mods the reviewed ATM10 table knows say which kind they
  are.
- The ones with content go into `<out>/horizon-mods/<id>/` as folder mods. Copy everything there into the game
  folder's `horizon-mods/` (beside `options.txt`) and turn them on in Options > Mods. `.hmod` files and folder
  mods already in the mods folder go along as they are.
- Two mods that claim one namespace: the first by file name is kept, as the game keeps it.
- `<out>/convert-mods-report.md` lists every file with its result; `<out>/work/<jar>/` keeps each jar's
  `check.json`, its carry and the files left outside it.
- `<out>` must be new or empty. The jars are only read.

## Jars Horizon couldn't read before

Both are handled by rewriting a copy of the jar before the check (`crates/horizon-port-pack/src/prepare.rs`),
so the mod browser takes them too:

- **A mod id with dashes** (`cloth-config`; Fabric allows them, Horizon's ids are `[a-z0-9_]`): the id and its
  namespace become `cloth_config`: the asset and data paths, every `cloth-config:` id and `#cloth-config:` tag
  in the mod's JSON, and its translation keys. The check says `id cloth-config renamed cloth_config`.
  [`convert-world`](world-convert.md) renames the ids in a world played with the mod the same way.
- **Made for 1.20 or older** (`data/<ns>/recipes/`, `loot_tables/`): data folders take 1.21's names
  (`recipe/`, `loot_table/`, `advancement/`, `tags/item/`, `tags/block/`...); recipe results and advancement
  icons become 1.20.5's `{"id", "count"}` (item NBT is dropped: it has no sure translation to components);
  loot tables' `set_nbt`/`copy_nbt` become `set_custom_data`/`copy_custom_data`, and a tool match's item
  predicate takes 1.20.5's shape (`items`, enchantments under `predicates`); `minecraft:grass` and
  `minecraft:scute` take their new names. The carry then upgrades from 1.21.1 to 26.3 as for any jar.
  Mods from before 1.13 (`mcmod.info`, numeric ids) aren't upgraded.

The mod browser keeps its answers in `mod-browser/checks-2/`: answers from before this change are checked again.

## What no converter does

Java code doesn't run: machines, block entities, entities and their AI, screens, keybinds, Mixins and event
handlers are left out, and the report says so per mod. Porting them means writing a native Rust mod
([native-mods.md](native-mods.md)); [modpacks.md](modpacks.md) describes doing that with an agent per mod.

Code: `crates/horizon-port-pack/src/convert_mods.rs`, `prepare.rs`, `check.rs`.
