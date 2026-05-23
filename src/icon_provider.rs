//! Port of Python src/icon_provider.py — resolves a path to a themed icon.

use std::cell::Cell;
use std::path::Path;

use relm4::gtk;
use relm4::gtk::prelude::*;
use relm4::gtk::{gdk, gio};
use tracing::{debug, warn};

#[derive(Debug, Clone)]
pub struct DesktopIcon {
    pub name: String,
    pub path: String,
    pub icon_name: Option<String>,
    pub is_app: bool,
    pub is_dir: bool,
}

impl DesktopIcon {
    fn new(name: &str, path: &str, icon_name: Option<&str>, is_app: bool, is_dir: bool) -> Self {
        Self {
            name: name.to_string(),
            path: path.to_string(),
            icon_name: icon_name.map(|s| s.to_string()),
            is_app,
            is_dir,
        }
    }
}

pub struct IconProvider {
    icon_size: Cell<i32>,
    icon_theme: gtk::IconTheme,
}

impl IconProvider {
    pub fn new(icon_size: i32) -> Self {
        let disp = gdk::Display::default().expect("no default display");
        let icon_theme = gtk::IconTheme::for_display(&disp);
        debug!(
            "IconProvider init: size={}, display={}",
            icon_size,
            disp.name()
        );
        Self {
            icon_size: Cell::new(icon_size),
            icon_theme,
        }
    }

    pub fn set_icon_size(&self, size: i32) {
        self.icon_size.set(size);
    }

    pub fn get_icon_for_path(&self, path: &str) -> DesktopIcon {
        let p = Path::new(path);
        let name = p
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();

        if !p.exists() {
            return DesktopIcon::new(&name, path, Some("text-x-generic"), false, false);
        }
        if p.is_dir() {
            return DesktopIcon::new(&name, path, Some("folder"), false, true);
        }
        if p.extension().and_then(|e| e.to_str()) == Some("desktop") {
            return self.get_desktop_icon(p, &name);
        }
        let mime_icon = self.get_mime_icon(p);
        DesktopIcon::new(&name, path, Some(&mime_icon), false, false)
    }

    fn get_desktop_icon(&self, p: &Path, name: &str) -> DesktopIcon {
        let mut icon_name = "application-x-executable".to_string();
        match std::fs::read_to_string(p) {
            Ok(content) => {
                for line in content.lines() {
                    if let Some(v) = line.strip_prefix("Icon=") {
                        icon_name = v.trim().to_string();
                        debug!(".desktop {}: Icon={}", name, icon_name);
                        break;
                    }
                }
            }
            Err(e) => warn!("Failed to read .desktop {}: {}", name, e),
        }
        DesktopIcon::new(name, &p.to_string_lossy(), Some(&icon_name), true, false)
    }

    fn get_mime_icon(&self, p: &Path) -> String {
        let (content_type, _) = gio::content_type_guess(Some(p), None);
        let content_type = content_type.to_string();
        let category = content_type.split('/').next().unwrap_or("");

        let mut base_icon = match category {
            "image" => "image-x-generic",
            "video" => "video-x-generic",
            "audio" => "audio-x-generic",
            "text" => "text-x-generic",
            "application" => "text-x-generic",
            _ => "text-x-generic",
        };

        base_icon = match content_type.as_str() {
            "application/pdf" => "x-office-document",
            "application/zip" => "package-x-generic",
            "application/x-rar" => "package-x-generic",
            "application/x-tar" => "package-x-generic",
            "application/x-gzip" => "package-x-generic",
            "application/vnd.rar" => "package-x-generic",
            "text/html" => "text-html",
            "text/plain" => "text-x-generic",
            "text/x-python" => "text-x-python",
            "application/json" => "text-x-json",
            _ => base_icon,
        };

        debug!(
            "MIME {} → {} → icon={}",
            p.display(),
            content_type,
            base_icon
        );
        base_icon.to_string()
    }

    pub fn load_icon_paintable(&self, desktop_icon: &DesktopIcon) -> Option<gdk::Paintable> {
        let icon_name = desktop_icon
            .icon_name
            .clone()
            .unwrap_or_else(|| "text-x-generic".to_string());

        // Absolute path to an existing file → load directly as a texture.
        let p = Path::new(&icon_name);
        if p.is_absolute() && p.is_file()
            && let Some(tex) = self.lookup_file(&icon_name) {
                debug!("Icon resolved: {} → {}", desktop_icon.name, icon_name);
                return Some(tex);
            }

        if self.icon_theme.has_icon(&icon_name) {
            let paintable = self.lookup(&icon_name);
            debug!("Icon resolved: {} → {}", desktop_icon.name, icon_name);
            return Some(paintable);
        }

        let fallback = if desktop_icon.is_app {
            "application-x-executable"
        } else {
            "text-x-generic"
        };
        debug!(
            "Icon not in theme: {} (icon_name={}), fallback={}",
            desktop_icon.name, icon_name, fallback
        );
        Some(self.lookup(fallback))
    }

    fn lookup_file(&self, path: &str) -> Option<gdk::Paintable> {
        match gdk::Texture::from_filename(path) {
            Ok(t) => Some(t.upcast()),
            Err(e) => {
                debug!("lookup_file failed for {}: {}", path, e);
                None
            }
        }
    }

    fn lookup(&self, icon_name: &str) -> gdk::Paintable {
        let paintable = self.icon_theme.lookup_icon(
            icon_name,
            &[],
            self.icon_size.get(),
            1,
            gtk::TextDirection::None,
            gtk::IconLookupFlags::empty(),
        );
        paintable.upcast()
    }

    pub fn get_system_icon_names(&self) -> Vec<String> {
        self.icon_theme
            .icon_names()
            .iter()
            .take(100)
            .map(|s| s.to_string())
            .collect()
    }
}
