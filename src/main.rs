//! # Noxel Valley
//!
//! A playable top-down farming game, and the reference project for the engine's
//! UI layer.
//!
//! ```text
//! cargo run -p noxel-valley --features window -- --window
//! cargo run -p noxel-valley -- --frames 600 --dump frames
//! ```
//!
//! ## How a frame is put together
//!
//! The game is one `Plugin`. That is the engine's unit of composition, and it
//! buys the whole frame loop for free:
//!
//! ```text
//!   App::step(dt)
//!     fixed_update    ValleyPlugin::update
//!                       player movement, tool use, the clock, the day rollover
//!     frame_update    camera follows app.context.focus, which update() set
//!     render          one mesh per atlas, no culling needed at this size
//!     draw_overlays   ValleyPlugin::draw  <- the entire UI, still in linear space
//!     resolve         linear HDR -> sRGB, once
//! ```
//!
//! Drawing the UI in `draw` rather than after `resolve` is the important part. It
//! means the interface is composited in the same linear space as the world, one
//! tone curve applies to both, and a colour specified as `#FF0000` in the theme
//! resolves to exactly `#FF0000` on screen.
//!
//! ## Why the game owns its own input
//!
//! The window host produces `noxel_window::Input` with the cursor already mapped
//! into framebuffer pixels. The game copies it into a `UiInput` before stepping,
//! rather than pushing it through `App::input_mut()`, because the UI needs the
//! pointer and `InputState` is keyboard-only. One conversion, in one place.

use std::cell::RefCell;
use std::rc::Rc;

use noxel_app::{App, AppConfig, Plugin};
use noxel_core::math::{Color, Color8, Vec3};
use noxel_render::framebuffer::Framebuffer;
use noxel_render::material::{AlphaMode, Material};
use noxel_render::renderer::ShadingMode;
use noxel_render::scene::{InstanceFlags, InstanceHandle};
use noxel_ui::{Ui, UiInput};

use noxel_valley::assets::Assets;
use noxel_valley::config::{
    FARM_HEIGHT, FARM_WIDTH, INTERNAL_HEIGHT, INTERNAL_WIDTH, ORTHO_HEIGHT,
};
use noxel_valley::player::Player;
use noxel_valley::sim::{GameState, Item};
use noxel_valley::ui::{GameUi, Screen, UiAction};
use noxel_valley::world::FarmMap;
use noxel_valley::{action, assets, config, save, screens, sim, skin, world};

/// The farm, the player, the clock and the interface.
struct Valley {
    map: FarmMap,
    player: Player,
    state: GameState,
    game_ui: GameUi,
    /// The frame's input, in framebuffer pixels.
    input: UiInput,
    /// The map revision the meshes were built from.
    built_revision: u64,
    /// Multiplies the clock for `--fast`.
    fast: bool,
    /// The three materials, so the time of day can dim them all at once.
    ground_material: noxel_render::material::MaterialHandle,
    crop_material: noxel_render::material::MaterialHandle,
    prop_material: noxel_render::material::MaterialHandle,
    /// The single instance each mesh is drawn through.
    ground_instance: Option<InstanceHandle>,
    crop_instance: Option<InstanceHandle>,
    prop_instance: Option<InstanceHandle>,
    /// The character material and the player's current animation.
    character_material: noxel_render::material::MaterialHandle,
    player_instance: Option<InstanceHandle>,
    player_sprite: String,
    /// A villager wandering the plaza, so the farm is not deserted.
    villager: Villager,
    villager_instance: Option<InstanceHandle>,
    villager_sprite: String,
    /// Set when the player asks to sleep early.
    sleep_requested: bool,
    /// Yesterday's takings, for the morning report.
    last_report: Option<sim::DaySummary>,
    /// Set when the player asked to leave.
    quit: bool,
    /// The world seed in play, so a save can reproduce the farm's scatter.
    seed: u32,
    /// The most recent tool use, for the log and for tests.
    last_action: Option<String>,
}

impl Valley {
    /// Builds the farm, the player and the interface.
    fn new(app: &mut App, seed: u32, assets: &Assets, fast: bool) -> Self {
        let map = world::build_farm();
        let player = Player::at(world::START_TILE);
        let ui = Ui::new(
            assets.font.clone().unwrap_or_else(default_font),
            skin::build(assets.atlases.ui.as_ref()),
            assets.ui_texture().clone(),
        );

        // One material per atlas, because a material carries one texture. The
        // ground is opaque and the sprites are cutouts: a cutout writes depth,
        // which is what lets the depth buffer sort a tree in front of the player
        // standing below it.
        // The texture handle is resolved *before* the scene is borrowed to add
        // the material: `add_material` takes `&mut scene`, and passing
        // `texture_handle(app, ..)` as an argument would borrow the app twice.
        let terrain_texture = texture_handle(app, assets, "terrain");
        let crop_texture = texture_handle(app, assets, "crops");
        let prop_texture = texture_handle(app, assets, "props");
        let ground_material = app.scene_mut().add_material(
            Material::sprite("valley.ground", terrain_texture).without_shadow_casting(),
        );
        let crop_material = app.scene_mut().add_material(
            Material::sprite("valley.crops", crop_texture)
                .with_alpha_mode(AlphaMode::cutout(0.5))
                .without_shadow_casting(),
        );
        let prop_material = app.scene_mut().add_material(
            Material::sprite("valley.props", prop_texture)
                .with_alpha_mode(AlphaMode::cutout(0.5))
                .without_shadow_casting(),
        );
        let character_texture = texture_handle(app, assets, "characters");
        let character_material = app.scene_mut().add_material(
            Material::sprite("valley.characters", character_texture)
                .with_alpha_mode(AlphaMode::cutout(0.5))
                .without_shadow_casting(),
        );

        setup_camera(app);
        app.scene_mut().clear_lights();
        app.scene_mut().background = Color::rgb(0.05, 0.06, 0.09);

        let mut valley = Self {
            map,
            player,
            state: GameState::new(seed),
            game_ui: GameUi::new(ui),
            input: UiInput::new(),
            built_revision: 0,
            fast,
            ground_material,
            crop_material,
            prop_material,
            ground_instance: None,
            crop_instance: None,
            prop_instance: None,
            character_material,
            player_instance: None,
            player_sprite: String::new(),
            villager: Villager::default(),
            villager_instance: None,
            villager_sprite: String::new(),
            sleep_requested: false,
            last_report: None,
            quit: false,
            seed,
            last_action: None,
        };
        valley.rebuild(app, assets);
        valley
    }

    /// Rebuilds the farm meshes if the map changed.
    fn rebuild(&mut self, app: &mut App, assets: &Assets) {
        if self.built_revision == self.map.revision() {
            return;
        }
        let (Some(terrain), Some(crops), Some(props)) = (
            assets.atlases.terrain.as_ref(),
            assets.atlases.crops.as_ref(),
            assets.atlases.props.as_ref(),
        ) else {
            // No art: the ground is a flat colour and there is nothing to build.
            self.built_revision = self.map.revision();
            return;
        };
        let meshes = world::build_meshes(&self.map, terrain, crops, props);

        for (mesh, material, slot) in [
            (meshes.ground, self.ground_material, 0u8),
            (meshes.crops, self.crop_material, 1),
            (meshes.props, self.prop_material, 2),
        ] {
            if mesh.is_empty() {
                continue;
            }
            let handle = app.scene_mut().add_mesh(mesh);
            let instance = app.scene_mut().spawn(
                format!("farm.{slot}"),
                handle,
                material,
                noxel_core::math::Transform::IDENTITY,
            );
            // Ground and crops are never occluders: nothing should fade because
            // the camera is behind a blade of grass.
            app.scene_mut().set_flags(
                instance,
                InstanceFlags {
                    occluder: false,
                    cast_shadow: false,
                    ..InstanceFlags::default()
                },
            );
            match slot {
                0 => {
                    self.release(app, self.ground_instance);
                    self.ground_instance = Some(instance);
                }
                1 => {
                    self.release(app, self.crop_instance);
                    self.crop_instance = Some(instance);
                }
                _ => {
                    self.release(app, self.prop_instance);
                    self.prop_instance = Some(instance);
                }
            }
        }
        app.scene_mut().update_all_bounds();
        self.built_revision = self.map.revision();
    }

    /// Rebuilds a character's mesh when its animation frame changes.
    ///
    /// Separate from [`Valley::rebuild`] because the farm changes a few times a
    /// second and the walk cycle changes six times a second: sharing one dirty
    /// flag would rebuild six thousand vertices to animate one sprite.
    fn refresh_characters(&mut self, app: &mut App, assets: &Assets) {
        let Some(atlas) = assets.atlases.characters.as_ref() else {
            return;
        };

        let player_sprite = self.player.sprite();
        if player_sprite != self.player_sprite {
            if let Some(instance) = self.player_instance.take() {
                app.scene_mut().remove_instance(instance);
            }
            let mesh = world::build_character_mesh(atlas, &player_sprite);
            if !mesh.is_empty() {
                let handle = app.scene_mut().add_mesh(mesh);
                let instance = app.scene_mut().spawn(
                    "player",
                    handle,
                    self.character_material,
                    noxel_core::math::Transform::IDENTITY,
                );
                app.scene_mut().set_flags(
                    instance,
                    InstanceFlags {
                        occluder: false,
                        cast_shadow: false,
                        ..InstanceFlags::default()
                    },
                );
                self.player_instance = Some(instance);
            }
            self.player_sprite = player_sprite;
        }

        let villager_sprite = self.villager.sprite();
        if villager_sprite != self.villager_sprite {
            if let Some(instance) = self.villager_instance.take() {
                app.scene_mut().remove_instance(instance);
            }
            let mesh = world::build_character_mesh(atlas, &villager_sprite);
            if !mesh.is_empty() {
                let handle = app.scene_mut().add_mesh(mesh);
                let instance = app.scene_mut().spawn(
                    "villager",
                    handle,
                    self.character_material,
                    noxel_core::math::Transform::IDENTITY,
                );
                app.scene_mut().set_flags(
                    instance,
                    InstanceFlags {
                        occluder: false,
                        cast_shadow: false,
                        ..InstanceFlags::default()
                    },
                );
                self.villager_instance = Some(instance);
            }
            self.villager_sprite = villager_sprite;
        }
    }

    /// Moves the character instances to where the characters are.
    ///
    /// The transform carries the ground position and the depth offset; the mesh
    /// is anchored at the sprite's feet, so the depth is taken at the feet. A
    /// sprite sorted by its head would let the player walk in front of a fence
    /// they are standing behind.
    fn place_characters(&mut self, app: &mut App) {
        let player_feet = self.player.position.y + 0.5;
        if let Some(instance) = self.player_instance {
            app.scene_mut().set_transform(
                instance,
                noxel_core::math::Transform::from_translation(Vec3::new(
                    self.player.position.x,
                    world::depth_of(player_feet),
                    player_feet,
                )),
            );
        }
        let villager_feet = self.villager.position.1 + 0.5;
        if let Some(instance) = self.villager_instance {
            app.scene_mut().set_transform(
                instance,
                noxel_core::math::Transform::from_translation(Vec3::new(
                    self.villager.position.0,
                    world::depth_of(villager_feet),
                    villager_feet,
                )),
            );
        }
    }

    fn release(&self, app: &mut App, instance: Option<InstanceHandle>) {
        if let Some(handle) = instance {
            app.scene_mut().remove_instance(handle);
        }
    }

    /// Applies the time of day and the weather to the three materials.
    ///
    /// One place, three materials. Everything in the world is unlit — pixel art
    /// has its lighting painted in — so "the sun went down" is a multiplier on
    /// the base colour and nothing else.
    fn update_lighting(&mut self, app: &mut App) {
        let hour = self.state.clock.hour();
        // Bright from mid-morning to late afternoon, falling off at both ends,
        // with a floor so the farm is never unplayably dark.
        let daylight = match hour {
            h if h < 7.0 => 0.55,
            h if h < 9.0 => 0.55 + (h - 7.0) / 2.0 * 0.4,
            h if h < 17.0 => 0.95,
            h if h < 20.0 => 0.95 - (h - 17.0) / 3.0 * 0.35,
            _ => 0.6,
        };
        let level = (daylight * self.state.weather.today().light()).clamp(0.35, 1.0);
        let season = self.state.clock.season().tint();
        let tint = Color8::new(
            (f32::from(season.r) * 1.0) as u8,
            (f32::from(season.g) * 1.0) as u8,
            (f32::from(season.b) * 1.0) as u8,
            255,
        );
        let apply =
            |app: &mut App, handle: noxel_render::material::MaterialHandle, tint: Color8| {
                if let Some(material) = app.scene_mut().material_mut(handle) {
                    material.base_color = Color::rgb(
                        level * f32::from(tint.r) / 255.0,
                        level * f32::from(tint.g) / 255.0,
                        level * f32::from(tint.b) / 255.0,
                    );
                }
            };
        // Only the ground takes the season's tint: a pumpkin should not turn
        // orange-er in autumn because the grass did.
        apply(app, self.ground_material, tint);
        apply(app, self.crop_material, Color8::WHITE);
        apply(app, self.prop_material, Color8::WHITE);
    }

    /// The fixed-step update.
    fn update(&mut self, app: &mut App, dt: f32, assets: &Assets) {
        // The toast is on a timer and nothing was winding it. Every message the
        // game ever showed stayed on screen for the rest of the session, which
        // is how "任务完成" became a permanent fixture of the interface.
        self.game_ui.tick(dt);
        self.handle_keys();

        let playing = self.game_ui.screen == Screen::Playing && !self.game_ui.help_open;
        if playing {
            let axis = movement_axis(&self.input);
            let running = self.input.shift;
            let travelled = self.player.update(&self.map, axis, running, dt);

            // Walking costs energy in proportion to distance, not to frames, so
            // a slow machine does not tire the player faster.
            let minutes = dt * config::GAME_MINUTES_PER_SECOND;
            if travelled > 1e-4 {
                self.state.drain_walking_energy(minutes);
            }

            if self.input.primary_pressed || self.input.key_pressed(KEY_SPACE) {
                self.use_tool();
            }
            if self.input.secondary_pressed {
                self.use_tool();
            }
            if self.input.key_pressed(KEY_E) {
                self.interact();
            }
        }

        // The clock, and the end of the day.
        //
        // `sleep_requested` is consumed here and nowhere else. It used to be a
        // one-way flag: once pressing E set it, every following frame slept
        // again, so the calendar ran forward at sixty days a second and the
        // summary screen could never be dismissed. A request is a request
        // because it is spent.
        let day_over = self.state.clock.advance(dt, self.fast);
        let asked_to_sleep = std::mem::take(&mut self.sleep_requested);
        if day_over || asked_to_sleep {
            self.end_day(day_over);
        }

        self.villager.update(dt, &self.map);
        self.update_lighting(app);
        self.rebuild(app, assets);
        self.refresh_characters(app, assets);
        self.place_characters(app);
        app.context.focus = Vec3::new(self.player.position.x, 0.0, self.player.position.y);
    }

    /// Reads the keys that are not movement.
    fn handle_keys(&mut self) {
        if self.input.key_pressed(KEY_TAB) {
            self.game_ui.screen = match self.game_ui.screen {
                Screen::Inventory => Screen::Playing,
                _ => Screen::Inventory,
            };
        }
        if self.input.key_pressed(KEY_SLASH) || self.input.key_pressed(KEY_H) {
            self.game_ui.help_open = !self.game_ui.help_open;
        }
        // Escape walks back out of whatever is open, one layer at a time, and
        // from the world it opens the pause menu. It never quits: a key that
        // ends the session without asking is a key that loses someone's season
        // to a mis-press, and holding it must not do anything at all — which is
        // what `key_pressed` (an edge, not a level) guarantees.
        if self.input.key_pressed(KEY_ESCAPE) {
            let ui = &mut self.game_ui;
            if ui.help_open {
                ui.help_open = false;
            } else {
                match ui.screen {
                    Screen::Playing => ui.screen = Screen::Pause,
                    Screen::Pause => ui.screen = Screen::Playing,
                    Screen::Title => {}
                    _ => ui.screen = Screen::Playing,
                }
            }
        }
        // Number keys select a hotbar slot, and the wheel cycles.
        for (index, key) in [KEY_1, KEY_2, KEY_3, KEY_4, KEY_5, KEY_6]
            .iter()
            .enumerate()
        {
            if self.input.key_pressed(*key) {
                self.state.selected = index;
            }
        }
        if self.input.scroll.abs() > 0.01 {
            let slots = config::HOTBAR_SLOTS as i32;
            let delta = if self.input.scroll > 0.0 { -1 } else { 1 };
            let next = (self.state.selected as i32 + delta).rem_euclid(slots);
            self.state.selected = next as usize;
        }
    }

    /// Acts on the tile the player is facing.
    ///
    /// The rules live in `noxel_valley::action`; this is the part that knows
    /// about the camera, the swing animation and the toast.
    fn use_tool(&mut self) {
        let tool = self.state.current_tool();
        let aim = self.player.aim_tile();
        self.player.start_swing();

        let selected = self
            .state
            .inventory
            .slots()
            .get(self.state.selected)
            .and_then(|slot| *slot)
            .map(|slot| slot.item);
        let outcome = action::use_tool(&mut self.map, &mut self.state, tool, selected, aim);
        if let Some(message) = outcome.message() {
            self.game_ui.toast(message);
        }
        // The quests are driven by what the player actually did, not by a
        // counter the caller has to remember to bump.
        match outcome {
            action::Outcome::Planted(_) => self.game_ui.quests.complete(screens::Quest::FirstSeed),
            action::Outcome::Harvested(_) => {
                self.game_ui.quests.complete(screens::Quest::FirstHarvest);
            }
            _ => {}
        }
        if let Some(quest) = self.game_ui.quests.take_completed() {
            self.game_ui.toast(format!("任务完成：{}", quest.title()));
        }
        if outcome.changed_something() {
            self.last_action = Some(format!("{outcome:?} at {aim:?}"));
        }
    }

    /// Opens whatever the player is standing in front of.
    ///
    /// Anything else says so and does nothing. The old version fell through to
    /// *sleep* for every tile without a shop or a bin on it, so pressing E in
    /// the middle of a field ended the day — which combined with the one-way
    /// sleep flag to make E a fast-forward button and the game unplayable.
    fn interact(&mut self) {
        match world::prompt_for(self.player.tile()) {
            Some(world::Prompt::Shop) => self.game_ui.screen = Screen::Shop,
            Some(world::Prompt::Bin) => self.game_ui.screen = Screen::Bin,
            Some(world::Prompt::Bed) => self.sleep_requested = true,
            // The villager walks, so she is found by distance rather than by
            // tile. She is also the tutorial, and a tutorial you have to stand
            // on one exact square to reach is a tutorial nobody reads.
            None if self.game_ui.villager_nearby => self.game_ui.open_dialogue(),
            None => self.game_ui.toast("这里没什么可做的"),
        }
    }

    /// Writes the save, reporting either way.
    ///
    /// Silently succeeding at a save is as bad as silently failing at one: the
    /// player has no way to know whether it worked.
    fn write_save(&mut self) {
        let text = save::to_text(&self.map, &self.state, self.seed);
        match save::write_file(&save::save_path(), &text) {
            Ok(()) => self.game_ui.toast("已保存"),
            Err(error) => self.game_ui.toast(format!("保存失败：{error}")),
        }
    }

    /// Throws the farm away and starts a new one.
    fn reset(&mut self, seed: u32) {
        self.map = world::build_farm();
        self.player = Player::at(world::START_TILE);
        self.state = sim::GameState::new(seed);
        self.last_report = None;
        self.sleep_requested = false;
        // A revision no map can have, so the meshes are rebuilt from scratch.
        self.built_revision = u64::MAX;
        self.seed = seed;
    }

    /// Ends the day and shows what it earned.
    fn end_day(&mut self, exhausted: bool) {
        let report = self.state.sleep(&mut self.map);
        self.game_ui.show_summary();
        self.last_report = Some(report);
        if exhausted {
            self.game_ui.toast("你累倒在田里，被人抬回了床上");
        } else {
            self.game_ui
                .toast(format!("第 {} 天开始了", self.state.clock.day_of_season()));
        }
    }

    /// Draws the interface.
    fn draw(&mut self, app: &App, framebuffer: &mut Framebuffer, assets: &Assets) {
        let input = self.input.clone();
        // Computed here because this is the only place that has the farm, the
        // player and the bag at once — and it is a *sentence*, not a widget.
        self.game_ui.hint = screens::hint(&self.state, &self.map, &self.player);
        self.game_ui.begin(app.config.fixed_dt, &input);
        // The action is applied here rather than in `update`, because this is
        // where the widgets run and the answer is only known now.
        let action = {
            let state = &self.state;
            let player = &self.player;
            self.game_ui
                .draw(framebuffer, &input, state, player, assets, save::has_save())
        };
        self.apply(action);
        // The cursor goes on last, over the world and under nothing.
        //
        // It projects through the camera's *snapped* focus rather than the
        // player's position. The camera smooths towards the player and snaps to
        // whole sixteenths, so the two differ by up to half a tile while
        // walking — which is exactly the offset between the highlight and the
        // tile the tool acts on.
        let focus = app.camera().snapped_focus();
        self.game_ui
            .draw_aim_highlight(framebuffer, &self.state, &self.player, focus);
        self.game_ui
            .draw_held_item(framebuffer, &self.state, &self.player, assets, focus);
        self.game_ui.end();
    }

    fn apply(&mut self, action: UiAction) {
        match action {
            UiAction::None => {}
            UiAction::SelectSlot(index) => self.state.selected = index,
            UiAction::BuySeed(crop, quantity) => {
                let cost = crop.seed_price * quantity;
                if self.state.inventory.free_slots() == 0 {
                    self.game_ui.toast("背包满了");
                    return;
                }
                if self.state.inventory.spend(cost) {
                    let leftover = self.state.inventory.add(Item::Seed(crop), quantity);
                    if leftover > 0 {
                        // Hand back what did not fit rather than deleting it.
                        self.state.inventory.earn(crop.seed_price * leftover);
                        self.game_ui.toast("背包满了，部分已退款");
                    } else {
                        self.game_ui
                            .toast(format!("买了 {} 个{}种子", quantity, crop.name));
                        self.game_ui.quests.complete(screens::Quest::FirstSeed);
                        // Hold it. A seed in a bag the hotbar cannot show is a
                        // seed the player cannot plant, which is what "I bought
                        // seeds and cannot farm" turned out to mean.
                        if let Some(slot) = self.state.inventory.find(Item::Seed(crop)) {
                            self.state.selected = slot;
                        }
                    }
                } else {
                    self.game_ui.toast("金币不够");
                }
            }
            UiAction::ShipAll => {
                let mut moved = 0;
                for index in 0..self.state.inventory.slots().len() {
                    let Some(slot) = self.state.inventory.slots()[index] else {
                        continue;
                    };
                    if !matches!(slot.item, Item::Produce(_)) {
                        continue;
                    }
                    let count = slot.count;
                    if self.state.inventory.remove(slot.item, count) == count {
                        self.state.bin.add(slot.item, count);
                        moved += count;
                    }
                }
                if moved > 0 {
                    let value = self.state.bin.value();
                    self.game_ui
                        .toast(format!("放入 {moved} 件，价值 {value} 金"));
                } else {
                    self.game_ui.toast("没有可以出货的作物");
                }
            }
            UiAction::Close => self.game_ui.screen = Screen::Playing,
            UiAction::Sleep => self.sleep_requested = true,
            UiAction::NewGame => {
                // A new farm replaces everything, so it is also the load path's
                // opposite: discard the save rather than leave it to be
                // "continued" into a game that no longer matches it.
                let _ = std::fs::remove_file(save::save_path());
                let seed = self.seed;
                self.reset(seed);
                self.game_ui.screen = Screen::Playing;
                self.game_ui
                    .toast("新的农场。走到薇拉旁边按 E 可以问怎么玩。");
            }
            UiAction::Continue => match save::read_file(&save::save_path()) {
                Ok(Some(text)) => match save::parse(&text) {
                    Ok(loaded) => {
                        save::apply(&loaded, &mut self.map, &mut self.state);
                        self.built_revision = u64::MAX;
                        self.game_ui.screen = Screen::Playing;
                        self.game_ui.toast("读档完成");
                    }
                    Err(error) => self.game_ui.toast(format!("存档读不了：{error}")),
                },
                Ok(None) => self.game_ui.toast("没有找到存档"),
                Err(error) => self.game_ui.toast(format!("存档读不了：{error}")),
            },
            UiAction::Settings => self.game_ui.screen = Screen::Settings,
            UiAction::Quests => self.game_ui.screen = Screen::Quests,
            UiAction::Resume => self.game_ui.screen = Screen::Playing,
            UiAction::Title => {
                self.write_save();
                self.game_ui.screen = Screen::Title;
            }
            UiAction::Save => self.write_save(),
            UiAction::SaveSettings => {
                let _ = save::write_file(&save::settings_path(), &self.game_ui.settings.to_text());
            }
            UiAction::Quit => self.quit = true,
            UiAction::Advance => {
                if self.game_ui.dialogue.advance() {
                    self.game_ui.screen = Screen::Playing;
                }
            }
            UiAction::DialogueDone => self.game_ui.screen = Screen::Playing,
        }
    }

    /// Plants one row per crop, each spread across the growth stages.
    ///
    /// A development affordance, and an honest one: the crop atlas has thirty
    /// sprites and no way to see them in the game until a season has passed.
    /// This is also how the crop art was checked while it was being drawn.
    fn plant_demo_field(&mut self) {
        // Iterating the `const` directly, rather than through a local binding:
        // the constant is promoted to `'static`, where a `let` would drop it at
        // the end of the statement and the borrow with it.
        for (row, crop) in config::CROPS.iter().enumerate() {
            for stage in 0..5u32 {
                // In the open field, which the scatter never plants on because
                // it only places props on grass.
                let x = 21 + (stage as i32 * 2);
                let y = 17 + row as i32;
                let Some(tile) = self.map.get_mut(x, y) else {
                    continue;
                };
                tile.ground = world::Ground::Tilled;
                tile.watered = stage % 2 == 0;
                let mut plant = world::Plant::new(crop);
                plant.days = stage * crop.growth_days / 4;
                plant.watered = tile.watered;
                tile.plant = Some(plant);
            }
        }
        // And stand the player where the camera will frame it, so `--demo` shows
        // the thirty crop sprites rather than a patch of field off-screen.
        self.player = Player::at((24, 19));
    }

    /// A one-line status for the window title and the log.
    fn status(&self) -> String {
        format!(
            "{} {} | {} | {} 金 | 体力 {:.0}",
            self.state.clock.season().name(),
            self.state.clock.day_of_season(),
            self.state.clock.time_string(),
            self.state.inventory.gold,
            self.state.energy
        )
    }
}

fn texture_handle(
    app: &mut App,
    assets: &Assets,
    which: &str,
) -> noxel_render::material::TextureHandle {
    let atlas = match which {
        "terrain" => assets.atlases.terrain.as_ref(),
        "crops" => assets.atlases.crops.as_ref(),
        "characters" => assets.atlases.characters.as_ref(),
        _ => assets.atlases.props.as_ref(),
    };
    match atlas {
        Some(atlas) => app.scene_mut().add_texture(atlas.texture().clone()),
        // A one-pixel white texture, so a missing atlas renders as flat colour
        // rather than failing to build.
        None => {
            let image = noxel_asset::image::Image::new(1, 1, Color8::WHITE);
            app.scene_mut()
                .add_texture(noxel_asset::texture::Texture::from_image(image))
        }
    }
}

/// One other person on the farm, walking a fixed route.
///
/// Deliberately not a crowd. `noxel-npc` exists for that and carries a flow-field
/// solver, a steering stack and a schedule system — all of which a single
/// villager pacing the plaza does not need. What this does need is to make the
/// farm look inhabited, and eight lines of waypoint following achieves that.
struct Villager {
    /// Waypoints in tiles, walked in order and then looped.
    route: [(f32, f32); 4],
    /// Which waypoint is being walked towards.
    target: usize,
    /// Current position in tiles.
    position: (f32, f32),
    /// Facing, for the sprite.
    facing: noxel_valley::player::Facing,
    /// Seconds spent walking, for the animation phase.
    phase: f32,
    /// Whether they are moving.
    moving: bool,
}

impl Default for Villager {
    fn default() -> Self {
        Self {
            // A short loop along the plaza in front of the farmhouse.
            route: [(13.5, 9.5), (24.5, 9.5), (24.5, 11.5), (13.5, 11.5)],
            target: 1,
            position: (13.5, 9.5),
            facing: noxel_valley::player::Facing::Right,
            phase: 0.0,
            moving: true,
        }
    }
}

impl Villager {
    /// Steps along the route.
    fn update(&mut self, dt: f32, map: &FarmMap) {
        const SPEED: f32 = 2.2;
        let goal = self.route[self.target];
        let dx = goal.0 - self.position.0;
        let dy = goal.1 - self.position.1;
        let distance = (dx * dx + dy * dy).sqrt();
        if distance < 0.05 {
            self.target = (self.target + 1) % self.route.len();
            return;
        }
        let step = (SPEED * dt).min(distance);
        let (nx, ny) = (dx / distance, dy / distance);
        self.facing = noxel_valley::player::Facing::from_vector(nx, ny);
        let next = (self.position.0 + nx * step, self.position.1 + ny * step);
        // If the route is blocked by scenery the villager simply waits rather
        // than pathing around it; the route is authored through open ground.
        if map.is_walkable(next.0.floor() as i32, next.1.floor() as i32) {
            self.position = next;
            self.moving = true;
            self.phase += dt;
        } else {
            self.moving = false;
        }
    }

    /// The atlas region for the current pose.
    fn sprite(&self) -> String {
        let frame = if self.moving {
            [1u32, 2, 3, 2][(self.phase * 5.0) as usize % 4]
        } else {
            0
        };
        format!("villager_{}_{}", self.facing.key(), frame)
    }
}

/// A camera that maps one world unit to exactly one tile of pixels.
fn setup_camera(app: &mut App) {
    let camera = app.camera_mut();
    camera.set_projection(noxel_camera::ProjectionMode::Orthographic {
        height: ORTHO_HEIGHT,
    });
    // Straight down. A tilted camera would foreshorten the ground and resample
    // every upright sprite by `cos(pitch)`, which is exactly the softness the
    // art is drawn to avoid.
    camera.set_pitch(core::f32::consts::FRAC_PI_2);
    camera.set_yaw_immediate(0.0);
    camera.lock_rotation();
    // Near-instant. The camera follows a player who is already moving smoothly,
    // so lag buys nothing and costs the feeling of control: a camera that eases
    // towards where the player *was* reads as the ground sliding under them.
    // What smooths the motion is the pixel snap, not the easing.
    camera.set_smoothing(0.0);
    camera.set_deadzone(0.0, 0.0, 0.0);
    camera.set_look_ahead(0.0, 0.0);
    camera.set_pixel_perfect(noxel_camera::PixelPerfect::at(
        INTERNAL_WIDTH,
        INTERNAL_HEIGHT,
    ));
    camera.set_clip_planes(0.05, 400.0);
    camera.set_bounds(Some(noxel_core::math::Aabb::new(
        Vec3::new(0.0, -1.0, 0.0),
        Vec3::new(FARM_WIDTH as f32, 1.0, FARM_HEIGHT as f32),
    )));
}

/// The movement axis from WASD and the arrow keys.
fn movement_axis(input: &UiInput) -> (f32, f32) {
    let mut x = 0.0;
    let mut y = 0.0;
    for key in input.keys_held.iter().copied() {
        match key {
            KEY_W | KEY_UP => y -= 1.0,
            KEY_S | KEY_DOWN => y += 1.0,
            KEY_A | KEY_LEFT => x -= 1.0,
            KEY_D | KEY_RIGHT => x += 1.0,
            _ => {}
        }
    }
    (x, y)
}

/// A font for the case where the bake on disk is missing.
///
/// A blank font draws no text at all. That is worse than a fallback face and
/// better than a panic: the game still runs, the layout is correct, and the
/// startup banner says exactly which command regenerates the bake. A silent
/// blank UI with no explanation is the failure this avoids.
fn default_font() -> noxel_ui::FontSet {
    noxel_ui::FontSet::blank()
}

// ---------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------

const KEY_ESCAPE: u32 = 27;
const KEY_SPACE: u32 = 32;
const KEY_TAB: u32 = 9;
const KEY_1: u32 = 49;
const KEY_2: u32 = 50;
const KEY_3: u32 = 51;
const KEY_4: u32 = 52;
const KEY_5: u32 = 53;
const KEY_6: u32 = 54;
const KEY_E: u32 = 69;
const KEY_H: u32 = 72;
const KEY_SLASH: u32 = 47;
const KEY_W: u32 = 87;
const KEY_A: u32 = 65;
const KEY_S: u32 = 83;
const KEY_D: u32 = 68;
const KEY_UP: u32 = 38;
const KEY_DOWN: u32 = 40;
const KEY_LEFT: u32 = 37;
const KEY_RIGHT: u32 = 39;

// ---------------------------------------------------------------------------
// The plugin
// ---------------------------------------------------------------------------

/// Adapter that lets the engine drive the game.
///
/// The state lives behind an `Rc<RefCell<_>>` so the host that owns the window
/// can read it for the title and the end-of-run report while the engine holds
/// the plugin. The alternative — the plugin owning the state outright — makes
/// every read from outside a downcast.
struct ValleyPlugin {
    valley: Rc<RefCell<Valley>>,
    assets: Rc<Assets>,
}

impl Plugin for ValleyPlugin {
    fn name(&self) -> &str {
        "noxel-valley"
    }

    fn update(&mut self, app: &mut App, dt: f32) {
        let mut valley = self.valley.borrow_mut();
        valley.update(app, dt, &self.assets);
    }

    fn draw(&mut self, app: &mut App, framebuffer: &mut Framebuffer) {
        let mut valley = self.valley.borrow_mut();
        valley.draw(app, framebuffer, &self.assets);
    }

    fn shutdown(&mut self, app: &mut App) {
        let valley = self.valley.borrow();
        app.debug_mut()
            .record_counter("gold", valley.state.inventory.gold as f32);
        app.debug_mut()
            .record_counter("crops", valley.map.ripe_count() as f32);
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// A parsed command line.
#[derive(Clone, Debug)]
struct Args {
    window: bool,
    frames: u64,
    seed: u64,
    fast: bool,
    stats: bool,
    dump: Option<std::path::PathBuf>,
    assets: Option<std::path::PathBuf>,
    /// Open this screen on the first frame, for screenshots and for the
    /// documentation's figures.
    screen: Option<Screen>,
    /// Plant a demonstration field, so the crop art can be seen without playing
    /// a season first.
    demo: bool,
    help: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            window: false,
            frames: 0,
            seed: 0x5EED,
            fast: false,
            stats: false,
            dump: None,
            assets: None,
            screen: None,
            demo: false,
            help: false,
        }
    }
}

const USAGE: &str = "\
noxel-valley — a playable top-down farming game

USAGE:
    noxel-valley [--window] [--frames N] [--seed N] [--fast] [--stats] [--dump DIR]

OPTIONS:
    --window       Play in a window. Needs a build with `--features window`.
                   A build with window support and no other flag opens a window
                   anyway, so double-clicking the game just plays it.
    --frames N     Run N frames and stop. 0 (the default) runs forever.
    --seed N       World and weather seed (decimal, or 0x-prefixed).
    --fast         Run the clock 24x, so a season passes in minutes.
    --stats        Print one frame's statistics and exit.
    --dump DIR     Write each frame as a PNG into DIR.
    --assets DIR   Asset root. Found automatically by default.
    --screen NAME  Open a screen at startup: inventory, shop, bin, summary.
    --demo         Plant a demonstration field at several growth stages.
    -h, --help     Print this text.

CONTROLS:
    WASD / arrows   walk                 Shift   run
    Space / LMB     use the held tool    1-6     select a hotbar slot
    Tab             bag                  E       shop, shipping bin, or sleep
    ?               controls             Esc     close / quit
";

impl Args {
    /// Whether to open a window rather than render to PNGs.
    ///
    /// `--window` asks for one explicitly. Beyond that, a build with window
    /// support that was given no output flag is someone who double-clicked the
    /// game, so it plays: `./noxel-valley` opens a window, and
    /// `./noxel-valley --frames 300 --dump frames` still renders headlessly.
    /// Anything else makes "just run it" do something other than run it.
    fn wants_window(&self) -> bool {
        if self.window {
            return true;
        }
        if !cfg!(feature = "window") {
            return false;
        }
        self.frames == 0 && !self.stats && self.dump.is_none() && self.screen.is_none()
    }

    fn parse(args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut parsed = Self::default();
        let mut args = args.peekable();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--window" => parsed.window = true,
                "--fast" => parsed.fast = true,
                "--stats" => parsed.stats = true,
                "-h" | "--help" => parsed.help = true,
                "--frames" => {
                    let value = args.next().ok_or("--frames needs a number")?;
                    parsed.frames = value
                        .parse()
                        .map_err(|_| format!("bad --frames: {value}"))?;
                }
                "--seed" => {
                    let value = args.next().ok_or("--seed needs a number")?;
                    parsed.seed =
                        parse_number(&value).map_err(|_| format!("bad --seed: {value}"))?;
                }
                "--dump" => {
                    let value = args.next().ok_or("--dump needs a directory")?;
                    parsed.dump = Some(value.into());
                }
                "--assets" => {
                    let value = args.next().ok_or("--assets needs a directory")?;
                    parsed.assets = Some(value.into());
                }
                "--demo" => parsed.demo = true,
                "--screen" => {
                    let value = args.next().ok_or("--screen needs a name")?;
                    parsed.screen = Some(match value.as_str() {
                        "playing" | "world" => Screen::Playing,
                        "inventory" | "bag" => Screen::Inventory,
                        "shop" => Screen::Shop,
                        "bin" | "shipping" => Screen::Bin,
                        "summary" => Screen::Summary,
                        "pause" => Screen::Pause,
                        "settings" => Screen::Settings,
                        "quests" | "questbook" | "quest" => Screen::Quests,
                        other => return Err(format!("unknown screen {other:?}")),
                    });
                }
                other => return Err(format!("unknown argument {other:?}")),
            }
        }
        Ok(parsed)
    }
}

fn parse_number(text: &str) -> Result<u64, core::num::ParseIntError> {
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => text.parse(),
    }
}

fn main() {
    let args = match Args::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("noxel-valley: {message}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    if args.help {
        print!("{USAGE}");
        return;
    }
    if let Err(message) = run(&args) {
        eprintln!("noxel-valley: {message}");
        std::process::exit(1);
    }
}

fn build_app(args: &Args, assets: &Assets) -> Result<App, String> {
    let mut config = AppConfig::headless();
    config.seed = args.seed;
    config.internal = (INTERNAL_WIDTH, INTERNAL_HEIGHT);
    config.mode = ShadingMode::Raster;
    config.assets_root = assets.root.clone().unwrap_or_default();
    // A UI drawn every frame into a framebuffer that also has a debug overlay is
    // two things writing the same pixels; every debug panel stays off.
    config.debug = noxel_app::DebugConfig::disabled();
    if let Some(directory) = &args.dump {
        config.dump = Some((directory.clone(), noxel_debug::dump::DumpFormat::Png));
    }
    App::new(config).map_err(|error| error.to_string())
}

fn run(args: &Args) -> Result<(), String> {
    let root = args.assets.clone().or_else(assets::find_root);
    let assets = Rc::new(Assets::load(root));

    match &assets.root {
        Some(path) => println!("noxel-valley: assets from {}", path.display()),
        None => println!("noxel-valley: no assets found; running with flat colours"),
    }
    if !assets.has_art() {
        println!("  (run `noxel-gen farm --out games/noxel-valley/assets` to generate the art)");
    }
    if !assets.has_font() {
        println!(
            "  (run `tools/fontgen/fontgen.py --out games/noxel-valley/assets/fonts` for the font)"
        );
    }

    let mut app = build_app(args, &assets)?;
    let valley = Rc::new(RefCell::new(Valley::new(
        &mut app,
        args.seed as u32,
        &assets,
        args.fast,
    )));
    // `--screen` exists so a screenshot of the shop does not need someone to
    // walk to the shop first — for the documentation's figures and for a
    // regression check on each overlay.
    if let Some(screen) = args.screen {
        valley.borrow_mut().game_ui.screen = screen;
    }
    if args.demo {
        valley.borrow_mut().plant_demo_field();
    }
    app.add_plugin(ValleyPlugin {
        valley: Rc::clone(&valley),
        assets: Rc::clone(&assets),
    });

    // Warm up so the first presented frame is a farm rather than an empty scene.
    for _ in 0..2 {
        app.step(1.0 / 60.0);
    }

    if args.stats {
        let report = app.run_and_capture(1);
        println!("{}", report.summary());
        println!("{}", valley.borrow().status());
        return Ok(());
    }

    if args.wants_window() {
        return run_window(args, app, valley);
    }

    run_headless(args, app, valley)
}

/// Renders a fixed number of frames and writes each one as a PNG.
fn run_headless(args: &Args, mut app: App, valley: Rc<RefCell<Valley>>) -> Result<(), String> {
    let frames = if args.frames == 0 { 600 } else { args.frames };
    let started = std::time::Instant::now();
    for _frame in 0..frames {
        // Drive the clock from the frame index so a headless run is exactly
        // reproducible: the same command produces the same PNGs on any machine.
        let dt = 1.0 / 60.0;
        app.step(dt);
        // Every frame is written when `--dump` is on: this is the mode a golden
        // image test drives, and a subset would make "which frame is this" a
        // question the file name already answers.
        if args.dump.is_some() {
            let _ = app.dump_current_frame();
        }
    }
    let elapsed = started.elapsed().as_secs_f32();
    let stats = app.debug().stats();
    println!(
        "noxel-valley: {frames} frames in {elapsed:.2}s ({:.1} fps), mean {:.2} ms, p95 {:.2} ms",
        frames as f32 / elapsed.max(1e-6),
        stats.frame_times.mean(),
        stats.frame_times.percentile(0.95)
    );
    println!("{}", valley.borrow().status());
    Ok(())
}

/// Opens a window and plays.
#[cfg(feature = "window")]
fn run_window(args: &Args, app: App, valley: Rc<RefCell<Valley>>) -> Result<(), String> {
    struct Game {
        app: App,
        valley: Rc<RefCell<Valley>>,
        input: UiInput,
        frames: u64,
        limit: u64,
        /// The soundtrack, or `None` on a machine with no audio device.
        #[cfg(feature = "audio")]
        audio: Option<noxel_audio::AudioHandle>,
    }

    impl noxel_window::Host for Game {
        fn step(&mut self, dt: f32, input: &noxel_window::Input) -> &Framebuffer {
            self.input = ui_input(input);
            self.input.keys_pressed = input.pressed().to_vec();

            self.valley.borrow_mut().input = self.input.clone();
            self.app.step(dt);
            self.frames += 1;

            // The soundtrack follows the world, not the other way round. The
            // director reads the clock and the weather after the frame has run,
            // so the music changes on the same frame the sky does.
            #[cfg(feature = "audio")]
            if let Some(audio) = &self.audio {
                let valley = self.valley.borrow();
                let clock = &valley.state.clock;
                let mood =
                    screens::mood_for(clock.season(), valley.state.weather.today(), clock.hour());
                audio.set_mood(mood_named(mood));
                audio.set_volume(valley.game_ui.settings.volume);
                audio.set_muted(!valley.game_ui.settings.music);
            }
            self.app.framebuffer()
        }

        fn internal_size(&self) -> (u32, u32) {
            (INTERNAL_WIDTH, INTERNAL_HEIGHT)
        }

        fn should_quit(&self) -> bool {
            // The player can also leave from the pause menu, which is the only
            // place that ends the session on purpose.
            if self.valley.borrow().quit {
                return true;
            }
            self.limit > 0 && self.frames >= self.limit
        }

        fn title_suffix(&self) -> String {
            self.valley.borrow().status()
        }
    }

    let config = noxel_window::WindowConfig::default()
        .with_title("星野农场 · Noxel Valley")
        .with_internal(INTERNAL_WIDTH, INTERNAL_HEIGHT)
        // Escape opens the pause menu. A window host that closed on it would
        // take the key before the game ever saw it, which is what made holding
        // Escape quit the session.
        .keep_escape();
    // The soundtrack starts on the season and weather the farm opens on, and
    // is never started at all if the machine has no device.
    #[cfg(feature = "audio")]
    let audio = {
        let valley = valley.borrow();
        let clock = &valley.state.clock;
        let mood = screens::mood_for(clock.season(), valley.state.weather.today(), clock.hour());
        let handle = start_music(valley.state.clock.year(), mood_named(mood));
        if let Some(handle) = &handle {
            handle.set_volume(valley.game_ui.settings.volume);
            handle.set_muted(!valley.game_ui.settings.music);
        }
        handle
    };

    let game = Game {
        app,
        valley: Rc::clone(&valley),
        input: UiInput::new(),
        frames: 0,
        limit: args.frames,
        #[cfg(feature = "audio")]
        audio,
    };
    noxel_window::run(config, game).map_err(|error| error.to_string())
}

/// Converts the window host's input into what the interface reads.
///
/// One conversion in one place, and extracted so it can be tested. It used to
/// be eight lines inside the frame loop, which meant the only thing standing
/// between a physical click and a button was the one piece of code no test
/// could reach — and when every button in the game stopped responding, that was
/// exactly where to look and nowhere to look *with*.
///
/// The host has already mapped the cursor into framebuffer pixels using the
/// presentation the upscale uses, so a hit test here is in the space the
/// interface draws in.
#[cfg(feature = "window")]
fn ui_input(input: &noxel_window::Input) -> noxel_ui::UiInput {
    let mut ui = noxel_ui::UiInputBuilder::new()
        .at(input.cursor.0, input.cursor.1)
        .build();
    ui.pointer_inside = input.cursor_inside;
    ui.primary_down = input.mouse_buttons[0];
    ui.primary_pressed = input.mouse_pressed[0];
    ui.primary_released = input.mouse_released[0];
    ui.secondary_pressed = input.mouse_pressed[2];
    ui.scroll = input.scroll;
    ui.shift = input.shift;
    ui.control = input.control;
    ui.alt = input.alt;
    ui.keys_held = input.held().to_vec();
    ui
}

/// Starts the soundtrack, if this build has one and the machine has a device.
///
/// A machine with no sound card should still be able to play the game, so a
/// failure here returns `None` and the game carries on in silence rather than
/// refusing to start.
#[cfg(feature = "audio")]
fn start_music(seed: u32, mood: noxel_audio::music::Mood) -> Option<noxel_audio::AudioHandle> {
    match noxel_audio::output::start(seed, mood) {
        Ok(handle) => Some(handle),
        Err(error) => {
            eprintln!("noxel-valley: no music ({error})");
            None
        }
    }
}

/// The engine's mood for a name from [`screens::mood_for`].
///
/// The mapping lives here rather than in `screens.rs` so that module stays
/// free of the audio engine: the weather-to-mood decision is a game rule and
/// worth testing, and it should not need a sound card to be testable.
#[cfg(feature = "audio")]
fn mood_named(name: &str) -> noxel_audio::music::Mood {
    use noxel_audio::music::moods;
    match name {
        "summer" => moods::SUMMER,
        "fall" => moods::FALL,
        "winter" => moods::WINTER,
        _ => moods::SPRING,
    }
}

#[cfg(not(feature = "window"))]
fn run_window(_args: &Args, _app: App, _valley: Rc<RefCell<Valley>>) -> Result<(), String> {
    Err("this build has no window support; rebuild with `--features window`".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use noxel_valley::config::TILE;

    #[test]
    fn the_seed_parser_accepts_decimal_and_hexadecimal() {
        assert_eq!(parse_number("1234").unwrap(), 1234);
        assert_eq!(parse_number("0x10").unwrap(), 16);
        assert_eq!(parse_number("0XFF").unwrap(), 255);
        assert!(parse_number("nonsense").is_err());
    }

    #[test]
    fn the_argument_parser_reads_every_flag() {
        let args = Args::parse(
            [
                "--window", "--frames", "30", "--seed", "0xAB", "--fast", "--stats",
            ]
            .iter()
            .map(|s| s.to_string()),
        )
        .unwrap();
        assert!(args.window);
        assert_eq!(args.frames, 30);
        assert_eq!(args.seed, 0xAB);
        assert!(args.fast);
        assert!(args.stats);
    }

    /// Drives one real frame of the real game with real window input.
    ///
    /// Everything below this line is the path a physical click takes: the
    /// window host's `Input`, the conversion into what the interface reads, the
    /// interface's own state machine, and the action the game applies. Every
    /// other UI test in this repository starts after the conversion, which is
    /// how a bug that stopped *every button in the game* from responding stayed
    /// invisible to a green test suite.
    #[cfg(feature = "window")]
    fn frame(valley: &Rc<RefCell<Valley>>, app: &mut App, input: &noxel_window::Input) {
        valley.borrow_mut().input = ui_input(input);
        valley.borrow_mut().input.keys_pressed = input.pressed().to_vec();
        app.step(1.0 / 60.0);
    }

    #[cfg(feature = "window")]
    fn window_input(cursor: (f32, f32)) -> noxel_window::Input {
        let mut input = noxel_window::Input::default();
        input.cursor = cursor;
        // What the host sets from `CursorMoved`. A click with this false is a
        // click the interface is not allowed to see.
        input.cursor_inside = true;
        input
    }

    #[test]
    #[cfg(feature = "window")]
    fn a_real_click_on_a_real_button_reaches_the_game() {
        let assets = Rc::new(Assets::empty());
        let mut app = build_app(&Args::default(), &assets).expect("the app must build");
        let valley = Rc::new(RefCell::new(Valley::new(&mut app, 1, &assets, false)));
        // The plugin is what makes the frame loop call the game's `draw`, and
        // `draw` is where every widget runs. A test that builds the app without
        // it exercises nothing at all — which is how this test passed its first
        // draft while proving nothing.
        app.add_plugin(ValleyPlugin {
            valley: Rc::clone(&valley),
            assets: Rc::clone(&assets),
        });
        assert_eq!(valley.borrow().game_ui.screen, Screen::Title);

        // Find a row that starts a new farm, by probing down the panel.
        let mut pressed_at = None;
        for y in (60..190).step_by(2) {
            {
                let mut v = valley.borrow_mut();
                v.game_ui.screen = Screen::Title;
                let mut down = window_input((240.0, y as f32));
                down.mouse_buttons[0] = true;
                down.mouse_pressed[0] = true;
                v.input = ui_input(&down);
                v.input.keys_pressed.clear();
            }
            app.step(1.0 / 60.0);
            let mut up = window_input((240.0, y as f32));
            up.mouse_released[0] = true;
            frame(&valley, &mut app, &up);
            if y == 124 || y == 126 {
                let v = valley.borrow();
                eprintln!(
                    "DEBUG y={y} screen={:?} modal={} pointer_over={} inside={} cursor={:?}",
                    v.game_ui.screen,
                    v.game_ui.ui.state.is_modal(),
                    v.game_ui.ui.state.pointer_over_ui(),
                    v.input.pointer_inside,
                    (v.input.pointer.0, v.input.pointer.1)
                );
            }
            if valley.borrow().game_ui.screen != Screen::Title {
                pressed_at = Some(y);
                break;
            }
            // Reset the interface's capture between probes.
            frame(&valley, &mut app, &window_input((240.0, y as f32)));
        }
        assert!(
            pressed_at.is_some(),
            "no row of the start menu answered a real click"
        );
        assert_eq!(
            valley.borrow().game_ui.screen,
            Screen::Playing,
            "the button was pressed but the game did not act on it"
        );
    }

    #[test]
    #[cfg(feature = "window")]
    fn a_real_click_buys_a_seed_in_the_shop() {
        // The second screen, because "none of the buttons work" deserves more
        // than one screen's worth of evidence.
        let assets = Rc::new(Assets::empty());
        let mut app = build_app(&Args::default(), &assets).expect("the app must build");
        let valley = Rc::new(RefCell::new(Valley::new(&mut app, 1, &assets, false)));
        app.add_plugin(ValleyPlugin {
            valley: Rc::clone(&valley),
            assets: Rc::clone(&assets),
        });
        valley.borrow_mut().game_ui.screen = Screen::Shop;

        let gold_before = valley.borrow().state.inventory.gold();
        let seeds_before: u32 = noxel_valley::config::CROPS
            .iter()
            .map(|c| {
                valley
                    .borrow()
                    .state
                    .inventory
                    .count_of(noxel_valley::sim::Item::Seed(c))
            })
            .sum();

        // Sweep the whole frame for a row whose "buy" button works.
        let mut bought = false;
        'outer: for y in (60..260).step_by(3) {
            for x in (150..470).step_by(6) {
                {
                    let mut v = valley.borrow_mut();
                    v.game_ui.screen = Screen::Shop;
                    let mut down = window_input((x as f32, y as f32));
                    down.mouse_buttons[0] = true;
                    down.mouse_pressed[0] = true;
                    v.input = ui_input(&down);
                    v.input.keys_pressed.clear();
                }
                app.step(1.0 / 60.0);
                let mut up = window_input((x as f32, y as f32));
                up.mouse_released[0] = true;
                frame(&valley, &mut app, &up);
                if valley.borrow().state.inventory.gold() != gold_before {
                    bought = true;
                    break 'outer;
                }
                frame(&valley, &mut app, &window_input((x as f32, y as f32)));
            }
        }

        let seeds_after: u32 = noxel_valley::config::CROPS
            .iter()
            .map(|c| {
                valley
                    .borrow()
                    .state
                    .inventory
                    .count_of(noxel_valley::sim::Item::Seed(c))
            })
            .sum();
        assert!(
            bought && seeds_after > seeds_before,
            "the shop's buy button did nothing"
        );
    }

    #[test]
    #[cfg(feature = "window")]
    fn a_click_is_not_seen_when_the_cursor_is_outside_the_window() {
        // `Input::cursor_inside` is false before the first mouse move and after
        // the cursor leaves. The interface must not act on a click it cannot
        // place, which is the difference between "no button works" and "no
        // button works until you move the mouse".
        let mut input = window_input((240.0, 150.0));
        let inside = ui_input(&input);
        assert!(
            inside.pointer_inside,
            "a cursor in the window must be usable"
        );
        input.cursor_inside = false;
        assert!(!ui_input(&input).pointer_inside);
        assert!(
            ui_input(&window_input((240.0, 150.0)))
                .pointer_over(noxel_ui::UiRect::new(0, 0, 480, 270)),
            "a cursor inside the window must be able to hover something"
        );
    }

    #[test]
    #[cfg(feature = "window")]
    fn every_mouse_button_the_game_uses_survives_the_conversion() {
        // A transposed index here is a click that does nothing, with no error
        // anywhere: the interface simply never sees a press.
        let mut input = window_input((10.0, 10.0));
        input.mouse_buttons[0] = true;
        input.mouse_pressed[0] = true;
        input.mouse_released[0] = true;
        input.mouse_pressed[2] = true;
        input.shift = true;
        let converted = ui_input(&input);
        assert!(
            converted.primary_down,
            "the left button's level was dropped"
        );
        assert!(converted.primary_pressed, "the left press edge was dropped");
        assert!(
            converted.primary_released,
            "the left release edge was dropped"
        );
        assert!(
            converted.secondary_pressed,
            "the right press edge was dropped"
        );
        assert!(converted.shift, "shift was dropped");
        assert!(converted.pointer_inside);
    }

    #[test]
    fn an_output_flag_means_headless_and_nothing_means_play() {
        // The rule that makes a double-clicked bundle play the game instead of
        // rendering 600 PNGs into a directory nobody will look at.
        if !cfg!(feature = "window") {
            // Without the feature there is no window to open, and every run is
            // headless regardless.
            assert!(!Args::default().wants_window());
            return;
        }
        assert!(
            Args::default().wants_window(),
            "no arguments should play the game"
        );
        assert!(
            Args {
                window: true,
                ..Args::default()
            }
            .wants_window()
        );

        let headless = [
            Args {
                frames: 300,
                ..Args::default()
            },
            Args {
                stats: true,
                ..Args::default()
            },
            Args {
                dump: Some(std::path::PathBuf::from("frames")),
                ..Args::default()
            },
            Args {
                screen: Some(Screen::Shop),
                ..Args::default()
            },
        ];
        for args in headless {
            assert!(!args.wants_window(), "{args:?} should render, not play");
        }
    }

    #[test]
    fn the_camera_sits_where_the_player_is_rather_than_easing_towards_it() {
        // A camera that eases towards where the player *was* reads as the ground
        // sliding under them, and combined with the pixel snap it moves in
        // irregular steps. The follow is exact; what smooths the motion is the
        // snap, not the easing.
        let assets = Rc::new(Assets::empty());
        let mut app = build_app(&Args::default(), &assets).expect("the app must build");
        let valley = Rc::new(RefCell::new(Valley::new(&mut app, 1, &assets, false)));
        app.add_plugin(ValleyPlugin {
            valley: Rc::clone(&valley),
            assets: Rc::clone(&assets),
        });
        valley.borrow_mut().game_ui.screen = Screen::Playing;

        let input = noxel_ui::UiInputBuilder::new().key(KEY_D).build();
        for _ in 0..120 {
            valley.borrow_mut().input = input.clone();
            app.step(1.0 / 60.0);
        }
        let player = valley.borrow().player.position;
        let focus = app.camera().snapped_focus();
        // One sixteenth of a unit is one pixel; the camera may be a pixel out
        // because of the snap and no more.
        assert!(
            (focus.x - player.x).abs() <= 1.0 / 16.0 + 1e-4,
            "the camera is {:.3} units behind the player",
            (focus.x - player.x).abs()
        );
        assert!((focus.z - player.y).abs() <= 1.0 / 16.0 + 1e-4);
    }

    #[test]
    fn a_toast_goes_away() {
        // It is on a timer, and nothing was winding it, so every message the
        // game ever showed stayed on screen for the rest of the session.
        let mut ui = GameUi::new(noxel_ui::Ui::flat(noxel_ui::testfont::test_font()));
        ui.toast("任务完成：第一粒种子");
        assert!(ui.toast_text().is_some(), "the toast should be showing");
        for _ in 0..200 {
            ui.tick(1.0 / 60.0);
        }
        assert!(ui.toast_text().is_none(), "the toast never went away");
    }

    #[test]
    fn the_screen_argument_names_every_overlay_and_rejects_the_rest() {
        for (name, expected) in [
            ("inventory", Screen::Inventory),
            ("shop", Screen::Shop),
            ("bin", Screen::Bin),
            ("summary", Screen::Summary),
        ] {
            let args = Args::parse(["--screen", name].iter().map(|s| s.to_string())).unwrap();
            assert_eq!(args.screen, Some(expected), "for {name}");
        }
        assert!(Args::parse(["--screen", "nonsense"].iter().map(|s| s.to_string())).is_err());
    }

    #[test]
    fn an_unknown_flag_is_an_error_rather_than_being_ignored() {
        // Silently ignoring an argument is how a run ends up doing something
        // other than what was asked for.
        assert!(Args::parse(["--nonsense"].iter().map(|s| s.to_string())).is_err());
        assert!(Args::parse(["--frames"].iter().map(|s| s.to_string())).is_err());
    }

    #[test]
    fn the_movement_axis_combines_wasd_and_the_arrows() {
        let mut input = noxel_ui::UiInputBuilder::new()
            .key(KEY_D)
            .key(KEY_UP)
            .build();
        assert_eq!(movement_axis(&input), (1.0, -1.0));
        input.keys_held = vec![KEY_A, KEY_D];
        assert_eq!(movement_axis(&input).0, 0.0, "opposite keys cancel");
        input.keys_held = vec![KEY_LEFT, KEY_RIGHT];
        assert_eq!(movement_axis(&input).0, 0.0);
    }

    #[test]
    fn the_camera_maps_one_world_unit_to_one_tile_of_pixels() {
        // The crispness contract, checked against the constant the camera is
        // actually configured with rather than against a literal.
        let pixels_per_unit = INTERNAL_HEIGHT as f32 / ORTHO_HEIGHT;
        assert!((pixels_per_unit - TILE as f32).abs() < 1e-4);
    }

    #[test]
    fn the_real_asset_tree_loads_and_the_farm_reaches_the_renderer() {
        // The test that would have caught "the world is empty": every unit test
        // ran against `Assets::empty()`, where an empty scene is correct. This
        // one loads what actually ships and asserts that the farm is in the
        // scene and survives culling.
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets");
        if !root.join("farm/terrain.png").exists() {
            // The art is generated, not committed to the source of a fresh
            // checkout of the engine; skip rather than fail there.
            eprintln!("skipping: run `noxel-gen farm --out games/noxel-valley/assets` first");
            return;
        }
        let assets = Assets::load(Some(root));
        assert!(assets.has_art(), "the farm atlases did not load");
        assert!(assets.atlases.terrain.is_some(), "terrain atlas missing");
        assert!(assets.atlases.crops.is_some(), "crop atlas missing");
        assert!(assets.atlases.props.is_some(), "prop atlas missing");
        assert!(
            assets.atlases.characters.is_some(),
            "character atlas missing"
        );

        let args = Args {
            seed: 3,
            ..Args::default()
        };
        let mut app = build_app(&args, &assets).expect("the app must build");
        let _valley = Valley::new(&mut app, 3, &assets, false);
        // Ground and props; the crop mesh is legitimately empty until something
        // is planted, which is why this is not `>= 3`.
        assert!(
            app.scene().instance_count() >= 2,
            "the farm meshes were not spawned: {} instances",
            app.scene().instance_count()
        );

        app.step(1.0 / 60.0);
        assert!(
            !app.context.visible.items.is_empty(),
            "every instance was culled, so the frame renders nothing"
        );
        // Culling accepting an instance is not the same as the rasterizer
        // drawing it: a mesh with the wrong winding passes every cull and then
        // has all of its triangles rejected as backfaces. Counting lit pixels is
        // the only check that covers both.
        let framebuffer = app.framebuffer();
        let lit = framebuffer
            .color_slice()
            .chunks_exact(3)
            .filter(|c| c[0] > 0.08)
            .count();
        assert!(
            lit > 10_000,
            "the farm passed culling but drew only {lit} of {} pixels",
            framebuffer.pixel_count()
        );
    }

    #[test]
    fn the_game_builds_and_draws_with_no_assets() {
        // The whole point of the flat fallback: `cargo test` on a fresh checkout
        // must be able to build a working game.
        let assets = Assets::empty();
        let args = Args {
            seed: 7,
            ..Args::default()
        };
        let mut app = build_app(&args, &assets).expect("the app must build");
        let valley = Valley::new(&mut app, 7, &assets, false);
        assert_eq!(valley.map.width(), FARM_WIDTH);
        assert_eq!(valley.map.height(), FARM_HEIGHT);
        assert!(valley.state.inventory.gold > 0);
    }
}
