use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub fn config_dir() -> PathBuf {
    dirs::home_dir()
        .expect("no home dir")
        .join(".config")
        .join("desktop-for-hypr")
}

pub fn config_file() -> PathBuf {
    config_dir().join("config.json")
}

fn expand_user(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    } else if p == "~"
        && let Some(home) = dirs::home_dir() {
            return home.to_string_lossy().into_owned();
        }
    p.to_string()
}

// serde default fns = DEFAULT_CONFIG values
fn d_desktop_path() -> String {
    "~/Desktop".into()
}
fn d_icon_size() -> i32 {
    48
}
fn d_show_hidden() -> bool {
    false
}
fn d_launch_single_click() -> bool {
    false
}
fn d_sort_by() -> String {
    "name".into()
}
fn d_sort_order() -> String {
    "asc".into()
}
fn d_layout() -> String {
    "grid".into()
}
fn d_columns() -> i32 {
    4
}
fn d_theme() -> String {
    "default".into()
}
fn d_arrange_mode() -> String {
    "auto".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "d_desktop_path")]
    pub desktop_path: String,
    #[serde(default = "d_icon_size")]
    pub icon_size: i32,
    #[serde(default = "d_show_hidden")]
    pub show_hidden: bool,
    #[serde(default = "d_launch_single_click")]
    pub launch_single_click: bool,
    #[serde(default = "d_sort_by")]
    pub sort_by: String,
    #[serde(default = "d_sort_order")]
    pub sort_order: String,
    #[serde(default = "d_layout")]
    pub layout: String,
    #[serde(default = "d_columns")]
    pub columns: i32,
    #[serde(default = "d_theme")]
    pub theme: String,
    #[serde(default = "d_arrange_mode")]
    pub arrange_mode: String,

    // excluded from JSON (Python: filtered in save, absent in file)
    #[serde(skip)]
    pub debug: bool,
    #[serde(skip)]
    pub config_file: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        // = get_default_settings(), with __post_init__ expand
        Self {
            desktop_path: expand_user(&d_desktop_path()),
            icon_size: d_icon_size(),
            show_hidden: d_show_hidden(),
            launch_single_click: d_launch_single_click(),
            sort_by: d_sort_by(),
            sort_order: d_sort_order(),
            layout: d_layout(),
            columns: d_columns(),
            theme: d_theme(),
            arrange_mode: d_arrange_mode(),
            debug: false,
            config_file: None,
        }
    }
}

/// CLI overrides — None = not passed (Python `args.get(k) is not None`).
#[derive(Debug, Default)]
pub struct Overrides {
    pub config_file: Option<String>,
    pub desktop_path: Option<String>,
    pub icon_size: Option<i32>,
    pub show_hidden: Option<bool>,
    pub launch_single_click: Option<bool>,
    pub sort_by: Option<String>,
    pub sort_order: Option<String>,
    pub layout: Option<String>,
    pub columns: Option<i32>,
    pub theme: Option<String>,
    pub arrange_mode: Option<String>,
    pub debug: Option<bool>,
}

pub fn load_settings(args: Option<&Overrides>) -> Settings {
    let mut config_path = config_file();

    if let Some(a) = args
        && let Some(cf) = &a.config_file {
            let custom = PathBuf::from(cf);
            if custom.exists() {
                config_path = custom;
            }
        }

    // defaults < file
    let mut s = if config_path.exists() {
        match fs::read_to_string(&config_path) {
            Ok(txt) => serde_json::from_str::<Settings>(&txt).unwrap_or_default(),
            Err(_) => Settings::default(), // IOError -> pass
        }
    } else {
        Settings::default()
    };

    // < CLI args
    if let Some(a) = args {
        if let Some(v) = &a.desktop_path {
            s.desktop_path = expand_user(v);
        }
        if let Some(v) = a.icon_size {
            s.icon_size = v;
        }
        if let Some(v) = a.show_hidden {
            s.show_hidden = v;
        }
        if let Some(v) = a.launch_single_click {
            s.launch_single_click = v;
        }
        if let Some(v) = &a.sort_by {
            s.sort_by = v.clone();
        }
        if let Some(v) = &a.sort_order {
            s.sort_order = v.clone();
        }
        if let Some(v) = &a.layout {
            s.layout = v.clone();
        }
        if let Some(v) = a.columns {
            s.columns = v;
        }
        if let Some(v) = &a.theme {
            s.theme = v.clone();
        }
        if let Some(v) = &a.arrange_mode {
            s.arrange_mode = v.clone();
        }
        if let Some(v) = a.debug {
            s.debug = v;
        }
        s.config_file = a.config_file.clone();
    }

    // __post_init__: expand even when from file/default
    s.desktop_path = expand_user(&s.desktop_path);
    s
}

pub fn save_settings(settings: &Settings) -> std::io::Result<()> {
    fs::create_dir_all(config_dir())?;
    // serde(skip) drops debug + config_file -> matches Python filter
    let json = serde_json::to_vec_pretty(settings).map_err(std::io::Error::other)?;
    // Atomic write: a crash mid-write can't leave a truncated config.
    let dst = config_file();
    let tmp = dst.with_extension("json.tmp");
    fs::write(&tmp, &json)?;
    fs::rename(&tmp, &dst)
}
