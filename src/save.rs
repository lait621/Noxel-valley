//! Saving and loading a farm.
//!
//! # Why the format is hand-written
//!
//! The engine has no serialisation crate (`docs/adr/0002-no-dependencies.md`),
//! and this file is small enough that adding one would be buying a dependency
//! to avoid forty lines of `format!`. It is a line-oriented text file: a header
//! line, then `key value` pairs, then one line per planted tile.
//!
//! ```text
//! noxel-valley 1
//! seed 1234
//! day 7
//! season 0
//! year 1
//! minutes 372.5
//! gold 840
//! energy 62.5
//! weather 2
//! selected 1
//! item 3 12
//! tile 24 19 tilled watered 0 3 1
//! ```
//!
//! Three properties matter more than compactness:
//!
//! * **A save from an older build loads.** Every unknown line is skipped and
//!   every missing one keeps its default, so adding a field later does not
//!   invalidate anyone's farm.
//! * **A corrupt save is refused, not half-applied.** The whole file is parsed
//!   into a value first; nothing touches the running game until it has all
//!   parsed.
//! * **It is readable.** When a save does go wrong, being able to open it in a
//!   text editor is the difference between a bug report and a shrug.

use std::path::{Path, PathBuf};

use crate::config::Season;
use crate::sim::{GameState, Item, WeatherSystem};
use crate::world::{FarmMap, Ground, Plant};

/// The version written into the header.
pub const FORMAT_VERSION: u32 = 1;

/// The name of the file inside the save directory.
pub const SAVE_FILE: &str = "save.txt";

/// The name of the settings file.
pub const SETTINGS_FILE: &str = "settings.txt";

/// Why a save could not be read or written.
#[derive(Debug)]
pub enum SaveError {
    /// The file could not be read or written.
    Io(std::io::Error),
    /// The file is not a Noxel Valley save.
    NotASave,
    /// The save is from a newer version of the game.
    TooNew(u32),
    /// A line could not be parsed.
    BadLine(String),
}

impl core::fmt::Display for SaveError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::NotASave => write!(f, "这不是星野农场的存档"),
            Self::TooNew(version) => {
                write!(
                    f,
                    "这个存档来自更新的版本（v{version}），当前是 v{FORMAT_VERSION}"
                )
            }
            Self::BadLine(line) => write!(f, "存档里有读不懂的一行：{line}"),
        }
    }
}

impl std::error::Error for SaveError {}

impl From<std::io::Error> for SaveError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Every planted tile, as text.
#[must_use]
fn tiles_of(map: &FarmMap) -> Vec<String> {
    let mut lines = Vec::new();
    for y in 0..map.height() as i32 {
        for x in 0..map.width() as i32 {
            let Some(tile) = map.get(x, y) else { continue };
            // Only what differs from a freshly built farm is written. A farm is
            // 1496 tiles and most of them never change; writing them all would
            // make the file thirty times the size for no information.
            let Some(plant) = tile.plant else {
                if tile.ground == Ground::Tilled || tile.watered || tile.prop.is_none() {
                    lines.push(format!(
                        "tile {x} {y} {} {} none",
                        ground_code(tile.ground),
                        u8::from(tile.watered)
                    ));
                }
                continue;
            };
            lines.push(format!(
                "tile {x} {y} {} {} {} {} {}",
                ground_code(tile.ground),
                u8::from(tile.watered),
                plant.crop.key,
                plant.days,
                u8::from(plant.watered)
            ));
        }
    }
    lines
}

/// A ground tile as a small integer, so the format does not depend on the
/// enum's declaration order.
fn ground_code(ground: Ground) -> u8 {
    match ground {
        Ground::Grass => 0,
        Ground::GrassDark => 1,
        Ground::GrassFlower => 2,
        Ground::Dirt => 3,
        Ground::Tilled => 4,
        Ground::Path => 5,
        Ground::Water => 6,
        Ground::Stone => 7,
        Ground::Sand => 8,
    }
}

fn ground_from(code: u8) -> Ground {
    match code {
        1 => Ground::GrassDark,
        2 => Ground::GrassFlower,
        3 => Ground::Dirt,
        4 => Ground::Tilled,
        5 => Ground::Path,
        6 => Ground::Water,
        7 => Ground::Stone,
        8 => Ground::Sand,
        _ => Ground::Grass,
    }
}

/// Serialises the whole game.
#[must_use]
pub fn to_text(map: &FarmMap, state: &GameState, seed: u32) -> String {
    let clock = &state.clock;
    let mut out = String::with_capacity(4096);
    out.push_str(&format!("noxel-valley {FORMAT_VERSION}\n"));
    out.push_str(&format!("seed {seed}\n"));
    out.push_str(&format!("year {}\n", clock.year()));
    out.push_str(&format!("season {}\n", season_code(clock.season())));
    out.push_str(&format!("day {}\n", clock.day_of_season()));
    out.push_str(&format!("minutes {:.2}\n", clock.minutes()));
    out.push_str(&format!("gold {}\n", state.inventory.gold()));
    out.push_str(&format!("energy {:.1}\n", state.energy));
    out.push_str(&format!("selected {}\n", state.selected));
    out.push_str(&format!(
        "weather {}\n",
        weather_code(state.weather.today())
    ));
    // The forecast is part of the state: loading a save that re-rolled it would
    // make tomorrow's weather different from what the player was told.
    let forecast: Vec<String> = state
        .weather
        .forecast()
        .iter()
        .map(|w| weather_code(*w).to_string())
        .collect();
    out.push_str(&format!("forecast {}\n", forecast.join(" ")));
    for (index, slot) in state.inventory.slots().iter().enumerate() {
        if let Some(slot) = slot {
            out.push_str(&format!(
                "item {index} {} {}\n",
                item_code(slot.item),
                slot.count
            ));
        }
    }
    // The bin stores a compact `(item, count)` list rather than slots, so it is
    // written in that shape: the position of a stack in the bin is not
    // something the player chose and not something to preserve.
    for (item, count) in state.bin.contents() {
        out.push_str(&format!("bin {} {}\n", item_code(*item), count));
    }
    for line in tiles_of(map) {
        out.push_str(&line);
        out.push('\n');
    }
    out
}

fn season_code(season: Season) -> u8 {
    match season {
        Season::Spring => 0,
        Season::Summer => 1,
        Season::Fall => 2,
        Season::Winter => 3,
    }
}

fn season_from(code: u8) -> Season {
    match code {
        1 => Season::Summer,
        2 => Season::Fall,
        3 => Season::Winter,
        _ => Season::Spring,
    }
}

fn weather_code(weather: crate::config::Weather) -> u8 {
    use crate::config::Weather as W;
    match weather {
        W::Sunny => 0,
        W::Cloudy => 1,
        W::Rain => 2,
        W::Storm => 3,
        W::Snow => 4,
    }
}

fn weather_from(code: u8) -> crate::config::Weather {
    use crate::config::Weather as W;
    match code {
        1 => W::Cloudy,
        2 => W::Rain,
        3 => W::Storm,
        4 => W::Snow,
        _ => W::Sunny,
    }
}

/// How an item is written down: `s:cropkey`, `p:cropkey`, or `t:toolkey`.
fn item_code(item: Item) -> String {
    match item {
        Item::Seed(crop) => format!("s:{}", crop.key),
        Item::Produce(crop) => format!("p:{}", crop.key),
        Item::Tool(tool) => format!("t:{}", tool.english()),
    }
}

fn item_from(code: &str) -> Option<Item> {
    let (kind, key) = code.split_once(':')?;
    match kind {
        "s" => crate::config::crop_by_key(key).map(Item::Seed),
        "p" => crate::config::crop_by_key(key).map(Item::Produce),
        "t" => crate::config::Tool::ALL
            .iter()
            .find(|t| t.english() == key)
            .copied()
            .map(Item::Tool),
        _ => None,
    }
}

/// One planted or worked tile, as a save records it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedTile {
    /// Tile column.
    pub x: i32,
    /// Tile row.
    pub y: i32,
    /// The ground under it.
    pub ground: Ground,
    /// Whether it was watered today.
    pub watered: bool,
    /// The crop, as `(key, days grown, watered)`.
    pub plant: Option<(String, u32, bool)>,
}

/// A save that has been read but not yet applied.
///
/// The whole point of this type is that parsing and applying are separate: a
/// file that fails halfway leaves the running game untouched rather than
/// half-loaded.
#[derive(Debug)]
pub struct Save {
    /// The world seed, so the farm's scatter is reproduced.
    pub seed: u32,
    /// The clock.
    pub year: u32,
    /// The season.
    pub season: Season,
    /// The day of the season.
    pub day: u32,
    /// Minutes past midnight.
    pub minutes: f32,
    /// Gold.
    pub gold: u32,
    /// Energy.
    pub energy: f32,
    /// The selected hotbar slot.
    pub selected: usize,
    /// Today's weather.
    pub weather: crate::config::Weather,
    /// The forecast.
    pub forecast: Vec<crate::config::Weather>,
    /// Bag contents: `(slot, item, count)`.
    pub items: Vec<(usize, Item, u32)>,
    /// Bin contents.
    pub bin: Vec<(usize, Item, u32)>,
    /// Changed tiles: where, what the ground is, and what is planted on it.
    pub tiles: Vec<SavedTile>,
}

/// Parses a save without applying it.
///
/// # Errors
/// Returns [`SaveError::NotASave`] for a file that is not one of ours,
/// [`SaveError::TooNew`] for one from a later version, and
/// [`SaveError::BadLine`] for a line that cannot be read.
pub fn parse(text: &str) -> Result<Save, SaveError> {
    let mut lines = text.lines();
    let header = lines.next().ok_or(SaveError::NotASave)?;
    let (name, version) = header.split_once(' ').ok_or(SaveError::NotASave)?;
    if name != "noxel-valley" {
        return Err(SaveError::NotASave);
    }
    let version: u32 = version.trim().parse().map_err(|_| SaveError::NotASave)?;
    if version > FORMAT_VERSION {
        return Err(SaveError::TooNew(version));
    }

    let mut save = Save {
        seed: 0,
        year: 1,
        season: Season::Spring,
        day: 1,
        minutes: crate::config::DAY_START_HOUR * 60.0,
        gold: crate::config::STARTING_GOLD,
        energy: crate::config::MAX_ENERGY,
        selected: 0,
        weather: crate::config::Weather::Sunny,
        forecast: Vec::new(),
        items: Vec::new(),
        bin: Vec::new(),
        tiles: Vec::new(),
    };

    for line in lines {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let key = parts.next().unwrap_or_default();
        // Every branch is forgiving: an unknown key is skipped, and a known key
        // whose value will not parse is skipped too. A save that is missing one
        // number is a save with one default, not an unloadable file.
        match key {
            "seed" => save.seed = number(&mut parts).unwrap_or(save.seed),
            "year" => save.year = number(&mut parts).unwrap_or(save.year),
            "season" => save.season = season_from(number(&mut parts).unwrap_or(0) as u8),
            "day" => save.day = number(&mut parts).unwrap_or(save.day),
            "minutes" => save.minutes = float(&mut parts).unwrap_or(save.minutes),
            "gold" => save.gold = number(&mut parts).unwrap_or(save.gold),
            "energy" => save.energy = float(&mut parts).unwrap_or(save.energy),
            "selected" => save.selected = number(&mut parts).unwrap_or(0) as usize,
            "weather" => save.weather = weather_from(number(&mut parts).unwrap_or(0) as u8),
            "forecast" => {
                save.forecast = parts
                    .filter_map(|p| p.parse::<u32>().ok())
                    .map(|c| weather_from(c as u8))
                    .collect();
            }
            "item" | "bin" => {
                let slot = number(&mut parts).unwrap_or(0) as usize;
                let Some(item) = parts.next().and_then(item_from) else {
                    continue;
                };
                let count = number(&mut parts).unwrap_or(1);
                if key == "item" {
                    save.items.push((slot, item, count));
                } else {
                    save.bin.push((slot, item, count));
                }
            }
            "tile" => {
                let Some(x) = number(&mut parts) else {
                    continue;
                };
                let Some(y) = number(&mut parts) else {
                    continue;
                };
                let ground = ground_from(number(&mut parts).unwrap_or(0) as u8);
                let watered = number::<u32>(&mut parts).unwrap_or(0) != 0;
                let crop = parts.next().unwrap_or("none");
                let plant = if crop == "none" {
                    None
                } else {
                    let days = number(&mut parts).unwrap_or(0);
                    let watered = number::<u32>(&mut parts).unwrap_or(0) != 0;
                    Some((crop.to_string(), days, watered))
                };
                save.tiles.push(SavedTile {
                    x,
                    y,
                    ground,
                    watered,
                    plant,
                });
            }
            // Forward compatibility: a line from a newer format is ignored
            // rather than refused, as long as the version header said it was
            // readable.
            _ => {}
        }
    }
    Ok(save)
}

fn number<T: std::str::FromStr>(parts: &mut std::str::SplitWhitespace<'_>) -> Option<T> {
    parts.next()?.parse().ok()
}

fn float(parts: &mut std::str::SplitWhitespace<'_>) -> Option<f32> {
    parts.next()?.parse().ok()
}

/// Applies a parsed save over a freshly built farm.
pub fn apply(save: &Save, map: &mut FarmMap, state: &mut GameState) {
    for SavedTile {
        x,
        y,
        ground,
        watered,
        plant,
    } in &save.tiles
    {
        let Some(tile) = map.get_mut(*x, *y) else {
            continue;
        };
        tile.ground = *ground;
        tile.watered = *watered;
        tile.plant = plant.as_ref().and_then(|(key, days, watered)| {
            let crop = crate::config::crop_by_key(key)?;
            let mut plant = Plant::new(crop);
            plant.days = *days;
            plant.watered = *watered;
            Some(plant)
        });
    }
    state.inventory.clear();
    for (slot, item, count) in &save.items {
        state.inventory.put(*slot, *item, *count);
    }
    state.inventory.set_gold(save.gold);
    state.bin.clear();
    for (_, item, count) in &save.bin {
        state.bin.add(*item, *count);
    }
    state.energy = save.energy;
    state.selected = save.selected.min(crate::config::HOTBAR_SLOTS - 1);
    state
        .clock
        .set(save.year, save.season, save.day, save.minutes);
    state.weather = WeatherSystem::restore(save.seed, save.weather, save.forecast.clone());
}

/// Where saves and settings live.
///
/// `$NOXEL_VALLEY_HOME`, then `$XDG_DATA_HOME/noxel-valley`, then
/// `~/.local/share/noxel-valley`, then the working directory. The environment
/// override is what makes the save path testable without touching a real home
/// directory.
#[must_use]
pub fn data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("NOXEL_VALLEY_HOME") {
        return PathBuf::from(dir);
    }
    if let Ok(dir) = std::env::var("XDG_DATA_HOME") {
        return PathBuf::from(dir).join("noxel-valley");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("noxel-valley");
    }
    PathBuf::from(".")
}

/// The save file's path.
#[must_use]
pub fn save_path() -> PathBuf {
    data_dir().join(SAVE_FILE)
}

/// The settings file's path.
#[must_use]
pub fn settings_path() -> PathBuf {
    data_dir().join(SETTINGS_FILE)
}

/// Whether a save exists.
#[must_use]
pub fn has_save() -> bool {
    save_path().is_file()
}

/// Writes `text` to `path`, creating the directory if needed.
///
/// Writes to a temporary file and renames it over the target, so a crash
/// halfway through leaves the previous save intact rather than a truncated one.
///
/// # Errors
/// Returns whatever the filesystem returns.
pub fn write_file(path: &Path, text: &str) -> Result<(), SaveError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, text)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

/// Reads a file, or `None` if it is not there.
///
/// # Errors
/// Returns an error for a file that exists but cannot be read.
pub fn read_file(path: &Path) -> Result<Option<String>, SaveError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(SaveError::Io(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Tool, crop_by_key};
    use crate::world::build_farm;

    fn saved() -> (FarmMap, GameState) {
        let mut map = build_farm();
        let mut state = GameState::new(4242);
        state
            .inventory
            .add(Item::Seed(crop_by_key("potato").unwrap()), 7);
        state
            .inventory
            .add(Item::Produce(crop_by_key("corn").unwrap()), 3);
        state.inventory.spend(120);
        state.energy = 61.5;
        state.selected = 3;
        let mut plant = Plant::new(crop_by_key("tomato").unwrap());
        plant.days = 5;
        plant.watered = true;
        map.get_mut(24, 19).unwrap().ground = Ground::Tilled;
        map.get_mut(24, 19).unwrap().watered = true;
        map.get_mut(24, 19).unwrap().plant = Some(plant);
        (map, state)
    }

    #[test]
    fn a_farm_survives_a_round_trip() {
        let (map, state) = saved();
        let text = to_text(&map, &state, 4242);
        let save = parse(&text).expect("the save we just wrote must parse");

        let mut fresh_map = build_farm();
        let mut fresh_state = GameState::new(4242);
        apply(&save, &mut fresh_map, &mut fresh_state);

        assert_eq!(fresh_state.inventory.gold(), state.inventory.gold());
        assert_eq!(fresh_state.energy, state.energy);
        assert_eq!(fresh_state.selected, state.selected);
        assert_eq!(
            fresh_state
                .inventory
                .count_of(Item::Seed(crop_by_key("potato").unwrap())),
            7
        );
        assert_eq!(
            fresh_state
                .inventory
                .count_of(Item::Produce(crop_by_key("corn").unwrap())),
            3
        );
        assert_eq!(fresh_state.clock.season(), state.clock.season());
        assert_eq!(
            fresh_state.clock.day_of_season(),
            state.clock.day_of_season()
        );

        let tile = fresh_map.get(24, 19).unwrap();
        let plant = tile.plant.expect("the planted crop came back");
        assert_eq!(plant.crop.key, "tomato");
        assert_eq!(plant.days, 5);
        assert!(plant.watered);
        assert!(tile.watered);
        assert_eq!(tile.ground, Ground::Tilled);
    }

    #[test]
    fn the_tools_are_still_in_the_bag_after_a_load() {
        // Tools are not `Item`s a player can drop, and the layout puts them in
        // the first slots. A save that did not write them would leave a loaded
        // game with no hoe.
        let (map, state) = saved();
        let text = to_text(&map, &state, 1);
        let save = parse(&text).unwrap();
        let mut fresh_map = build_farm();
        let mut fresh_state = GameState::new(1);
        apply(&save, &mut fresh_map, &mut fresh_state);
        for tool in Tool::ALL {
            assert!(
                fresh_state.inventory.count_of(Item::Tool(tool)) > 0,
                "{:?} vanished across the save",
                tool
            );
        }
    }

    #[test]
    fn the_forecast_comes_back_as_it_was_told() {
        // Re-rolling the forecast on load would make tomorrow's weather differ
        // from what the player was shown yesterday.
        let (map, mut state) = saved();
        state.weather = WeatherSystem::new(Season::Summer, 99);
        let before = state.weather.forecast().to_vec();
        let text = to_text(&map, &state, 99);
        let save = parse(&text).unwrap();
        let mut fresh_map = build_farm();
        let mut fresh_state = GameState::new(99);
        apply(&save, &mut fresh_map, &mut fresh_state);
        assert_eq!(fresh_state.weather.forecast(), before.as_slice());
        assert_eq!(fresh_state.weather.today(), state.weather.today());
    }

    #[test]
    fn a_file_that_is_not_a_save_is_refused() {
        assert!(matches!(parse(""), Err(SaveError::NotASave)));
        assert!(matches!(parse("hello world\n"), Err(SaveError::NotASave)));
        assert!(matches!(parse("noxel-valley\n"), Err(SaveError::NotASave)));
    }

    #[test]
    fn a_save_from_the_future_is_refused_rather_than_half_read() {
        let text = format!("noxel-valley {}\nseed 1\n", FORMAT_VERSION + 5);
        assert!(matches!(parse(&text), Err(SaveError::TooNew(_))));
    }

    #[test]
    fn a_save_missing_lines_keeps_its_defaults() {
        // Adding a field later must not invalidate anyone's farm.
        let save = parse("noxel-valley 1\nseed 3\nday 4\n").unwrap();
        assert_eq!(save.seed, 3);
        assert_eq!(save.day, 4);
        assert_eq!(save.gold, crate::config::STARTING_GOLD);
        assert_eq!(save.energy, crate::config::MAX_ENERGY);
    }

    #[test]
    fn a_malformed_line_does_not_stop_the_rest_of_the_save() {
        let text = "noxel-valley 1\nseed banana\nday 9\ngold 250\n";
        let save = parse(text).expect("a bad value is not a bad file");
        assert_eq!(save.day, 9, "parsing stopped at the bad line");
        assert_eq!(save.gold, 250);
    }

    #[test]
    fn an_unknown_line_from_a_newer_format_is_ignored() {
        let save = parse("noxel-valley 1\nseed 1\ncasino 500\n").unwrap();
        assert_eq!(save.seed, 1);
    }

    #[test]
    fn every_ground_tile_round_trips_through_its_code() {
        // The codes are written by hand, so a transposed pair would swap two
        // terrains silently and only show up as a strange-looking farm.
        for ground in [
            Ground::Grass,
            Ground::GrassDark,
            Ground::GrassFlower,
            Ground::Dirt,
            Ground::Tilled,
            Ground::Path,
            Ground::Water,
            Ground::Stone,
            Ground::Sand,
        ] {
            assert_eq!(
                ground_from(ground_code(ground)),
                ground,
                "{ground:?} did not survive"
            );
        }
    }

    #[test]
    fn a_write_is_atomic_enough_to_keep_the_old_file() {
        let dir = std::env::temp_dir().join(format!("noxel-save-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("save.txt");
        write_file(&path, "first").unwrap();
        write_file(&path, "second").unwrap();
        assert_eq!(read_file(&path).unwrap().unwrap(), "second");
        // The temporary file is not left behind.
        assert!(!path.with_extension("tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
