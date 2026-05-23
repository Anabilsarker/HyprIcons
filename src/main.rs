mod config;
mod desktop_icon_view;
mod desktop_window;
mod icon_provider;
mod positions;
mod watcher;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use clap::Parser;
use relm4::gtk;
use relm4::gtk::prelude::*;
use relm4::gtk::{gdk, gio, glib};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, error, info, warn, Level};
use tracing_subscriber::fmt;

use config::{config_file, load_settings, save_settings, Overrides, Settings};
use desktop_icon_view::Callbacks;
use desktop_window::DesktopWindow;
use watcher::FileWatcher;

const APP_ID: &str = "org.example.DesktopForHypr";

// ---------------------------------------------------------------------------
// gtk4-layer-shell must be loaded before libwayland-client. Re-exec with
// LD_PRELOAD set if not already preloaded.
// ---------------------------------------------------------------------------

fn find_layer_shell_so() -> Option<PathBuf> {
    let candidates = [
        "/usr/lib/libgtk4-layer-shell.so",
        "/usr/lib64/libgtk4-layer-shell.so",
        "/usr/local/lib/libgtk4-layer-shell.so",
        "/usr/lib/x86_64-linux-gnu/libgtk4-layer-shell.so",
        "/usr/lib/aarch64-linux-gnu/libgtk4-layer-shell.so",
    ];
    for c in candidates {
        let p = Path::new(c);
        if p.exists() {
            return Some(p.to_path_buf());
        }
    }
    glob::glob("/usr/lib*/**/libgtk4-layer-shell.so*")
        .ok()?
        .filter_map(Result::ok)
        .next()
}

fn ensure_layer_shell_preloaded() {
    let so = match find_layer_shell_so() {
        Some(p) => p,
        None => return,
    };
    let so = so.to_string_lossy().into_owned();

    let preload = std::env::var("LD_PRELOAD").unwrap_or_default();
    if preload.split(':').any(|p| p == so) {
        return;
    }
    let new_preload = format!("{so}:{preload}");
    let new_preload = new_preload.trim_matches(':');

    let exe = std::env::current_exe().expect("current_exe failed");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let err = Command::new(exe)
        .args(&args)
        .env("LD_PRELOAD", new_preload)
        .exec();
    eprintln!("re-exec failed: {err}");
    std::process::exit(1);
}

// ---------------------------------------------------------------------------
// CLI args
// ---------------------------------------------------------------------------

#[derive(Debug, Parser)]
#[command(
    name = "hypricons",
    version,
    about = "Desktop icons for Hyprland"
)]
struct Args {
    /// Config file path
    #[arg(long)]
    config: Option<String>,

    /// Desktop folder path
    #[arg(long = "desktop-path")]
    desktop_path: Option<String>,

    /// Icon size in pixels
    #[arg(long = "icon-size")]
    icon_size: Option<i32>,

    /// Show hidden files (--show-hidden / --no-show-hidden)
    #[arg(long = "show-hidden", action = clap::ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    show_hidden: Option<bool>,

    /// Launch on single click (--single-click / --no-single-click)
    #[arg(long = "single-click", action = clap::ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    launch_single_click: Option<bool>,

    /// Sort by name/date/type
    #[arg(long = "sort-by", value_parser = ["name", "date", "type"])]
    sort_by: Option<String>,

    /// Number of columns
    #[arg(long)]
    columns: Option<i32>,

    /// Enable debug output
    #[arg(long, action = clap::ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    debug: Option<bool>,

    /// Save current settings to config file
    #[arg(long = "save-config", action = clap::ArgAction::SetTrue)]
    save_config: bool,
}

impl Args {
    fn to_overrides(&self) -> Overrides {
        Overrides {
            config_file: self.config.clone(),
            desktop_path: self.desktop_path.clone(),
            icon_size: self.icon_size,
            show_hidden: self.show_hidden,
            launch_single_click: self.launch_single_click,
            sort_by: self.sort_by.clone(),
            sort_order: None,
            layout: None,
            columns: self.columns,
            theme: None,
            arrange_mode: None,
            debug: self.debug,
        }
    }
}

fn setup_logging(debug: bool) {
    let level = if debug { Level::DEBUG } else { Level::INFO };
    fmt()
        .with_max_level(level)
        .with_target(true)
        .with_writer(std::io::stderr)
        .without_time()
        .init();
}

// ---------------------------------------------------------------------------
// Application — port of main.py::Application
// ---------------------------------------------------------------------------

const CSS: &str = "
window, scrolledwindow, viewport, flowbox, flowboxchild {
    background: transparent;
    background-color: transparent;
}
.desktop-icon,
.desktop-icon * {
    color: white;
    background: transparent;
    background-color: transparent;
}
.desktop-icon label {
    text-shadow: 0px 1px 3px rgba(0,0,0,1), 0px 0px 8px rgba(0,0,0,0.9);
}
.desktop-icon.selected {
    background-color: rgba(80, 140, 220, 0.35);
    border-radius: 8px;
}
.desktop-icon.selected label {
    color: white;
}
.desktop-icon.drop-target {
    background-color: rgba(255, 140, 0, 0.55);
    border-radius: 8px;
}
:drop(active) {
    box-shadow: none;
    outline: none;
    border-color: transparent;
    background-color: transparent;
}
";

struct App {
    gtk_app: gtk::Application,
    windows: RefCell<Vec<Rc<DesktopWindow>>>,
    enabled: Cell<bool>,
    settings: Rc<RefCell<Settings>>,
    under_wayland: bool,
    display: Option<gdk::Display>,
    watcher: RefCell<Option<Rc<FileWatcher>>>,
    hold_guard: RefCell<Option<gio::ApplicationHoldGuard>>,
}

impl App {
    fn new(settings: Settings) -> Rc<Self> {
        let gtk_app = gtk::Application::builder()
            .application_id(APP_ID)
            .flags(gio::ApplicationFlags::FLAGS_NONE)
            .build();

        let display = gdk::Display::default();
        let mut under_wayland = false;
        if let Some(d) = &display {
            let name = d.name().to_string();
            under_wayland = name.to_lowercase().contains("wayland");
            info!("Display: {} (wayland={})", name, under_wayland);
        } else {
            warn!("No display available, running headless");
        }

        let this = Rc::new(Self {
            gtk_app: gtk_app.clone(),
            windows: RefCell::new(Vec::new()),
            enabled: Cell::new(false),
            settings: Rc::new(RefCell::new(settings)),
            under_wayland,
            display: display.clone(),
            watcher: RefCell::new(None),
            hold_guard: RefCell::new(None),
        });

        gtk_app.connect_activate(glib::clone!(
            #[strong]
            this,
            move |_| this.on_activate()
        ));

        if let Some(d) = &display {
            let monitors = d.monitors();
            monitors.connect_items_changed(glib::clone!(
                #[strong]
                this,
                move |_, pos, removed, added| {
                    info!(
                        "Monitors changed: pos={} removed={} added={}",
                        pos, removed, added
                    );
                    let this2 = this.clone();
                    glib::idle_add_local_once(move || this2.recreate_desktop_windows());
                }
            ));
            debug!("Subscribed to monitors items-changed");
        }

        this
    }

    fn run(&self) -> glib::ExitCode {
        let empty: [String; 0] = [];
        self.gtk_app.run_with_args(&empty)
    }

    fn apply_css(&self) {
        let Some(display) = &self.display else { return };
        let provider = gtk::CssProvider::new();
        provider.load_from_string(CSS);
        gtk::style_context_add_provider_for_display(
            display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    fn on_activate(self: &Rc<Self>) {
        info!("Application activate");
        *self.hold_guard.borrow_mut() = Some(self.gtk_app.hold());
        self.apply_css();
        self.log_settings();
        self.desktop_manager(true);
    }

    fn log_settings(&self) {
        let s = self.settings.borrow();
        info!(
            "Settings: desktop_path={} icon_size={} columns={} sort_by={} \
             show_hidden={} single_click={} theme={}",
            s.desktop_path,
            s.icon_size,
            s.columns,
            s.sort_by,
            s.show_hidden,
            s.launch_single_click,
            s.theme
        );
    }

    fn monitors_list(&self) -> Vec<gdk::Monitor> {
        let Some(d) = &self.display else {
            return Vec::new();
        };
        let model = d.monitors();
        let n = model.n_items();
        (0..n)
            .filter_map(|i| model.item(i))
            .filter_map(|o| o.downcast::<gdk::Monitor>().ok())
            .collect()
    }

    fn log_monitors(&self, monitors: &[gdk::Monitor]) {
        info!("Monitors: {}", monitors.len());
        for (i, m) in monitors.iter().enumerate() {
            let r = m.geometry();
            let model = m.model().map(|s| s.to_string()).unwrap_or_else(|| "unknown".into());
            info!(
                "  [{}] {}: {}x{}+{}+{}",
                i,
                model,
                r.width(),
                r.height(),
                r.x(),
                r.y()
            );
        }
    }

    fn make_callbacks(self: &Rc<Self>) -> Callbacks {
        let weak = Rc::downgrade(self);
        Callbacks {
            on_icon_activated: Some(Rc::new(|p: &str| info!("Activated: {}", p))),
            on_settings_changed: Some(Rc::new(move || {
                let Some(app) = weak.upgrade() else { return };
                match save_settings(&app.settings.borrow()) {
                    Ok(_) => info!("Settings saved"),
                    Err(e) => error!("Save settings failed: {}", e),
                }
                let app2 = app.clone();
                glib::idle_add_local_once(move || app2.refresh_all_windows());
            })),
            get_parent_window: None,
        }
    }

    fn create_desktop_window(
        self: &Rc<Self>,
        screen_num: i32,
        screen_name: &str,
    ) -> Rc<DesktopWindow> {
        let w = DesktopWindow::new(
            &self.gtk_app,
            screen_num,
            screen_name,
            self.settings.clone(),
            self.make_callbacks(),
        );

        if screen_num == -1 {
            w.set_geometry_virtual();
        } else {
            let monitors = self.monitors_list();
            if screen_num >= 0 && (screen_num as usize) < monitors.len() {
                w.set_geometry_from_monitor(Some(&monitors[screen_num as usize]));
            } else {
                w.set_geometry_from_monitor(None);
            }
        }

        let desktop_path = self.settings.borrow().desktop_path.clone();
        if Path::new(&desktop_path).exists()
            && let Ok(rd) = std::fs::read_dir(&desktop_path) {
                let files: Vec<String> = rd
                    .filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect();
                info!(
                    "Desktop {}: {} files",
                    if screen_name.is_empty() {
                        screen_num.to_string()
                    } else {
                        screen_name.to_string()
                    },
                    files.len()
                );
                w.update_icons(&files, &desktop_path);
            }
        w
    }

    fn desktop_manager(self: &Rc<Self>, enabled: bool) {
        if enabled {
            if !self.enabled.get() {
                let monitors = self.monitors_list();
                self.log_monitors(&monitors);
                self.start_file_watcher();
                if !self.under_wayland && monitors.len() > 1 {
                    info!("Mode: virtual (non-Wayland multi-monitor)");
                    let w = self.create_desktop_window(-1, "virtual");
                    self.windows.borrow_mut().push(w);
                } else {
                    let n = monitors.len().max(1);
                    info!("Mode: per-monitor ({} windows)", n);
                    for i in 0..n {
                        let name = if i < monitors.len() {
                            format!("monitor-{i}")
                        } else {
                            String::new()
                        };
                        let w = self.create_desktop_window(i as i32, &name);
                        self.windows.borrow_mut().push(w);
                    }
                }
            }
        } else if self.enabled.get() {
            self.stop_file_watcher();
            for w in self.windows.borrow().iter() {
                w.destroy();
            }
            self.windows.borrow_mut().clear();
            info!("Desktop manager stopped");
        }
        self.enabled.set(enabled);
    }

    fn start_file_watcher(self: &Rc<Self>) {
        if self.watcher.borrow().is_some() {
            return;
        }
        let desktop_path = self.settings.borrow().desktop_path.clone();
        if !Path::new(&desktop_path).exists() {
            if let Err(e) = std::fs::create_dir_all(&desktop_path) {
                error!("Failed to create desktop path {}: {}", desktop_path, e);
            } else {
                info!("Created desktop path: {}", desktop_path);
            }
        }

        let weak = Rc::downgrade(self);
        let cb: watcher::Callback = Rc::new(move |event_type: &str, path: &str| {
            debug!("Watcher callback: event={} path={}", event_type, path);
            let Some(app) = weak.upgrade() else { return };
            glib::idle_add_local_once(move || app.refresh_all_windows());
        });

        let fw = FileWatcher::new(&desktop_path, cb);
        if !fw.start() {
            error!("Failed to watch {}", desktop_path);
        }
        *self.watcher.borrow_mut() = Some(fw);
    }

    fn stop_file_watcher(&self) {
        if let Some(fw) = self.watcher.borrow_mut().take() {
            fw.stop();
        }
    }

    fn refresh_all_windows(&self) {
        debug!("Refreshing {} window(s)", self.windows.borrow().len());
        for w in self.windows.borrow().iter() {
            w.refresh_icons();
        }
    }

    fn recreate_desktop_windows(self: &Rc<Self>) {
        if self.enabled.get() {
            self.desktop_manager(false);
            let this = self.clone();
            glib::idle_add_local_once(move || this.desktop_manager(true));
        }
    }
}

fn main() {
    ensure_layer_shell_preloaded();

    let args = Args::parse();
    let overrides = args.to_overrides();
    let settings = load_settings(Some(&overrides));

    setup_logging(settings.debug);

    info!(
        "desktop-for-hypr starting (GTK {}.{}.{})",
        gtk::major_version(),
        gtk::minor_version(),
        gtk::micro_version()
    );
    info!("Config file: {}", config_file().display());
    debug!("args: {:?}", args);

    if args.save_config {
        if let Err(e) = save_settings(&settings) {
            error!("Save config failed: {}", e);
            std::process::exit(1);
        }
        println!("Saved config to {}", config_file().display());
        return;
    }

    gtk::init().expect("failed to init GTK");
    let app = App::new(settings);
    let code = app.run();
    std::process::exit(if code == glib::ExitCode::SUCCESS { 0 } else { 1 });
}
