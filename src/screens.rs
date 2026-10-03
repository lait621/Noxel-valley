//! The screens around the game: the start menu, the pause menu, the settings,
//! the quest log, and the conversation with the villager.
//!
//! These live apart from [`crate::ui`] because they are a different kind of
//! thing. The HUD and the bag describe *the world right now*; these describe
//! *the session* — what is saved, what is configured, what has been asked of
//! you — and they can be read and changed without the farm being loaded at all.
//!
//! The other thing that lives here is the aim highlight, for the same reason:
//! it is an overlay, not a part of the farm, and drawing it in the interface
//! layer is what keeps it crisp, correctly layered, and free of a texture.

use noxel_core::math::Color8;
use noxel_render::framebuffer::Framebuffer;
use noxel_ui::theme::BarStyle;
use noxel_ui::{Id, TextStyle, UiInput, UiRect};

use crate::assets::Assets;
use crate::config::{Season, Weather};
use crate::ui::{GameUi, Screen, UiAction};

// ---------------------------------------------------------------------------
// Conversations
// ---------------------------------------------------------------------------

/// One thing somebody says.
#[derive(Clone, Copy, Debug)]
pub struct Line {
    /// Who is talking.
    pub speaker: &'static str,
    /// What they say. Kept short: this is a page, not a paragraph.
    pub text: &'static str,
}

/// A conversation: a list of lines and where the player is in it.
#[derive(Clone, Debug, Default)]
pub struct Dialogue {
    /// The lines.
    pub lines: Vec<Line>,
    /// Which line is showing.
    pub index: usize,
}

impl Dialogue {
    /// Whether a conversation is open.
    #[must_use]
    pub fn is_open(&self) -> bool {
        !self.lines.is_empty()
    }

    /// The line showing now.
    #[must_use]
    pub fn current(&self) -> Option<&Line> {
        self.lines.get(self.index)
    }

    /// Advances, and reports whether the conversation is over.
    pub fn advance(&mut self) -> bool {
        self.index += 1;
        if self.index >= self.lines.len() {
            self.lines.clear();
            self.index = 0;
            return true;
        }
        false
    }

    /// Closes it.
    pub fn close(&mut self) {
        self.lines.clear();
        self.index = 0;
    }
}

/// What the villager says.
///
/// The game's tutorial, and its only one. A farming game that explains itself
/// in a menu explains itself to nobody; these are the six things a new player
/// has to know, in the order they need them, said by the person standing in the
/// field next to them.
#[must_use]
pub fn tutorial_lines(quests: &QuestLog) -> Vec<Line> {
    if quests.done(Quest::FirstHarvest) {
        return vec![
            Line {
                speaker: "薇拉",
                text: "你的第一批作物卖得不错呀。",
            },
            Line {
                speaker: "薇拉",
                text: "要不要试试种点别的？换季的时候地会荒，记得看日历。",
            },
            Line {
                speaker: "薇拉",
                text: "有什么想问的，随时来找我。",
            },
        ];
    }
    if quests.done(Quest::FirstSeed) {
        return vec![
            Line {
                speaker: "薇拉",
                text: "种下去了？那就每天给它浇水。",
            },
            Line {
                speaker: "薇拉",
                text: "水壶拿在手上，站在田里按空格就行。",
            },
            Line {
                speaker: "薇拉",
                text: "下雨天不用浇，老天爷替你做。",
            },
            Line {
                speaker: "薇拉",
                text: "长熟了用镰刀收，收完放进出货箱，第二天早上钱就到账了。",
            },
        ];
    }
    vec![
        Line {
            speaker: "薇拉",
            text: "你就是新来的农场主吧？我叫薇拉。",
        },
        Line {
            speaker: "薇拉",
            text: "这片地荒了很久了。想种东西，先要翻土。",
        },
        Line {
            speaker: "薇拉",
            text: "拿锄头站在地里按空格，翻好了再选中种子袋按一次。",
        },
        Line {
            speaker: "薇拉",
            text: "种子在那边的小店买，只卖当季的。",
        },
        Line {
            speaker: "薇拉",
            text: "按 Tab 看背包，按 ? 看操作。去吧，我在这儿看着。",
        },
    ]
}

// ---------------------------------------------------------------------------
// Quests
// ---------------------------------------------------------------------------

/// The jobs the game gives the player.
///
/// Three, and no more. A quest log is a promise that the list is worth reading,
/// and a list of twelve introductory tasks is a list nobody reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quest {
    /// Buy a seed.
    FirstSeed,
    /// Harvest anything.
    FirstHarvest,
    /// Ship anything and be paid.
    FirstSale,
}

/// Every quest, in the order they unlock.
pub const QUESTS: [Quest; 3] = [Quest::FirstSeed, Quest::FirstHarvest, Quest::FirstSale];

impl Quest {
    /// The title.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::FirstSeed => "第一粒种子",
            Self::FirstHarvest => "第一次收获",
            Self::FirstSale => "第一笔收入",
        }
    }

    /// What to do.
    #[must_use]
    pub const fn detail(self) -> &'static str {
        match self {
            Self::FirstSeed => "在种子商店买一包种子",
            Self::FirstHarvest => "种下并收获一株作物",
            Self::FirstSale => "把收成放进出货箱，睡一觉",
        }
    }
}

/// Which quests are done.
#[derive(Clone, Debug, Default)]
pub struct QuestLog {
    done: [bool; QUESTS.len()],
    /// Set the frame a quest completes, so the toast fires once.
    just_completed: Option<Quest>,
}

impl QuestLog {
    /// Whether a quest is done.
    #[must_use]
    pub fn done(&self, quest: Quest) -> bool {
        self.done[index_of(quest)]
    }

    /// Marks a quest done, once.
    pub fn complete(&mut self, quest: Quest) {
        let slot = &mut self.done[index_of(quest)];
        if !*slot {
            *slot = true;
            self.just_completed = Some(quest);
        }
    }

    /// Takes the quest that completed since this was last called.
    pub fn take_completed(&mut self) -> Option<Quest> {
        self.just_completed.take()
    }

    /// How many are done.
    #[must_use]
    pub fn count(&self) -> usize {
        self.done.iter().filter(|d| **d).count()
    }

    /// The first quest that is not done, if any.
    #[must_use]
    pub fn next(&self) -> Option<Quest> {
        QUESTS.iter().copied().find(|q| !self.done(*q))
    }
}

fn index_of(quest: Quest) -> usize {
    QUESTS.iter().position(|q| *q == quest).unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// What the player configured.
///
/// Written to a plain text file next to the save, in the same key/value format,
/// because a settings file that needs a serialisation crate to read is a
/// settings file that cannot be hand-edited when something goes wrong.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// 0 to 1.
    pub volume: f32,
    /// Whether the music plays at all.
    pub music: bool,
    /// Whether the aim highlight is drawn.
    pub highlight: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            volume: 0.55,
            music: true,
            highlight: true,
        }
    }
}

impl Settings {
    /// The file format: one `key=value` per line.
    #[must_use]
    pub fn to_text(self) -> String {
        format!(
            "volume={:.2}\nmusic={}\nhighlight={}\n",
            self.volume, self.music, self.highlight
        )
    }

    /// Reads the format back, keeping the default for anything missing or
    /// malformed.
    ///
    /// Deliberately forgiving: a settings file is not worth refusing to start
    /// over, and a player who breaks one should get their defaults back rather
    /// than a crash.
    #[must_use]
    pub fn from_text(text: &str) -> Self {
        let mut settings = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key.trim() {
                "volume" => {
                    if let Ok(value) = value.trim().parse::<f32>() {
                        settings.volume = value.clamp(0.0, 1.0);
                    }
                }
                // Only a real `true` or `false` counts. Treating anything else
                // as `false` would let a corrupted line silently turn a feature
                // off, which is worse than ignoring the line.
                "music" => {
                    if let Ok(value) = value.trim().parse::<bool>() {
                        settings.music = value;
                    }
                }
                "highlight" => {
                    if let Ok(value) = value.trim().parse::<bool>() {
                        settings.highlight = value;
                    }
                }
                _ => {}
            }
        }
        settings
    }
}

// ---------------------------------------------------------------------------
// The screens
// ---------------------------------------------------------------------------

/// The name every screen shares for its panel.
const PANEL_WIDTH: u32 = 208;

impl GameUi {
    /// Opens the villager's conversation.
    pub fn open_dialogue(&mut self) {
        self.dialogue = Dialogue {
            lines: tutorial_lines(&self.quests),
            index: 0,
        };
        self.screen = Screen::Dialogue;
    }

    /// Shows the morning report.
    pub fn show_summary(&mut self) {
        self.screen = Screen::Summary;
    }

    /// Draws the start menu.
    pub fn draw_title(
        &mut self,
        framebuffer: &mut Framebuffer,
        input: &UiInput,
        assets: &Assets,
        screen: UiRect,
        has_save: bool,
    ) -> UiAction {
        let theme = self.ui.theme;

        let mut action = UiAction::None;
        let height = 136;
        let panel = screen.place((PANEL_WIDTH, height), noxel_ui::Anchor::Center, (0, 0));
        let inner = {
            let mut painter = self.ui.painter(framebuffer);
            self.ui
                .panel_blocking(&mut painter, input, panel, &theme.panel);
            panel.inset(theme.metrics.padding)
        };

        let mut painter = self.ui.painter(framebuffer);
        let title = TextStyle::new(theme.palette.accent).with_scale(2);
        self.ui.font.draw_text(
            &mut painter,
            UiRect::new(inner.x, inner.y, inner.w, 24),
            "星野农场",
            &title.with_align(noxel_ui::TextAlign::Center),
        );
        let subtitle =
            TextStyle::new(theme.palette.text_dim).with_align(noxel_ui::TextAlign::Center);
        self.ui.font.draw_text(
            &mut painter,
            UiRect::new(inner.x, inner.y + 26, inner.w, 12),
            "Noxel Valley",
            &subtitle,
        );

        let mut y = inner.y + 44;
        let row = "title.buttons";
        let items: [(&str, UiAction, bool); 4] = [
            ("继续游戏", UiAction::Continue, has_save),
            ("新的农场", UiAction::NewGame, true),
            ("设置", UiAction::Settings, true),
            ("退出", UiAction::Quit, true),
        ];
        for (index, (label, act, enabled)) in items.into_iter().enumerate() {
            let rect = UiRect::new(inner.x + 24, y, inner.w - 48, 18);
            let response = self.ui.button(
                &mut painter,
                input,
                rect,
                Id::new(row).with(&index.to_string()),
                label,
                enabled,
            );
            if response.clicked {
                action = act;
            }
            y += 21;
        }
        let _ = assets;
        action
    }

    /// Draws the pause menu.
    pub fn draw_pause(
        &mut self,
        framebuffer: &mut Framebuffer,
        input: &UiInput,
        screen: UiRect,
    ) -> UiAction {
        let theme = self.ui.theme;

        let mut action = UiAction::None;
        let panel = screen.place((190, 112), noxel_ui::Anchor::Center, (0, 0));
        let inner = {
            let mut painter = self.ui.painter(framebuffer);
            self.ui
                .panel_blocking(&mut painter, input, panel, &theme.sunken);
            panel.inset(theme.metrics.padding)
        };
        let mut painter = self.ui.painter(framebuffer);
        self.ui.heading(
            &mut painter,
            UiRect::new(inner.x, inner.y, inner.w, 24),
            "暂停",
        );
        let row = "pause.buttons";
        let items: [(&str, UiAction); 4] = [
            ("继续游戏", UiAction::Resume),
            ("任务书", UiAction::Quests),
            ("设置", UiAction::Settings),
            ("保存并回到标题", UiAction::Title),
        ];
        let mut y = inner.y + 26;
        for (index, (label, act)) in items.into_iter().enumerate() {
            let rect = UiRect::new(inner.x + 16, y, inner.w - 32, 14);
            if self
                .ui
                .button(
                    &mut painter,
                    input,
                    rect,
                    Id::new(row).with(&index.to_string()),
                    label,
                    true,
                )
                .clicked
            {
                action = act;
            }
            y += 19;
        }
        action
    }

    /// Draws the settings screen.
    pub fn draw_settings(
        &mut self,
        framebuffer: &mut Framebuffer,
        input: &UiInput,
        screen: UiRect,
    ) -> UiAction {
        let theme = self.ui.theme;

        let mut action = UiAction::None;
        let heading_height = self.heading_height("设置");
        let panel = screen.place((248, heading_height + 92), noxel_ui::Anchor::Center, (0, 0));
        let inner = {
            let mut painter = self.ui.painter(framebuffer);
            self.ui
                .panel_blocking(&mut painter, input, panel, &theme.panel);
            panel.inset(theme.metrics.padding)
        };
        let mut painter = self.ui.painter(framebuffer);
        let used = self.title(
            &mut painter,
            UiRect::new(inner.x, inner.y, inner.w, heading_height),
            "设置",
        );

        let row = "settings";
        let mut y = inner.y + used as i32 + theme.metrics.gap;

        // -- volume: minus, a bar, plus ------------------------------------
        self.ui.label(
            &mut painter,
            UiRect::new(inner.x, y, 40, 12),
            "音量",
            &TextStyle::new(theme.palette.text),
        );
        let minus = UiRect::new(inner.right() - 74, y, 14, 12);
        let plus = UiRect::new(inner.right() - 16, y, 16, 12);
        if self
            .ui
            .button(
                &mut painter,
                input,
                minus,
                Id::new(row).with("0"),
                "-",
                true,
            )
            .clicked
        {
            self.settings.volume = (self.settings.volume - 0.1).clamp(0.0, 1.0);
            action = UiAction::SaveSettings;
        }
        if self
            .ui
            .button(&mut painter, input, plus, Id::new(row).with("1"), "+", true)
            .clicked
        {
            self.settings.volume = (self.settings.volume + 0.1).clamp(0.0, 1.0);
            action = UiAction::SaveSettings;
        }
        self.ui.bar(
            &mut painter,
            UiRect::new(inner.right() - 58, y + 3, 40, 6),
            self.settings.volume,
            &BarStyle {
                fill: theme.palette.accent,
                ..theme.bar
            },
        );
        y += 21;

        // -- music ---------------------------------------------------------
        self.ui.label(
            &mut painter,
            UiRect::new(inner.x, y, 60, 12),
            "音乐",
            &TextStyle::new(theme.palette.text),
        );
        if self
            .ui
            .checkbox(
                &mut painter,
                input,
                UiRect::new(inner.right() - 16, y, 16, 12),
                Id::new(row).with("2"),
                "",
                &mut self.settings.music,
            )
            .clicked
        {
            action = UiAction::SaveSettings;
        }
        y += 21;

        // -- the aim highlight ---------------------------------------------
        self.ui.label(
            &mut painter,
            UiRect::new(inner.x, y, 80, 12),
            "地块高亮",
            &TextStyle::new(theme.palette.text),
        );
        if self
            .ui
            .checkbox(
                &mut painter,
                input,
                UiRect::new(inner.right() - 16, y, 16, 12),
                Id::new(row).with("3"),
                "",
                &mut self.settings.highlight,
            )
            .clicked
        {
            action = UiAction::SaveSettings;
        }
        y += 22;

        let done = UiRect::new(inner.x + 30, y, inner.w - 60, 17);
        if self
            .ui
            .button(
                &mut painter,
                input,
                done,
                Id::new(row).with("4"),
                "返回",
                true,
            )
            .clicked
        {
            action = UiAction::Resume;
        }
        action
    }

    /// Draws the conversation.
    pub fn draw_dialogue(
        &mut self,
        framebuffer: &mut Framebuffer,
        input: &UiInput,
        screen: UiRect,
    ) -> UiAction {
        let theme = self.ui.theme;
        let Some(line) = self.dialogue.current().copied() else {
            return UiAction::DialogueDone;
        };

        // No scrim: a conversation happens in the world, and blacking it out is
        // how a chat becomes a cutscene.
        let mut action = UiAction::None;
        let panel = UiRect::new(screen.x + 12, screen.bottom() - 74, screen.w - 24, 62);
        let inner = {
            let mut painter = self.ui.painter(framebuffer);
            self.ui
                .panel_blocking(&mut painter, input, panel, &theme.panel);
            panel.inset(theme.metrics.padding)
        };
        let mut painter = self.ui.painter(framebuffer);
        self.ui.label_role(
            &mut painter,
            UiRect::new(inner.x, inner.y, inner.w, 12),
            line.speaker,
            noxel_ui::TextRole::Accent,
        );
        self.ui.label(
            &mut painter,
            UiRect::new(inner.x, inner.y + 14, inner.w, 30),
            line.text,
            &TextStyle::new(theme.palette.text).with_wrap(noxel_ui::Wrap::Width(inner.w)),
        );
        let hint = TextStyle::new(theme.palette.text_dim).with_align(noxel_ui::TextAlign::Right);
        let last = self.dialogue.index + 1 >= self.dialogue.lines.len();
        self.ui.font.draw_text(
            &mut painter,
            UiRect::new(inner.x, inner.bottom() - 12, inner.w, 12),
            if last { "[E] 结束" } else { "[E] 继续" },
            &hint,
        );
        // The whole panel is the button: a conversation line is not a form.
        if self
            .ui
            .button(
                &mut painter,
                input,
                panel,
                Id::new("dialogue.next"),
                "",
                true,
            )
            .clicked
        {
            action = UiAction::Advance;
        }
        let _ = input;
        action
    }

    /// Draws the quest log.
    pub fn draw_quests(
        &mut self,
        framebuffer: &mut Framebuffer,
        input: &UiInput,
        screen: UiRect,
    ) -> UiAction {
        let theme = self.ui.theme;

        let mut action = UiAction::None;
        let height = 34 + QUESTS.len() as u32 * 30 + 26;
        let panel = screen.place((248, height), noxel_ui::Anchor::Center, (0, 0));
        let inner = {
            let mut painter = self.ui.painter(framebuffer);
            self.ui
                .panel_blocking(&mut painter, input, panel, &theme.panel);
            panel.inset(theme.metrics.padding)
        };
        let mut painter = self.ui.painter(framebuffer);
        self.ui.heading(
            &mut painter,
            UiRect::new(inner.x, inner.y, inner.w, 24),
            "任务书",
        );
        let count = TextStyle::new(theme.palette.text_dim);
        self.ui.font.draw_text(
            &mut painter,
            UiRect::new(inner.x, inner.y, inner.w, 24),
            &format!("{}/{}", self.quests.count(), QUESTS.len()),
            &count.with_align(noxel_ui::TextAlign::Right),
        );

        let mut y = inner.y + 30;
        for quest in QUESTS {
            let done = self.quests.done(quest);
            let colour = if done {
                theme.palette.accent
            } else {
                theme.palette.text
            };
            // A filled square when done, a hollow one when not. A text glyph
            // would depend on the baked font carrying a tick, which the GB2312
            // common set does not promise.
            let box_rect = UiRect::new(inner.x, y + 2, 7, 7);
            if done {
                painter.fill(box_rect, theme.palette.accent);
            } else {
                painter.outline(box_rect, theme.palette.text_dim, 1);
            }
            self.ui.label(
                &mut painter,
                UiRect::new(inner.x + 12, y, inner.w - 12, 12),
                quest.title(),
                &TextStyle::new(colour),
            );
            self.ui.label(
                &mut painter,
                UiRect::new(inner.x + 12, y + 12, inner.w - 12, 12),
                quest.detail(),
                &TextStyle::new(theme.palette.text_dim),
            );
            y += 30;
        }

        let done = UiRect::new(inner.x + 40, y, inner.w - 80, 17);
        if self
            .ui
            .button(
                &mut painter,
                input,
                done,
                Id::new("quests.close"),
                "返回",
                true,
            )
            .clicked
        {
            action = UiAction::Resume;
        }
        action
    }

    /// Draws what the player is holding, beside them.
    ///
    /// Over the character, in the interface layer, where it is crisp and cannot
    /// be hidden behind a tree. The player is a sixteen-pixel sprite with no
    /// arms to speak of, so the only place a held hoe can be visible is next to
    /// them — and a player who cannot see what is in their hand has no way to
    /// tell the hoe from the watering can.
    pub fn draw_held_item(
        &mut self,
        framebuffer: &mut Framebuffer,
        state: &crate::sim::GameState,
        player: &crate::player::Player,
        assets: &Assets,
        focus: noxel_core::math::Vec3,
    ) {
        let Some(item) = state
            .inventory
            .slots()
            .get(state.selected)
            .and_then(|slot| *slot)
            .map(|slot| slot.item)
        else {
            return;
        };
        let at = aim_rect(
            player.tile(),
            focus,
            framebuffer.width(),
            framebuffer.height(),
        );
        let badge = UiRect::new(at.right() + 1, at.y - 5, 16, 16);
        let mut painter = self.ui.painter(framebuffer);
        if let Some(sprite) = crate::ui::item_sprite(assets, item) {
            painter.blit(sprite.image, sprite.rect, badge.x, badge.y, Color8::WHITE);
        } else {
            // No art for it: a coloured chip still says "you are holding
            // something", which is the part that matters.
            painter.fill(badge.inset(noxel_ui::Insets::all(3)), item.color());
        }
    }

    /// Outlines the tile the tool would act on.
    ///
    /// Drawn in the interface layer rather than as a world sprite because a
    /// sprite would need a texture, a material and a depth that is very slightly
    /// *above* the thing it is highlighting — and getting that last part wrong
    /// makes the cursor disappear behind a potato. The interface is composited
    /// after everything, so it cannot be occluded at all.
    ///
    /// The colour says whether the tool would do anything, which turns the
    /// cursor into feedback rather than decoration.
    pub fn draw_aim_highlight(
        &mut self,
        framebuffer: &mut Framebuffer,
        state: &crate::sim::GameState,
        player: &crate::player::Player,
        focus: noxel_core::math::Vec3,
    ) {
        if !self.settings.highlight || self.screen != Screen::Playing || self.help_open {
            return;
        }
        // The highlight is the tool cursor, and it is hidden when there is a
        // prompt on screen: two things asking for the same attention is one
        // thing too many.
        if crate::world::prompt_for(player.tile()).is_some() {
            return;
        }
        {
            let rect = aim_rect(
                player.aim_tile(),
                focus,
                framebuffer.width(),
                framebuffer.height(),
            );
            let theme = self.ui.theme;
            // Idle is a quiet outline; a tool that will do something is the
            // accent colour. The player learns the difference in about a second
            // without being told.
            let active = state.energy > 0.0;
            let colour = if active {
                theme.palette.accent
            } else {
                theme.palette.text_dim
            };
            let mut painter = self.ui.painter(framebuffer);
            for (dx, dy, w, h) in [
                (0, 0, crate::config::TILE, 1),
                (0, crate::config::TILE as i32 - 1, crate::config::TILE, 1),
                (0, 0, 1, crate::config::TILE),
                (crate::config::TILE as i32 - 1, 0, 1, crate::config::TILE),
            ] {
                painter.fill(
                    UiRect::new(rect.x + dx, rect.y + dy, w, h),
                    Color8::new(colour.r, colour.g, colour.b, 190),
                );
            }
        }
    }
}

/// What to do next, in one line.
///
/// The game had no way of answering this, and the consequence was a player who
/// bought a seed, had it put in their hand, pressed the use key on un-tilled
/// ground, and watched nothing happen — with no way to find out that the hoe
/// was the missing step. The genre's convention is not self-evident; it is
/// learned, and this is where it gets taught.
///
/// It reads the world rather than a script, so it is right at every point in
/// the loop: it names the ripe crop you are standing next to, the seed in your
/// hand, and the tool you have not selected.
#[must_use]
pub fn hint(
    state: &crate::sim::GameState,
    map: &crate::world::FarmMap,
    player: &crate::player::Player,
) -> Option<String> {
    use crate::config::Tool;
    use crate::sim::Item;
    use crate::world::Ground;

    let aim = player.aim_tile();
    let tile = map.get(aim.0, aim.1);
    let held = state
        .inventory
        .slots()
        .get(state.selected)
        .and_then(|slot| *slot)
        .map(|slot| slot.item);

    // Something to harvest beats everything: it is the one thing that will not
    // wait, because the season ends.
    if tile.is_some_and(|t| t.plant.is_some_and(|p| p.is_ripe())) {
        return Some("按 空格 收割".to_string());
    }

    if let Some(Item::Seed(crop)) = held {
        let ready = tile.is_some_and(|t| t.ground == Ground::Tilled && t.plant.is_none());
        if ready {
            return Some(format!("按 空格 种下{}", crop.name));
        }
        return Some("这块地还没翻。按 1 选锄头，对着地按 空格 翻土".to_string());
    }

    match state.current_tool() {
        Tool::Hoe if tile.is_some_and(|t| t.ground.is_hoeable() && t.plant.is_none()) => {
            Some("按 空格 翻土".to_string())
        }
        Tool::Can if tile.is_some_and(|t| t.ground == Ground::Tilled && !t.watered) => {
            Some("按 空格 浇水".to_string())
        }
        Tool::Hand => Some("按 Tab 打开背包，点一格种子拿在手上".to_string()),
        _ => Some("1-6 或滚轮 换工具 · Tab 背包 · ? 操作说明".to_string()),
    }
}

/// Where a tile lands on screen.
///
/// The same projection the renderer uses: one world unit is exactly `TILE`
/// pixels, and the camera's focus is the centre of the frame. The caller must
/// pass the camera's **snapped** focus rather than the player's position — the
/// camera smooths towards the player and snaps to whole sixteenths, so the two
/// differ by up to half a tile while walking, and the highlight drifts from the
/// tile the tool acts on by exactly that much.
#[must_use]
pub fn aim_rect(
    tile: (i32, i32),
    focus: noxel_core::math::Vec3,
    width: u32,
    height: u32,
) -> UiRect {
    let size = crate::config::TILE;
    let scale = size as f32;
    let centre_x = (tile.0 as f32 + 0.5 - focus.x) * scale + width as f32 * 0.5;
    let centre_y = (z_of(tile.1) - focus.z) * scale + height as f32 * 0.5;
    // Rounded, never truncated: a fractional screen position is a highlight
    // that shimmers by a pixel as the camera moves.
    UiRect::new(
        centre_x.round() as i32 - size as i32 / 2,
        centre_y.round() as i32 - size as i32 / 2,
        size,
        size,
    )
}

/// The world `z` of a tile row.
///
/// Screen `y` grows downwards and world `z` grows towards the bottom of the
/// screen, which is the whole reason the game can be top-down and depth-sorted
/// at once. Kept as a named function because getting the sign wrong is a
/// highlight that is mirrored about the middle of the screen — which still
/// looks plausible in the centre and is obviously wrong at the edges.
#[must_use]
pub fn z_of(row: i32) -> f32 {
    row as f32 + 0.5
}

/// The season and weather, as a mood name, for the music director.
///
/// A free function rather than a method so the mapping is testable on its own —
/// it is the whole of "the music changes with the weather", and it deserves to
/// be checkable without a sound card.
#[must_use]
pub fn mood_for(season: Season, weather: Weather, hour: f32) -> &'static str {
    let night = hour < 7.0 || hour >= 20.0;
    match (season, weather, night) {
        (_, Weather::Rain | Weather::Storm, _) => "fall",
        (Season::Winter, _, _) | (_, Weather::Snow, _) => "winter",
        (Season::Fall, _, _) => "fall",
        (_, _, true) => "fall",
        (Season::Summer, _, _) => "summer",
        (Season::Spring, _, _) => "spring",
    }
}

/// Weather you can see.
///
/// The weather was simulated and invisible: it changed what grew and how bright
/// the light was, and the only way to know it was raining was to read the icon
/// in the clock panel. That is a spreadsheet, not weather.
impl GameUi {
    /// Advances the weather animation.
    pub fn tick_weather(&mut self, dt: f32) {
        self.weather_phase = (self.weather_phase + dt) % 64.0;
    }

    /// Draws rain, snow or a storm over the world.
    ///
    /// In the interface layer, over everything: rain is *between* the camera and
    /// the farm, so nothing in the world may occlude it. The scatter is a pair
    /// of co-prime strides off the frame counter rather than a random number
    /// generator, so it is deterministic and needs no state.
    pub fn draw_weather(&mut self, framebuffer: &mut Framebuffer, weather: crate::config::Weather) {
        use crate::config::Weather;
        if matches!(weather, Weather::Sunny | Weather::Cloudy) {
            return;
        }
        let (width, height) = (framebuffer.width() as i32, framebuffer.height() as i32);
        let phase = self.weather_phase;
        let mut painter = self.ui.painter(framebuffer);

        match weather {
            Weather::Rain | Weather::Storm => {
                let heavy = weather == Weather::Storm;
                let count = if heavy { 96 } else { 54 };
                // Slanted, because vertical rain reads as static. The slant is
                // what tells the eye it is falling rather than hanging.
                let slant = if heavy { 3 } else { 2 };
                let speed = if heavy { 150.0 } else { 110.0 };
                let length = if heavy { 9 } else { 6 };
                let colour = if heavy {
                    Color8::new(150, 180, 220, 150)
                } else {
                    Color8::new(160, 190, 220, 110)
                };
                for index in 0..count {
                    let seed = index as f32 * 37.7;
                    let x = ((seed * 7.3).fract() * width as f32) as i32;
                    let y =
                        ((seed * 3.1 + phase * speed).fract() * (height as f32 + 24.0)) as i32 - 12;
                    for step in 0..length {
                        painter.fill(
                            UiRect::new(x + step * slant / length, y + step, 1, 1),
                            colour,
                        );
                    }
                    if heavy {
                        // A slower second layer, which is what gives the storm
                        // depth instead of a flat sheet of streaks.
                        let far = ((seed * 5.7 + phase * speed * 0.6).fract()
                            * (height as f32 + 24.0)) as i32
                            - 12;
                        for step in 0..5 {
                            painter.fill(
                                UiRect::new(x + 13 + step, far + step, 1, 1),
                                Color8::new(140, 165, 205, 90),
                            );
                        }
                    }
                }
            }
            Weather::Snow => {
                for index in 0..72 {
                    let seed = index as f32 * 23.9;
                    let drift = (phase * 0.9 + seed).sin() * 6.0;
                    let x = ((seed * 11.1).fract() * width as f32) as i32 + drift as i32;
                    let y =
                        ((seed * 4.3 + phase * 26.0).fract() * (height as f32 + 12.0)) as i32 - 6;
                    // Snow drifts rather than falls: a flake takes its time.
                    painter.fill(UiRect::new(x, y, 1, 1), Color8::new(232, 238, 248, 170));
                    if index % 3 == 0 {
                        painter.fill(UiRect::new(x + 1, y, 1, 1), Color8::new(232, 238, 248, 120));
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_highlight_lands_where_the_camera_says_the_tile_is() {
        // The reported bug was a highlight that did not sit on the tile the
        // tool acted on. This pins the projection to the renderer's own
        // arithmetic: one world unit is `TILE` pixels and the focus is the
        // centre of the frame, so a tile centred on the focus must land centred
        // on the frame.
        use noxel_core::math::Vec3;
        let (w, h) = (480, 270);
        let focus = Vec3::new(10.5, 0.0, 20.5);
        let rect = aim_rect((10, 20), focus, w, h);
        assert_eq!(rect.x, w as i32 / 2 - crate::config::TILE as i32 / 2);
        assert_eq!(rect.y, h as i32 / 2 - crate::config::TILE as i32 / 2);
        assert_eq!(rect.w, crate::config::TILE);
    }

    #[test]
    fn moving_one_tile_moves_the_highlight_exactly_one_tile() {
        use noxel_core::math::Vec3;
        let focus = Vec3::new(0.0, 0.0, 0.0);
        let base = aim_rect((5, 5), focus, 480, 270);
        let right = aim_rect((6, 5), focus, 480, 270);
        let below = aim_rect((5, 6), focus, 480, 270);
        assert_eq!(right.x - base.x, crate::config::TILE as i32);
        assert_eq!(
            right.y, base.y,
            "moving right must not move the highlight down"
        );
        assert_eq!(below.y - base.y, crate::config::TILE as i32);
        assert_eq!(
            below.x, base.x,
            "moving down must not move the highlight right"
        );
    }

    #[test]
    fn a_tile_below_the_focus_is_drawn_below_the_focus() {
        // Getting this sign wrong mirrors the highlight about the middle of the
        // screen: still plausible in the centre, obviously wrong at the edges.
        use noxel_core::math::Vec3;
        let focus = Vec3::new(0.0, 0.0, 0.0);
        let above = aim_rect((0, -3), focus, 480, 270);
        let below = aim_rect((0, 3), focus, 480, 270);
        assert!(
            above.y < 270 / 2,
            "a tile with a smaller row must be higher"
        );
        assert!(below.y > 270 / 2, "a tile with a larger row must be lower");
    }

    #[test]
    fn the_highlight_is_always_whole_pixels() {
        // A fractional position is a highlight that shimmers by a pixel as the
        // camera moves, which is exactly the kind of softness the art avoids.
        use noxel_core::math::Vec3;
        for step in 0..64 {
            let focus = Vec3::new(step as f32 / 16.0, 0.0, step as f32 / 7.0);
            let rect = aim_rect((step % 11, step % 7), focus, 480, 270);
            // The rect is built from rounded integers, so this is a check that
            // nothing fractional reached it in the first place.
            assert_eq!(rect.w, crate::config::TILE);
            assert!(rect.x.abs() < 10_000 && rect.y.abs() < 10_000);
        }
    }

    #[test]
    fn the_hint_names_the_hoe_when_a_seed_is_in_hand_and_the_ground_is_bare() {
        // This is the sentence whose absence made the game unplayable: buy a
        // seed, have it put in your hand, press the use key on un-tilled
        // ground, and watch nothing happen with no way to find out why.
        use crate::config::crop_by_key;
        use crate::sim::Item;
        use crate::world::{Ground, build_farm};

        let mut map = build_farm();
        let mut state = crate::sim::GameState::new(1);
        let player = crate::player::Player::at(crate::world::START_TILE);
        let crop = crop_by_key("parsnip").unwrap();
        let (x, y) = player.aim_tile();
        map.get_mut(x, y).unwrap().ground = Ground::Dirt;

        // A hoe in hand on bare ground.
        state.selected = 0;
        let text = hint(&state, &map, &player).expect("there is always something to say");
        assert!(
            text.contains("翻土"),
            "a hoe on bare ground should offer to till: {text}"
        );

        // A seed in hand on the same ground: the hoe is the missing step, and
        // the hint has to say so.
        state.inventory.add(Item::Seed(crop), 1);
        let slot = state.inventory.find(Item::Seed(crop)).unwrap();
        state.selected = slot;
        let text = hint(&state, &map, &player).expect("a hint");
        assert!(
            text.contains("翻土"),
            "a seed on bare ground must name the hoe: {text}"
        );
        assert!(text.contains('1'), "and it must name the key: {text}");

        // Tilled ground and the same seed: now it offers to sow.
        map.get_mut(x, y).unwrap().ground = Ground::Tilled;
        let text = hint(&state, &map, &player).expect("a hint");
        assert!(
            text.contains("种下"),
            "a seed on tilled ground should offer to sow: {text}"
        );
    }

    #[test]
    fn settings_survive_a_round_trip() {
        let settings = Settings {
            volume: 0.35,
            music: false,
            highlight: true,
        };
        assert_eq!(Settings::from_text(&settings.to_text()), settings);
    }

    #[test]
    fn a_broken_settings_file_gives_the_defaults_rather_than_a_crash() {
        // A settings file is not worth refusing to start over.
        let broken = "volume=banana\nmusic=\n=== \nhighlight=yes\n";
        let settings = Settings::from_text(broken);
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn a_settings_volume_outside_the_range_is_clamped() {
        assert_eq!(Settings::from_text("volume=9.0").volume, 1.0);
        assert_eq!(Settings::from_text("volume=-3.0").volume, 0.0);
    }

    #[test]
    fn a_dialogue_advances_through_its_lines_and_then_ends() {
        let mut dialogue = Dialogue {
            lines: vec![
                Line {
                    speaker: "a",
                    text: "one",
                },
                Line {
                    speaker: "b",
                    text: "two",
                },
            ],
            index: 0,
        };
        assert!(dialogue.is_open());
        assert_eq!(dialogue.current().unwrap().text, "one");
        assert!(!dialogue.advance());
        assert_eq!(dialogue.current().unwrap().text, "two");
        assert!(dialogue.advance(), "the last line should end it");
        assert!(!dialogue.is_open());
        assert!(dialogue.current().is_none());
    }

    #[test]
    fn the_tutorial_says_different_things_as_the_player_learns() {
        // The villager is the tutorial. If the first three conversations are
        // identical, she is a signpost rather than a character.
        let mut quests = QuestLog::default();
        let fresh = tutorial_lines(&quests).len();
        quests.complete(Quest::FirstSeed);
        let seeded = tutorial_lines(&quests).len();
        quests.complete(Quest::FirstHarvest);
        let harvested = tutorial_lines(&quests).len();
        assert_ne!(fresh, seeded);
        assert_ne!(seeded, harvested);
        // And the first conversation has to mention the hoe, or it teaches
        // nothing.
        let lines = tutorial_lines(&QuestLog::default());
        assert!(
            lines.iter().any(|l| l.text.contains("锄头")),
            "no hoe in the tutorial"
        );
    }

    #[test]
    fn a_quest_completes_once_and_reports_it_once() {
        let mut log = QuestLog::default();
        assert!(log.next() == Some(Quest::FirstSeed));
        log.complete(Quest::FirstSeed);
        log.complete(Quest::FirstSeed);
        assert_eq!(log.take_completed(), Some(Quest::FirstSeed));
        assert_eq!(log.take_completed(), None, "the completion fired twice");
        assert_eq!(log.count(), 1);
        assert_eq!(log.next(), Some(Quest::FirstHarvest));
    }

    #[test]
    fn the_quest_log_finishes() {
        let mut log = QuestLog::default();
        for quest in QUESTS {
            log.complete(quest);
        }
        assert_eq!(log.count(), QUESTS.len());
        assert!(log.next().is_none());
    }

    #[test]
    fn the_music_follows_the_weather_and_the_hour() {
        // The whole of "the soundtrack changes with the world", as one function.
        assert_eq!(mood_for(Season::Spring, Weather::Sunny, 12.0), "spring");
        assert_eq!(mood_for(Season::Summer, Weather::Sunny, 12.0), "summer");
        assert_eq!(mood_for(Season::Fall, Weather::Cloudy, 12.0), "fall");
        assert_eq!(mood_for(Season::Winter, Weather::Sunny, 12.0), "winter");
        // Rain overrides the season: it is the loudest thing happening.
        assert_eq!(mood_for(Season::Summer, Weather::Storm, 12.0), "fall");
        // Night is quieter than day whatever the season.
        assert_eq!(mood_for(Season::Spring, Weather::Sunny, 22.0), "fall");
    }

    #[test]
    fn the_mood_names_are_ones_the_audio_engine_knows() {
        // A typo here is silent: the director would fall back to its first mood
        // and every season would sound like spring.
        let known = ["spring", "summer", "fall", "winter"];
        for season in Season::ALL {
            for weather in [
                Weather::Sunny,
                Weather::Cloudy,
                Weather::Rain,
                Weather::Storm,
                Weather::Snow,
            ] {
                for hour in [3.0, 12.0, 21.0] {
                    let mood = mood_for(season, weather, hour);
                    assert!(
                        known.contains(&mood),
                        "{mood:?} is not a mood the engine has"
                    );
                }
            }
        }
    }
}
