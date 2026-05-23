use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use crate::config::Settings;
use crate::desktop_icon_view::{Callbacks, DesktopIconView};
use crate::icon_provider::IconProvider;

use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use relm4::gtk;
use relm4::gtk::gdk;
use relm4::gtk::prelude::*;
use tracing::debug;

pub struct DesktopWindow {
    pub window: gtk::ApplicationWindow,
    settings: Rc<RefCell<Settings>>,
    icon_view: DesktopIconView,
}

impl DesktopWindow {
    pub fn new(
        app: &gtk::Application,
        screen_num: i32,
        screen_name: &str,
        settings: Rc<RefCell<Settings>>,
        mut callbacks: Callbacks,
    ) -> Rc<Self> {
        let screen_name = if !screen_name.is_empty() {
            screen_name.to_string()
        } else if screen_num >= 0 {
            format!("monitor-{screen_num}")
        } else {
            "virtual".to_string()
        };

        let title = format!("Desktop {}", screen_name);
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title(&title)
            .decorated(false)
            .build();

        // Layer shell — must init before window realized.
        window.init_layer_shell();
        window.set_layer(Layer::Bottom);
        for edge in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
            window.set_anchor(edge, true);
        }
        window.set_exclusive_zone(0);
        // Start with NO keyboard so the desktop never holds the Wayland
        // keyboard while idle (otherwise apps opened via external keybinds
        // can't take focus). OnDemand is armed only on desktop interaction
        // (see DesktopIconView::ensure_keyboard) and released on focus loss.
        window.set_keyboard_mode(KeyboardMode::None);
        debug!(window = %screen_name, "Layer shell init");

        let icon_size = settings.borrow().icon_size;
        let icon_provider = Rc::new(IconProvider::new(icon_size));

        // get_parent_window callback resolves to this window.
        let win_for_cb = window.clone();
        callbacks.get_parent_window = Some(Rc::new(move || {
            Some(win_for_cb.clone().upcast::<gtk::Window>())
        }));

        let icon_view = DesktopIconView::new(icon_provider, settings.clone(), callbacks);

        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Never);
        scrolled.set_hexpand(true);
        scrolled.set_vexpand(true);
        scrolled.set_child(Some(&icon_view));

        window.set_child(Some(&scrolled));
        debug!(screen = %screen_name, "DesktopWindow created");

        Rc::new(Self {
            window,
            settings,
            icon_view,
        })
    }

    pub fn set_geometry_from_monitor(&self, monitor: Option<&gdk::Monitor>) {
        match monitor {
            None => {
                self.window.present();
            }
            Some(m) => {
                let r = m.geometry();
                debug!(
                    "Monitor geometry: {}x{}+{}+{}",
                    r.width(),
                    r.height(),
                    r.x(),
                    r.y()
                );
                self.window.set_monitor(Some(m));
                self.window.present();
                debug!("Layer shell active: {}", self.window.is_layer_window());
            }
        }
    }

    pub fn set_geometry_virtual(&self) {
        // No monitor set → layer shell fills primary; anchors cover it fully.
        self.window.present();
    }

    pub fn refresh_icons(&self) {
        let path = self.settings.borrow().desktop_path.clone();
        if Path::new(&path).exists()
            && let Ok(rd) = std::fs::read_dir(&path)
        {
            let files: Vec<String> = rd
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            debug!("refresh_icons: {} files", files.len());
            self.icon_view.update_icons(&files, &path);
        }
    }

    pub fn update_icons(&self, files: &[String], desktop_path: &str) {
        self.icon_view.update_icons(files, desktop_path);
    }

    pub fn destroy(&self) {
        self.window.destroy();
    }
}
