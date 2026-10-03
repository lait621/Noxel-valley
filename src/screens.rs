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
        let height = 118;
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
            let rect = UiRect::new(inner.x + 24, y, inner.w - 48, 16);
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
            y += 18;
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
        let panel = screen.place((190, 96), noxel_ui::Anchor::Center, (0, 0));
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
            y += 16;
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
        let panel = screen.place((220, 104), noxel_ui::Anchor::Center, (0, 0));
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
            "设置",
        );

        let row = "settings";
        let mut y = inner.y + 28;

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
        y += 18;

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
        y += 18;

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
        y += 20;

        let done = UiRect::new(inner.x + 30, y, inner.w - 60, 14);
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
        let height = 30 + QUESTS.len() as u32 * 26 + 22;
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
            y += 26;
        }

        let done = UiRect::new(inner.x + 40, y, inner.w - 80, 14);
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
            let (x, y) = player.aim_tile();
            let focus = player.camera_focus();
            let tile = crate::config::TILE as f32;
            let sx = ((x as f32 + 0.5 - focus.x) * tile) + framebuffer.width() as f32 * 0.5;
            let sy = ((y as f32 + 0.5 - focus.y) * tile) + framebuffer.height() as f32 * 0.5;
            let rect = UiRect::new(
                sx.round() as i32 - crate::config::TILE as i32 / 2,
                sy.round() as i32 - crate::config::TILE as i32 / 2,
                crate::config::TILE,
                crate::config::TILE,
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

#[cfg(test)]
mod tests {
    use super::*;

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
