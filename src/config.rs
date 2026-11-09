use std::path::PathBuf;

use serde::Deserialize;

pub fn cycle<T: Copy + PartialEq>(all: &[T], cur: T, forward: bool) -> T {
    let n = all.len();
    let i = all.iter().position(|x| *x == cur).unwrap_or(0);
    all[if forward {
        (i + 1) % n
    } else {
        (i + n - 1) % n
    }]
}

pub const APP_NAME: &str = env!("CARGO_PKG_NAME");

pub struct ApiCredentials {
    pub api_id: i32,
    pub api_hash: String,
}

impl std::fmt::Debug for ApiCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiCredentials").finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApiFile {
    api_id: i32,
    api_hash: String,
}

pub fn env_prefix() -> String {
    APP_NAME.to_uppercase().replace('-', "_")
}

fn home() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set".to_owned())
}

fn xdg_dir(var: &str, fallback: &str) -> Result<PathBuf, String> {
    match std::env::var_os(var).filter(|v| !v.is_empty()) {
        Some(dir) => Ok(PathBuf::from(dir)),
        None => Ok(home()?.join(fallback)),
    }
}

pub fn config_dir() -> Result<PathBuf, String> {
    Ok(xdg_dir("XDG_CONFIG_HOME", ".config")?.join(APP_NAME))
}

pub fn cache_dir() -> Result<PathBuf, String> {
    Ok(xdg_dir("XDG_CACHE_HOME", ".cache")?.join(APP_NAME))
}

pub fn data_dir() -> Result<PathBuf, String> {
    if let Some(dir) =
        std::env::var_os(format!("{}_DATA_DIR", env_prefix())).filter(|v| !v.is_empty())
    {
        return Ok(PathBuf::from(dir));
    }
    Ok(xdg_dir("XDG_DATA_HOME", ".local/share")?.join(APP_NAME))
}

pub fn load_credentials() -> Result<ApiCredentials, String> {
    let prefix = env_prefix();
    let id_var = format!("{prefix}_API_ID");
    let hash_var = format!("{prefix}_API_HASH");
    let from_env = env_credentials(
        (&id_var, std::env::var(&id_var).ok()),
        (&hash_var, std::env::var(&hash_var).ok()),
    )?;
    if let Some(creds) = from_env {
        return Ok(creds);
    }
    let path = config_dir()?.join("api.toml");
    let raw = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "cannot read {}: {e}; create it with api_id and api_hash or set {id_var}/{hash_var}",
            path.display()
        )
    })?;
    let file: ApiFile = toml::from_str(&raw)
        .map_err(|e| format!("{} is invalid: {}", path.display(), e.message()))?;
    if file.api_hash.trim().is_empty() {
        return Err(format!("{}: api_hash is empty", path.display()));
    }
    Ok(ApiCredentials {
        api_id: file.api_id,
        api_hash: file.api_hash.trim().to_owned(),
    })
}

fn env_credentials(
    (id_var, id): (&str, Option<String>),
    (hash_var, hash): (&str, Option<String>),
) -> Result<Option<ApiCredentials>, String> {
    let (id, hash) = match (id, hash) {
        (None, None) => return Ok(None),
        (Some(_), None) => {
            return Err(format!(
                "{id_var} is set but {hash_var} is not; set both or neither"
            ));
        }
        (None, Some(_)) => {
            return Err(format!(
                "{hash_var} is set but {id_var} is not; set both or neither"
            ));
        }
        (Some(id), Some(hash)) => (id, hash),
    };
    let api_id = id
        .trim()
        .parse()
        .map_err(|_| format!("{id_var} is not a number"))?;
    if hash.trim().is_empty() {
        return Err(format!("{hash_var} is empty"));
    }
    Ok(Some(ApiCredentials {
        api_id,
        api_hash: hash.trim().to_owned(),
    }))
}

pub fn keymap_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("keymap.toml"))
}

pub fn settings_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("config.toml"))
}

pub fn theme_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("theme.toml"))
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Notifications {
    pub desktop: bool,
    pub sound: bool,
    pub volume: u8,
    pub sound_file: String,
}

impl Default for Notifications {
    fn default() -> Self {
        Self {
            desktop: true,
            sound: true,
            volume: 70,
            sound_file: String::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageLayout {
    #[serde(alias = "soft")]
    Classic,
    Cards,
    Compact,
    Minimal,
    #[default]
    Bubbles,
    Split,
}

pub const SOFT_REMOVED: &str = "layout \"soft\" was removed, using classic";

const LIBRETRANSLATE_KEY: &str = "libretranslate_url";

pub const LIBRETRANSLATE_REMOVED: &str = "[chats] libretranslate_url is no longer supported and is ignored; translation uses Telegram only";

impl MessageLayout {
    pub const ALL: [MessageLayout; 6] = [
        MessageLayout::Classic,
        MessageLayout::Cards,
        MessageLayout::Compact,
        MessageLayout::Minimal,
        MessageLayout::Split,
        MessageLayout::Bubbles,
    ];

    pub fn name(self) -> &'static str {
        match self {
            MessageLayout::Classic => "classic",
            MessageLayout::Cards => "cards",
            MessageLayout::Compact => "compact",
            MessageLayout::Minimal => "minimal",
            MessageLayout::Bubbles => "bubbles",
            MessageLayout::Split => "split",
        }
    }

    pub fn step(self, forward: bool) -> MessageLayout {
        cycle(&Self::ALL, self, forward)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BorderStyle {
    Rounded,
    #[default]
    Plain,
    Thick,
    Double,
}

impl BorderStyle {
    pub const ALL: [BorderStyle; 4] = [
        BorderStyle::Rounded,
        BorderStyle::Plain,
        BorderStyle::Thick,
        BorderStyle::Double,
    ];

    pub fn name(self) -> &'static str {
        match self {
            BorderStyle::Rounded => "rounded",
            BorderStyle::Plain => "plain",
            BorderStyle::Thick => "thick",
            BorderStyle::Double => "double",
        }
    }

    pub fn step(self, forward: bool) -> BorderStyle {
        cycle(&Self::ALL, self, forward)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeChoice {
    Custom,
    #[default]
    Terminal,
    Default,
    HighContrast,
    Light,
    Mono,
}

impl ThemeChoice {
    pub const ALL: [ThemeChoice; 6] = [
        ThemeChoice::Terminal,
        ThemeChoice::Default,
        ThemeChoice::HighContrast,
        ThemeChoice::Light,
        ThemeChoice::Mono,
        ThemeChoice::Custom,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ThemeChoice::Custom => "custom",
            other => other.base_name(),
        }
    }

    pub fn step(self, forward: bool) -> ThemeChoice {
        cycle(&Self::ALL, self, forward)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Appearance {
    pub theme: ThemeChoice,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout: Option<MessageLayout>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub border_style: Option<BorderStyle>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_shape: Option<crate::theme::AvatarShape>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seconds: Option<bool>,
    pub show_ids: bool,
    pub exact_numbers: bool,
    pub folder_counter: FolderCounter,
    pub folder_counter_unmuted_only: bool,
    pub hide_all_chats: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FolderCounter {
    #[default]
    Chats,
    Messages,
    Off,
}

impl FolderCounter {
    pub const ALL: [FolderCounter; 3] = [
        FolderCounter::Chats,
        FolderCounter::Messages,
        FolderCounter::Off,
    ];

    pub fn name(self) -> &'static str {
        match self {
            FolderCounter::Chats => "unread chats",
            FolderCounter::Messages => "unread messages",
            FolderCounter::Off => "off",
        }
    }

    pub fn step(self, forward: bool) -> FolderCounter {
        cycle(&Self::ALL, self, forward)
    }
}

pub const TRANSLATE_LANGUAGES: [&str; 16] = [
    "ru", "en", "uk", "be", "kk", "de", "fr", "es", "it", "pt", "pl", "tr", "zh", "ja", "ko", "ar",
];

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Chats {
    pub translate_to: String,
    pub photos_as_files: bool,
    pub save_to_chat: i64,
}

impl Default for Chats {
    fn default() -> Self {
        Self {
            translate_to: "ru".to_owned(),
            photos_as_files: false,
            save_to_chat: 0,
        }
    }
}

impl Chats {
    pub fn step_language(&mut self, forward: bool) {
        let cur = TRANSLATE_LANGUAGES
            .iter()
            .position(|l| *l == self.translate_to);
        let n = TRANSLATE_LANGUAGES.len();
        let next = match (cur, forward) {
            (Some(i), true) => (i + 1) % n,
            (Some(i), false) => (i + n - 1) % n,
            (None, _) => 0,
        };
        self.translate_to = TRANSLATE_LANGUAGES[next].to_owned();
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Privacy {
    pub dont_mark_read: bool,
    pub ghost_mode: bool,
    pub keep_deleted: bool,
    pub keep_deleted_limit: usize,
    pub hide_blocked: bool,
    pub hide_stories: bool,
    pub hide_phone: bool,
}

pub const KEEP_DELETED_LIMIT: usize = 5000;

impl Default for Privacy {
    fn default() -> Self {
        Self {
            dont_mark_read: false,
            ghost_mode: false,
            keep_deleted: false,
            keep_deleted_limit: KEEP_DELETED_LIMIT,
            hide_blocked: false,
            hide_stories: false,
            hide_phone: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OpenFocus {
    #[default]
    Smart,
    Composer,
    Messages,
}

impl OpenFocus {
    pub const ALL: [OpenFocus; 3] = [OpenFocus::Smart, OpenFocus::Composer, OpenFocus::Messages];

    pub fn name(self) -> &'static str {
        match self {
            OpenFocus::Smart => "smart",
            OpenFocus::Composer => "composer",
            OpenFocus::Messages => "messages",
        }
    }

    pub fn step(self, forward: bool) -> OpenFocus {
        cycle(&Self::ALL, self, forward)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Navigation {
    pub open_chat_focus: OpenFocus,
    pub after_send_next_unread: bool,
    pub skip_muted: bool,
}

impl Default for Navigation {
    fn default() -> Self {
        Self {
            open_chat_focus: OpenFocus::Smart,
            after_send_next_unread: false,
            skip_muted: true,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub notifications: Notifications,
    pub privacy: Privacy,
    pub appearance: Appearance,
    pub navigation: Navigation,
    pub chats: Chats,
}

impl Settings {
    pub fn parse(raw: &str, notes: &mut Vec<String>) -> Result<Settings, String> {
        let mut table: toml::Table = raw
            .parse()
            .map_err(|e: toml::de::Error| e.message().to_owned())?;
        let soft = table
            .get("appearance")
            .and_then(|a| a.get("layout"))
            .and_then(|l| l.as_str())
            == Some("soft");
        if soft {
            notes.push(format!("settings: {SOFT_REMOVED}"));
        }
        if let Some(toml::Value::Table(chats)) = table.get_mut("chats")
            && chats.remove(LIBRETRANSLATE_KEY).is_some()
        {
            notes.push(format!("settings: {LIBRETRANSLATE_REMOVED}"));
        }
        Settings::deserialize(toml::Value::Table(table)).map_err(|e| e.message().to_owned())
    }

    pub fn to_toml(&self) -> Result<String, String> {
        toml::to_string(self).map_err(|e| e.to_string())
    }

    pub fn volume(&self) -> u8 {
        self.notifications.volume.min(100)
    }

    pub fn auto_read(&self) -> bool {
        !self.privacy.dont_mark_read && !self.privacy.ghost_mode
    }

    pub fn send_chat_actions(&self) -> bool {
        !self.privacy.ghost_mode
    }

    pub fn online(&self, focused: bool) -> bool {
        focused && !self.privacy.ghost_mode
    }
}

pub fn load_settings(path: &std::path::Path, notes: &mut Vec<String>) -> Result<Settings, String> {
    match std::fs::read_to_string(path) {
        Ok(raw) => Settings::parse(&raw, notes).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

pub fn save_settings(path: &std::path::Path, settings: &Settings) -> Result<(), String> {
    let raw = settings.to_toml()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, raw).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("cannot save {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_credentials_need_both_values_and_a_hash() {
        let id = |v: Option<&str>| ("ID", v.map(str::to_owned));
        let hash = |v: Option<&str>| ("HASH", v.map(str::to_owned));
        assert!(env_credentials(id(None), hash(None)).unwrap().is_none());
        let both = env_credentials(id(Some(" 12 ")), hash(Some(" abc ")))
            .unwrap()
            .unwrap();
        assert_eq!(both.api_id, 12);
        assert_eq!(both.api_hash, "abc");
        let err = |i, h| env_credentials(id(i), hash(h)).err().unwrap();
        assert_eq!(err(Some("12"), Some("  ")), "HASH is empty");
        assert_eq!(
            err(Some("12"), None),
            "ID is set but HASH is not; set both or neither"
        );
        assert_eq!(
            err(None, Some("abc")),
            "HASH is set but ID is not; set both or neither"
        );
        assert_eq!(err(Some("x"), Some("abc")), "ID is not a number");
    }

    #[test]
    fn typos_in_any_settings_section_are_rejected() {
        for (raw, key) in [
            ("[privacy]\nghost = true\n", "ghost"),
            ("[notifications]\nsoud = false\n", "soud"),
            ("[privcy]\nghost_mode = true\n", "privcy"),
            ("[appearance]\nthme = \"dark\"\n", "thme"),
            ("[chats]\ntranslate = \"en\"\n", "translate"),
            ("[navigation]\nskip = true\n", "skip"),
        ] {
            let err = Settings::parse(raw, &mut Vec::new()).unwrap_err();
            assert!(
                err.contains("unknown field") && err.contains(key),
                "{raw}: {err}"
            );
        }
        let ok = Settings::parse(
            "[notifications]\nsound = false\n[privacy]\nghost_mode = true\n",
            &mut Vec::new(),
        )
        .unwrap();
        assert!(!ok.notifications.sound);
        assert!(ok.privacy.ghost_mode);
        assert_eq!(
            Settings::parse(&Settings::default().to_toml().unwrap(), &mut Vec::new()).unwrap(),
            Settings::default()
        );
    }

    #[test]
    fn settings_roundtrip_and_gating() {
        let d = Settings::default();
        assert!(d.notifications.desktop);
        assert!(d.auto_read());
        assert!(d.send_chat_actions());
        assert!(d.online(true));
        assert!(!d.online(false));
        let partial =
            Settings::parse("[privacy]\ndont_mark_read = true\n", &mut Vec::new()).unwrap();
        assert!(partial.notifications.desktop);
        assert!(!partial.auto_read());
        assert!(partial.send_chat_actions());
        assert!(partial.online(true));
        let ghost = Settings::parse("[privacy]\nghost_mode = true\n", &mut Vec::new()).unwrap();
        assert!(!ghost.auto_read());
        assert!(!ghost.send_chat_actions());
        assert!(!ghost.online(true));
        assert!(Settings::parse("privacy = 3", &mut Vec::new()).is_err());
        assert_eq!(d.appearance.layout, None);
        assert_eq!(d.appearance.border_style, None);
        assert_eq!(d.appearance.theme, ThemeChoice::Terminal);
        assert!(d.notifications.sound);
        let classic = Settings::parse(
            "[appearance]\ntheme = \"high-contrast\"\nlayout = \"bubbles\"\nborder_style = \"double\"\navatar_shape = \"rounded\"\n[notifications]\nvolume = 250\nsound_file = \"/tmp/a.oga\"\n",
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(classic.appearance.layout, Some(MessageLayout::Bubbles));
        assert_eq!(classic.appearance.border_style, Some(BorderStyle::Double));
        assert_eq!(
            classic.appearance.avatar_shape,
            Some(crate::theme::AvatarShape::Rounded)
        );
        assert_eq!(d.appearance.avatar_shape, None);
        assert_eq!(classic.appearance.theme, ThemeChoice::HighContrast);
        assert!(Settings::parse("[appearance]\ntheme = \"neon\"\n", &mut Vec::new()).is_err());
        assert!(Settings::parse("[appearance]\naccent = \"red\"\n", &mut Vec::new()).is_err());
        assert_eq!(MessageLayout::default(), MessageLayout::Bubbles);
        assert_eq!(BorderStyle::default(), BorderStyle::Plain);
        assert_eq!(ThemeChoice::Mono.step(true), ThemeChoice::Custom);
        assert_eq!(ThemeChoice::Custom.step(true), ThemeChoice::Terminal);
        assert_eq!(ThemeChoice::Terminal.step(false), ThemeChoice::Custom);
        assert_eq!(MessageLayout::Classic.step(false), MessageLayout::Bubbles);
        assert!(
            Settings::parse("[appearance]\nborder_style = \"dotted\"\n", &mut Vec::new()).is_err()
        );
        assert_eq!(classic.volume(), 100);
        assert_eq!(classic.notifications.sound_file, "/tmp/a.oga");
        assert!(Settings::parse("[appearance]\nlayout = \"fancy\"\n", &mut Vec::new()).is_err());
        let mut notes = Vec::new();
        let old = Settings::parse("[appearance]\nlayout = \"soft\"\n", &mut notes).unwrap();
        assert_eq!(old.appearance.layout, Some(MessageLayout::Classic));
        assert!(notes.iter().any(|n| n.contains(SOFT_REMOVED)));
        assert_eq!(
            Settings::parse(&classic.to_toml().unwrap(), &mut Vec::new()).unwrap(),
            classic
        );
        let dir = std::env::temp_dir().join(format!("gramtty-settings-{}", std::process::id()));
        let path = dir.join("config.toml");
        assert_eq!(
            load_settings(&path, &mut Vec::new()).unwrap(),
            Settings::default()
        );
        save_settings(&path, &ghost).unwrap();
        assert_eq!(load_settings(&path, &mut Vec::new()).unwrap(), ghost);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn chats_and_folder_options_parse_and_roundtrip() {
        let d = Settings::default();
        assert_eq!(d.chats.translate_to, "ru");
        assert!(!d.chats.photos_as_files);
        assert_eq!(d.chats.save_to_chat, 0);
        assert_eq!(d.appearance.folder_counter, FolderCounter::Chats);
        assert!(!d.appearance.folder_counter_unmuted_only);
        assert!(!d.appearance.hide_all_chats);
        let s = Settings::parse(
            "[chats]\ntranslate_to = \"de\"\nphotos_as_files = true\nsave_to_chat = -100123\n[appearance]\nfolder_counter = \"messages\"\nfolder_counter_unmuted_only = true\nhide_all_chats = true\n",
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(s.chats.translate_to, "de");
        assert_eq!(s.chats.save_to_chat, -100123);
        assert_eq!(s.appearance.folder_counter, FolderCounter::Messages);
        assert_eq!(
            Settings::parse(&s.to_toml().unwrap(), &mut Vec::new()).unwrap(),
            s
        );
        assert!(Settings::parse("[chats]\ntranslate = 1\n", &mut Vec::new()).is_err());
        let mut notes = Vec::new();
        let old = Settings::parse(
            "[chats]\ntranslate_to = \"en\"\nlibretranslate_url = \"http://lt.local\"\n",
            &mut notes,
        )
        .unwrap();
        assert_eq!(old.chats.translate_to, "en");
        assert_eq!(notes, vec![format!("settings: {LIBRETRANSLATE_REMOVED}")]);
        assert!(!old.to_toml().unwrap().contains("libretranslate"));
        let mut notes = Vec::new();
        assert!(Settings::parse("[chats]\nlibretranslate = \"x\"\n", &mut notes).is_err());
        Settings::parse("[chats]\ntranslate_to = \"en\"\n", &mut notes).unwrap();
        assert!(notes.is_empty());
        assert!(
            Settings::parse("[appearance]\nfolder_counter = \"all\"\n", &mut Vec::new()).is_err()
        );
        let mut c = Chats::default();
        c.step_language(false);
        assert_eq!(c.translate_to, "ar");
        c.translate_to = "xx".to_owned();
        c.step_language(true);
        assert_eq!(c.translate_to, "ru");
        assert_eq!(FolderCounter::Off.step(true), FolderCounter::Chats);
    }

    #[test]
    fn navigation_options_parse_and_roundtrip() {
        let d = Settings::default();
        assert_eq!(d.navigation.open_chat_focus, OpenFocus::Smart);
        assert!(!d.navigation.after_send_next_unread);
        assert!(d.navigation.skip_muted);
        let s = Settings::parse(
            "[navigation]\nopen_chat_focus = \"messages\"\nafter_send_next_unread = true\nskip_muted = false\n",
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(s.navigation.open_chat_focus, OpenFocus::Messages);
        assert!(s.navigation.after_send_next_unread);
        assert!(!s.navigation.skip_muted);
        assert_eq!(
            Settings::parse(&s.to_toml().unwrap(), &mut Vec::new()).unwrap(),
            s
        );
        assert!(
            Settings::parse(
                "[navigation]\nopen_chat_focus = \"auto\"\n",
                &mut Vec::new()
            )
            .is_err()
        );
        assert!(Settings::parse("[navigation]\nnext = 1\n", &mut Vec::new()).is_err());
        assert_eq!(OpenFocus::Smart.step(true), OpenFocus::Composer);
        assert_eq!(OpenFocus::Messages.step(true), OpenFocus::Smart);
    }
}
