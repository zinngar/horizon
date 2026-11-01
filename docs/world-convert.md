# Converting a modded Java world

Horizon opens vanilla Java worlds from 1.8 on and upgrades them as it loads them. A world played with
Forge, NeoForge or Fabric mods holds blocks, items, biomes and entities that only those mods define:
vanilla turns unknown blocks into air, so a modded world opened as it is loses them. `horizon-port-pack
convert-world` makes a copy that opens in Horizon. The launcher does it too: **Convert a Java world…** and
**Bring to Horizon** on the Mods page ([mod-convert.md](mod-convert.md)), or `horizon-launcher worlds convert`:

```sh
cargo run --release -p horizon-port-pack -- convert-world \
  ~/.minecraft/saves/MyWorld "~/Desktop/MyWorld (Horizon)" --mods ~/.minecraft/mods
```

1. **Mods.** The folder in `--mods` is converted with [`convert-mods`](mod-convert.md) into
   `--mods-out` (default `<out>-horizon-mods`). Whatever the converted mods add stays in the world:
   copy `<mods-out>/horizon-mods/` into the game folder's `horizon-mods/` and turn them on before
   opening it. A mod whose dashed id was renamed (`gem-mod` to `gem_mod`) has its ids renamed in the
   world too.
2. **Every other modded id** is decided by kind:

   | In the world | Becomes |
   |---|---|
   | Block (section palettes) | `remap.toml`'s choice, else a guess from its name: ores → stone (netherrack, end stone), logs → oak log (axis kept), planks, leaves, stairs, slabs, walls, fences, glass, wool, sand, dirt..., plants, pipes, doors, torches and fluids → air, anything else → stone |
   | Item stack (anywhere: containers, inventories, equipment, items in items) | `remap.toml`'s choice, else the block guess when it is one, else **paper named `Missing: <id>`** holding the original stack in its custom data, with the same count |
   | Component of a mod that goes, on a kept item | removed |
   | Biome (1.18 on) | `remap.toml`'s choice, else plains (nether wastes, the End by dimension) |
   | Block entity, entity, point of interest, scheduled tick | removed (content mods have no code to run them) |
   | Structure start and reference | removed, unless a converted mod adds the structure |
   | Dimension (`dimensions/<ns>/`, `DIM<n>`) | left out of the copy; players in it are moved to the world spawn |
   | Loader data: `ForgeCaps`, `ForgeData`, `NeoForgeData`, `neoforge:attachments`, `fabric:attachments`, `cardinal_components`, `level.dat`'s `fml`/`forge`, loader data packs (`mod:<id>`, `fabric`) | removed |

   A rewritten chunk has its light turned off (`isLightOn`), so the game lights it again. Chunks
   nothing changed keep their bytes; a chunk that doesn't read is copied as it is and listed.
3. **The copy** is the world's own version: Horizon's datafix upgrades it to 26.3 when it opens. The
   original world is only read. `<out>` must be new or empty.

`<out>/horizon-convert-report.md` lists the mods and every modded id, what it became and how often.
`<out>/remap-suggested.toml` is the same choices as a remap: edit it and convert again into a new folder
with `--remap`:

```toml
[blocks]
"create:zinc_ore" = "minecraft:iron_ore"
"create:andesite_casing" = "minecraft:stripped_oak_wood[axis=y]"

[items]
"create:wrench" = "minecraft:stick"

[biomes]
"biomesoplenty:redwood_forest" = "minecraft:old_growth_spruce_taiga"
```

Remap targets are vanilla ids that exist in the world's own version (the game renames them on upgrade).

## Not converted yet

- Forge worlds from before 1.13: their blocks are numbers that only Forge's registry in `level.dat`
  names. Refused with a message.
- A modded container's items: the block entity goes with them (the report counts it). Moving them into
  a vanilla chest where the container stood is the next step.
- Modded state properties on a kept block that its Horizon version doesn't declare.
- Biomes before 1.18 (numeric arrays), and modded world generation settings for the vanilla dimensions.

Code: `crates/horizon-port-pack/src/world/` (`ids.rs` decides, `chunk.rs` rewrites, `mod.rs` walks the
world); tests in `world/tests.rs`.
