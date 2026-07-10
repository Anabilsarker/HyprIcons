use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk4_layer_shell::{KeyboardMode, LayerShell};
use relm4::gtk;
use relm4::gtk::prelude::*;
use relm4::gtk::subclass::prelude::*;
use relm4::gtk::{gdk, gio, glib, graphene, gsk, pango};
use tracing::{debug, error, info, warn};

use crate::config::Settings;
use crate::icon_provider::{DesktopIcon, IconProvider};
use crate::positions::Positions;

pub const ICON_GAP_X: i32 = 4;
pub const ICON_GAP_Y: i32 = 4;

fn item_size(icon_size: i32) -> (i32, i32) {
    (icon_size + 48, icon_size + 52)
}

fn cell_size(icon_size: i32) -> (i32, i32) {
    let (iw, ih) = item_size(icon_size);
    (iw + ICON_GAP_X, ih + ICON_GAP_Y)
}

fn truncate(text: &str, limit: usize) -> String {
    // Avoid collecting into a Vec<char> on the common (within-limit) path.
    match text.char_indices().nth(limit) {
        None => text.to_string(),
        Some((byte_idx, _)) => {
            let mut s = String::with_capacity(byte_idx + 3);
            s.push_str(&text[..byte_idx]);
            s.push('…');
            s
        }
    }
}

// ---------------------------------------------------------------------------
// IconItem : Gtk.Box
// ---------------------------------------------------------------------------

mod icon_item_imp {
    use super::*;

    #[derive(Default)]
    pub struct IconItem {
        pub file_path: RefCell<String>,
        pub filename: RefCell<String>,
        pub full_label: RefCell<String>,
        pub image: RefCell<Option<gtk::Image>>,
        pub label: RefCell<Option<gtk::Label>>,
        pub click_pending: Cell<bool>,
        pub collapse_on_release: Cell<bool>,
        // Special items (Home, Trash) are synthetic — not files on the
        // desktop. They can't be renamed/deleted/dragged, and may open a
        // URI (e.g. trash:///) instead of their backing path.
        pub special: Cell<bool>,
        pub open_uri: RefCell<Option<String>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for IconItem {
        const NAME: &'static str = "HypriconsIconItem";
        type Type = super::IconItem;
        type ParentType = gtk::Box;
    }

    impl ObjectImpl for IconItem {}
    impl WidgetImpl for IconItem {}
    impl BoxImpl for IconItem {}
}

glib::wrapper! {
    pub struct IconItem(ObjectSubclass<icon_item_imp::IconItem>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Orientable, gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl IconItem {
    pub fn new(_icon_name: &str, label_text: &str, file_path: &str, icon_size: i32) -> Self {
        let obj: Self = glib::Object::builder()
            .property("orientation", gtk::Orientation::Vertical)
            .property("spacing", 0)
            .build();

        let imp = obj.imp();
        *imp.file_path.borrow_mut() = file_path.to_string();
        *imp.filename.borrow_mut() = Path::new(file_path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        *imp.full_label.borrow_mut() = label_text.to_string();

        obj.add_css_class("desktop-icon");
        obj.set_margin_top(0);
        obj.set_margin_bottom(0);
        obj.set_margin_start(0);
        obj.set_margin_end(0);
        obj.set_halign(gtk::Align::Center);
        obj.set_valign(gtk::Align::Start);

        let (item_w, _) = item_size(icon_size);
        obj.set_size_request(item_w, -1);

        let image = gtk::Image::new();
        image.set_size_request(icon_size, icon_size);
        image.set_icon_size(gtk::IconSize::Large);
        image.set_valign(gtk::Align::End);
        obj.append(&image);
        *imp.image.borrow_mut() = Some(image);

        let label = gtk::Label::new(Some(&truncate(label_text, 12)));
        label.set_justify(gtk::Justification::Center);
        label.set_max_width_chars(12);
        label.set_width_chars(12);
        label.set_halign(gtk::Align::Center);
        label.set_valign(gtk::Align::Start);
        label.set_wrap(false);
        label.set_ellipsize(pango::EllipsizeMode::End);
        obj.append(&label);
        *imp.label.borrow_mut() = Some(label);

        obj.set_can_focus(true);
        obj.set_focusable(true); // GTK4: required for grab_focus / key events
        obj
    }

    pub fn file_path(&self) -> String {
        self.imp().file_path.borrow().clone()
    }

    pub fn filename(&self) -> String {
        self.imp().filename.borrow().clone()
    }

    /// Override the positions/layout key (special items use a stable label
    /// instead of the backing path's basename).
    pub fn set_filename(&self, name: &str) {
        *self.imp().filename.borrow_mut() = name.to_string();
    }

    pub fn set_special(&self, special: bool) {
        self.imp().special.set(special);
    }

    pub fn is_special(&self) -> bool {
        self.imp().special.get()
    }

    pub fn set_open_uri(&self, uri: &str) {
        *self.imp().open_uri.borrow_mut() = Some(uri.to_string());
    }

    pub fn open_uri(&self) -> Option<String> {
        self.imp().open_uri.borrow().clone()
    }

    pub fn set_icon_paintable(&self, paintable: &impl IsA<gdk::Paintable>) {
        if let Some(img) = self.imp().image.borrow().as_ref() {
            img.set_paintable(Some(paintable));
        }
    }

    pub fn paintable(&self) -> Option<gdk::Paintable> {
        self.imp()
            .image
            .borrow()
            .as_ref()
            .and_then(|i| i.paintable())
    }

    pub fn set_selected(&self, selected: bool) {
        let imp = self.imp();
        let label = imp.label.borrow();
        let Some(label) = label.as_ref() else { return };
        if selected {
            self.add_css_class("selected");
            label.set_text(&imp.full_label.borrow());
            label.set_ellipsize(pango::EllipsizeMode::None);
            label.set_wrap(true);
            label.set_wrap_mode(pango::WrapMode::WordChar);
            label.set_lines(-1);
        } else {
            self.remove_css_class("selected");
            label.set_wrap(false);
            label.set_ellipsize(pango::EllipsizeMode::End);
            label.set_text(&truncate(&imp.full_label.borrow(), 12));
        }
    }
}

// ---------------------------------------------------------------------------
// DesktopIconView : Gtk.Fixed
// ---------------------------------------------------------------------------

/// (filename, offset-x, offset-y) of each icon in a multi-icon drag.
pub type DragGroup = Vec<(String, i32, i32)>;

pub type IconActivatedFn = Rc<dyn Fn(&str)>;
pub type SettingsChangedFn = Rc<dyn Fn()>;
pub type ParentWindowFn = Rc<dyn Fn() -> Option<gtk::Window>>;

#[derive(Clone, Default)]
pub struct Callbacks {
    pub on_icon_activated: Option<IconActivatedFn>,
    pub on_settings_changed: Option<SettingsChangedFn>,
    pub get_parent_window: Option<ParentWindowFn>,
}

mod view_imp {
    use super::*;

    pub struct DesktopIconView {
        pub icon_provider: RefCell<Option<Rc<IconProvider>>>,
        pub settings: RefCell<Option<Rc<RefCell<Settings>>>>,
        pub callbacks: RefCell<Callbacks>,
        pub icons: RefCell<Vec<IconItem>>,
        pub icons_by_name: RefCell<HashMap<String, IconItem>>,
        pub selected: RefCell<Option<IconItem>>,
        pub selection: RefCell<HashSet<IconItem>>,
        pub positions: RefCell<Positions>,
        pub desktop_path: RefCell<String>,
        pub rubber_active: Cell<bool>,
        pub rubber_start: Cell<(f64, f64)>,
        pub rubber_cur: Cell<(f64, f64)>,
        pub drag_group: RefCell<Option<DragGroup>>,
        pub drop_hl: RefCell<Option<IconItem>>,
        // The item currently being dragged from this view. Set on drag
        // prepare, cleared on drag end. Lets on_drop recognise an internal
        // (reposition) drag without depending on fragile DnD content-type
        // negotiation — critical for special items, which carry no file URI.
        pub drag_item: RefCell<Option<IconItem>>,
    }

    impl Default for DesktopIconView {
        fn default() -> Self {
            Self {
                icon_provider: RefCell::new(None),
                settings: RefCell::new(None),
                callbacks: RefCell::new(Callbacks::default()),
                icons: RefCell::new(Vec::new()),
                icons_by_name: RefCell::new(HashMap::new()),
                selected: RefCell::new(None),
                selection: RefCell::new(HashSet::new()),
                positions: RefCell::new(Positions::new()),
                desktop_path: RefCell::new(String::new()),
                rubber_active: Cell::new(false),
                rubber_start: Cell::new((0.0, 0.0)),
                rubber_cur: Cell::new((0.0, 0.0)),
                drag_group: RefCell::new(None),
                drop_hl: RefCell::new(None),
                drag_item: RefCell::new(None),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DesktopIconView {
        const NAME: &'static str = "HypriconsDesktopIconView";
        type Type = super::DesktopIconView;
        type ParentType = gtk::Fixed;
    }

    impl ObjectImpl for DesktopIconView {}
    impl FixedImpl for DesktopIconView {}

    impl WidgetImpl for DesktopIconView {
        // do_snapshot override — draw children (parent) then rubber-band overlay.
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            self.parent_snapshot(snapshot);
            if !self.rubber_active.get() {
                return;
            }
            let (x0, y0) = self.rubber_start.get();
            let (x1, y1) = self.rubber_cur.get();
            let x = x0.min(x1) as f32;
            let y = y0.min(y1) as f32;
            let w = (x1 - x0).abs() as f32;
            let h = (y1 - y0).abs() as f32;
            if w < 1.0 || h < 1.0 {
                return;
            }
            let rect = graphene::Rect::new(x, y, w, h);
            let fill = gdk::RGBA::new(0.31, 0.55, 0.86, 0.25);
            snapshot.append_color(&fill, &rect);
            let border = gdk::RGBA::new(0.31, 0.55, 0.86, 0.9);
            let rounded = gsk::RoundedRect::from_rect(rect, 0.0);
            snapshot.append_border(
                &rounded,
                &[1.0, 1.0, 1.0, 1.0],
                &[border, border, border, border],
            );
        }
    }
}

glib::wrapper! {
    pub struct DesktopIconView(ObjectSubclass<view_imp::DesktopIconView>)
        @extends gtk::Fixed, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl DesktopIconView {
    pub fn new(
        icon_provider: Rc<IconProvider>,
        settings: Rc<RefCell<Settings>>,
        callbacks: Callbacks,
    ) -> Self {
        let obj: Self = glib::Object::builder().build();
        let imp = obj.imp();
        let desktop_path = settings.borrow().desktop_path.clone();
        *imp.icon_provider.borrow_mut() = Some(icon_provider);
        *imp.settings.borrow_mut() = Some(settings);
        *imp.callbacks.borrow_mut() = callbacks;
        *imp.desktop_path.borrow_mut() = desktop_path;

        obj.set_hexpand(true);
        obj.set_vexpand(true);
        obj.set_can_focus(true);
        obj.set_focusable(true);

        // Empty-area click (any button)
        let empty_click = gtk::GestureClick::new();
        empty_click.set_button(0);
        empty_click.connect_pressed(glib::clone!(
            #[weak]
            obj,
            move |g, n, x, y| obj.on_empty_click(g, n, x, y)
        ));
        obj.add_controller(empty_click);

        // Rubber-band drag (primary)
        let rubber = gtk::GestureDrag::new();
        rubber.set_button(gdk::BUTTON_PRIMARY);
        rubber.connect_drag_begin(glib::clone!(
            #[weak]
            obj,
            move |_, x, y| obj.on_rubber_begin(x, y)
        ));
        rubber.connect_drag_update(glib::clone!(
            #[weak]
            obj,
            move |_, dx, dy| obj.on_rubber_update(dx, dy)
        ));
        rubber.connect_drag_end(glib::clone!(
            #[weak]
            obj,
            move |_, _, _| obj.on_rubber_end()
        ));
        obj.add_controller(rubber);

        // Drop target: STRING (internal) + FileList + Gio.File (external)
        let drop_target = gtk::DropTarget::new(
            glib::Type::STRING,
            gdk::DragAction::MOVE | gdk::DragAction::COPY,
        );
        // STRING first: internal drags (icon name / uri-list text) negotiate
        // as STRING and reach on_drop intact. If STRING were after FileList,
        // GTK would coerce a bare name token into a null GdkFileList and the
        // reposition signal would be lost. External file drops that can't be
        // delivered as STRING fall through to FileList / File.
        drop_target.set_types(&[
            glib::Type::STRING,
            gdk::FileList::static_type(),
            gio::File::static_type(),
        ]);
        drop_target.connect_drop(glib::clone!(
            #[weak]
            obj,
            #[upgrade_or]
            false,
            move |_, value, x, y| obj.on_drop(value, x, y)
        ));
        // Highlight the folder under the pointer while dragging.
        drop_target.connect_motion(glib::clone!(
            #[weak]
            obj,
            #[upgrade_or]
            gdk::DragAction::empty(),
            move |_, x, y| {
                // A special icon being dragged can only be repositioned, so
                // don't highlight folder/special drop targets for it.
                let dragging_special = obj
                    .imp()
                    .drag_item
                    .borrow()
                    .as_ref()
                    .is_some_and(|i| i.is_special());
                let target = if dragging_special {
                    None
                } else {
                    obj.folder_at(x, y).or_else(|| obj.special_at(x, y))
                };
                obj.set_drop_highlight(target.as_ref());
                gdk::DragAction::MOVE
            }
        ));
        drop_target.connect_leave(glib::clone!(
            #[weak]
            obj,
            move |_| obj.set_drop_highlight(None)
        ));
        obj.add_controller(drop_target);

        obj.connect_notify_local(
            Some("default-width"),
            glib::clone!(
                #[weak]
                obj,
                move |_, _| obj.maybe_reflow()
            ),
        );

        // The first layout in update_icons runs before the window is
        // allocated (usable area unknown → everything in one column). Once
        // the frame clock starts and the area is known, relayout once and
        // stop the callback. size_allocate handles later size changes.
        obj.add_tick_callback(|view, _clock| {
            if view.area_height() > 0 {
                debug!("tick relayout: area_h={}", view.area_height());
                view.layout_all();
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });

        // View-level keys: arrow navigation + clipboard (Ctrl+C/X/V).
        let nav = gtk::EventControllerKey::new();
        nav.connect_key_pressed(glib::clone!(
            #[weak]
            obj,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, keyval, _, state| obj.on_view_key(keyval, state)
        ));
        obj.add_controller(nav);

        // Arm OnDemand while the pointer is over the desktop, so a click
        // here is granted keyboard focus (Hyprland decides keyboard at
        // click time — too late if armed mid-click). Drop it when the
        // pointer leaves so the desktop doesn't hog the keyboard.
        let motion = gtk::EventControllerMotion::new();
        motion.connect_enter(glib::clone!(
            #[weak]
            obj,
            move |_, _, _| obj.set_keyboard(KeyboardMode::OnDemand)
        ));
        motion.connect_leave(glib::clone!(
            #[weak]
            obj,
            move |_| obj.set_keyboard(KeyboardMode::None)
        ));
        obj.add_controller(motion);

        // Also release the keyboard the moment the desktop loses focus
        // (e.g. another app, opened via a system keybind, takes focus)
        // even if the pointer is still over the desktop.
        let focus = gtk::EventControllerFocus::new();
        focus.connect_leave(glib::clone!(
            #[weak]
            obj,
            move |_| obj.set_keyboard(KeyboardMode::None)
        ));
        obj.add_controller(focus);

        obj
    }

    // ---- helpers to reach shared state ----

    fn settings(&self) -> Rc<RefCell<Settings>> {
        self.imp()
            .settings
            .borrow()
            .clone()
            .expect("settings unset")
    }

    fn provider(&self) -> Rc<IconProvider> {
        self.imp()
            .icon_provider
            .borrow()
            .clone()
            .expect("provider unset")
    }

    fn emit_settings_changed(&self) {
        let cb = self.imp().callbacks.borrow().on_settings_changed.clone();
        if let Some(cb) = cb {
            cb();
        }
    }

    fn parent_window(&self) -> Option<gtk::Window> {
        let cb = self.imp().callbacks.borrow().get_parent_window.clone();
        cb.and_then(|f| f())
    }

    // ---- public API ----

    pub fn update_icons(&self, files: &[String], desktop_path: &str) {
        debug!("update_icons: {} files from {}", files.len(), desktop_path);
        let imp = self.imp();
        *imp.desktop_path.borrow_mut() = desktop_path.to_string();

        for item in imp.icons.borrow().iter() {
            self.remove(item);
        }
        imp.icons.borrow_mut().clear();
        imp.icons_by_name.borrow_mut().clear();
        *imp.selected.borrow_mut() = None;

        let settings = self.settings();
        let (icon_size, show_hidden) = {
            let s = settings.borrow();
            (s.icon_size, s.show_hidden)
        };

        let sorted = self.sort_files(files);
        let mut loaded = 0;
        let mut failed = 0;
        let mut visible: Vec<String> = Vec::new();
        let provider = self.provider();

        self.add_special_icons(icon_size, &mut visible);

        for filename in &sorted {
            let full_path = Path::new(desktop_path).join(filename);
            if !full_path.exists() {
                continue;
            }
            if filename.starts_with('.') && !show_hidden {
                continue;
            }
            let full_path_s = full_path.to_string_lossy().into_owned();
            let desktop_icon = provider.get_icon_for_path(&full_path_s);
            let paintable = provider.load_icon_paintable(&desktop_icon);

            match &paintable {
                Some(_) => loaded += 1,
                None => {
                    failed += 1;
                    warn!(
                        "No icon for: {} (icon_name={:?})",
                        filename, desktop_icon.icon_name
                    );
                }
            }

            let item = IconItem::new(
                desktop_icon
                    .icon_name
                    .as_deref()
                    .unwrap_or("text-x-generic"),
                filename,
                &full_path_s,
                icon_size,
            );
            if let Some(p) = paintable {
                item.set_icon_paintable(&p);
            }

            self.attach_item_controllers(&item);
            imp.icons.borrow_mut().push(item.clone());
            imp.icons_by_name
                .borrow_mut()
                .insert(filename.clone(), item);
            visible.push(filename.clone());
        }

        imp.positions.borrow_mut().prune(&visible);
        self.layout_all();
        info!("Icons loaded: {} ok, {} missing icon", loaded, failed);
    }

    /// Prepend the active user's Home and Trash icons (when enabled). These
    /// are synthetic — they live before the file icons at grid index 0/1.
    fn add_special_icons(&self, icon_size: i32, visible: &mut Vec<String>) {
        let (show_home, show_trash) = {
            let s = self.settings();
            let s = s.borrow();
            (s.show_home, s.show_trash)
        };
        let imp = self.imp();
        let provider = self.provider();

        let mut add = |label: &str, icon_name: &str, path: String, uri: Option<&str>| {
            let di = DesktopIcon {
                name: label.to_string(),
                path: path.clone(),
                icon_name: Some(icon_name.to_string()),
                is_app: false,
                is_dir: true,
            };
            let paintable = provider.load_icon_paintable(&di);
            let item = IconItem::new(icon_name, label, &path, icon_size);
            item.set_filename(label);
            item.set_special(true);
            if let Some(u) = uri {
                item.set_open_uri(u);
            }
            if let Some(p) = paintable {
                item.set_icon_paintable(&p);
            }
            self.attach_item_controllers(&item);
            imp.icons.borrow_mut().push(item.clone());
            imp.icons_by_name
                .borrow_mut()
                .insert(label.to_string(), item);
            visible.push(label.to_string());
        };

        if show_home
            && let Some(home) = dirs::home_dir()
        {
            add("Home", "user-home", home.to_string_lossy().into_owned(), None);
        }
        if show_trash {
            let icon = if trash_has_files() {
                "user-trash-full"
            } else {
                "user-trash"
            };
            add("Trash", icon, trash_files_dir(), Some("trash:///"));
        }
    }

    pub fn refresh(&self) {
        let path = self.imp().desktop_path.borrow().clone();
        if Path::new(&path).is_dir()
            && let Ok(rd) = std::fs::read_dir(&path)
        {
            let files: Vec<String> = rd
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            self.update_icons(&files, &path);
        }
    }

    /// compat shim — columns now from settings on layout
    pub fn set_max_children_per_line(&self, _n: i32) {}

    // ---- sorting / layout ----

    fn sort_files(&self, files: &[String]) -> Vec<String> {
        let settings = self.settings();
        let (sort_by, reverse) = {
            let s = settings.borrow();
            (s.sort_by.clone(), s.sort_order == "desc")
        };
        let dir = self.imp().desktop_path.borrow().clone();
        let mut v = files.to_vec();
        match sort_by.as_str() {
            "name" => v.sort(),
            // sort_by_cached_key: key fn runs once per element, not O(n log n)
            // times — one fs::metadata syscall / extension parse per file.
            "date" => v.sort_by_cached_key(|f| {
                std::fs::metadata(Path::new(&dir).join(f))
                    .and_then(|m| m.modified())
                    .ok()
            }),
            "type" => v.sort_by_cached_key(|f| {
                Path::new(f)
                    .extension()
                    .map(|e| e.to_string_lossy().to_lowercase())
                    .unwrap_or_default()
            }),
            _ => v.sort(),
        }
        if reverse {
            v.reverse();
        }
        v
    }

    const GRID_MARGIN: i32 = 8;

    /// Usable desktop height. Read from a container ancestor (viewport /
    /// ScrolledWindow / window) whose allocation is the stable viewport size,
    /// NOT this Fixed's content-driven natural height (which would feed back
    /// into the grid). Returns the first ancestor reporting a real height.
    fn area_height(&self) -> i32 {
        let mut w = self.parent();
        while let Some(widget) = w {
            let h = widget.height();
            if h > 0 {
                return h;
            }
            w = widget.parent();
        }
        0
    }

    /// Rows that fit in the usable desktop height — the column-wise grid
    /// fills a column top-to-bottom before moving to the next column.
    fn rows(&self) -> i32 {
        let icon_size = self.settings().borrow().icon_size;
        let (_, ch) = cell_size(icon_size);
        let h = self.area_height();
        if h <= 0 || ch <= 0 {
            // Not realized yet — fall back to one column; size_allocate
            // reflows once the real height is known.
            return 1.max(self.icons_count());
        }
        ((h - 2 * Self::GRID_MARGIN) / ch).max(1)
    }

    fn icons_count(&self) -> i32 {
        self.imp().icons.borrow().len() as i32
    }

    /// Column-major grid placement: index 0 top-left, filling down each
    /// column before starting the next column to the right.
    fn grid_position(&self, index: i32) -> (i32, i32) {
        let rows = self.rows();
        let icon_size = self.settings().borrow().icon_size;
        let (cw, ch) = cell_size(icon_size);
        let col = index / rows;
        let row = index % rows;
        (
            Self::GRID_MARGIN + col * cw,
            Self::GRID_MARGIN + row * ch,
        )
    }

    fn layout_all(&self) {
        let imp = self.imp();
        let mode = self.settings().borrow().arrange_mode.clone();
        // Only persist seeded grid positions once the usable area is known —
        // otherwise a pre-allocation pass (area 0 → single column) would
        // overwrite the user's saved free-mode layout with bogus positions.
        let area_known = self.area_height() > 0;
        debug!("layout_all: mode={} area_known={}", mode, area_known);
        let icons = imp.icons.borrow().clone();
        for (i, item) in icons.iter().enumerate() {
            let pos = if mode == "free" {
                // Bind into a local so the immutable borrow is dropped
                // before the borrow_mut() below (no double-borrow panic).
                let existing = imp.positions.borrow().get(&item.filename());
                match existing {
                    Some(p) => p,
                    None => {
                        let p = self.grid_position(i as i32);
                        imp.positions.borrow_mut().set(&item.filename(), p.0, p.1);
                        p
                    }
                }
            } else {
                self.grid_position(i as i32)
            };
            if item.parent().is_none() {
                self.put(item, pos.0 as f64, pos.1 as f64);
            } else {
                self.move_(item, pos.0 as f64, pos.1 as f64);
            }
        }
        if mode == "free" {
            imp.positions.borrow().save();
        }
    }

    fn maybe_reflow(&self) {
        if self.settings().borrow().arrange_mode == "auto" {
            self.layout_all();
        }
    }

    // ---- per-item gestures ----

    fn attach_item_controllers(&self, item: &IconItem) {
        item.imp().click_pending.set(false);

        let click = gtk::GestureClick::new();
        click.set_button(0);
        click.connect_pressed(glib::clone!(
            #[weak(rename_to = view)]
            self,
            #[weak]
            item,
            move |g, n, x, y| view.on_item_pressed(g, n, x, y, &item)
        ));
        click.connect_released(glib::clone!(
            #[weak(rename_to = view)]
            self,
            #[weak]
            item,
            move |g, n, x, y| view.on_item_released(g, n, x, y, &item)
        ));
        click.connect_cancel(glib::clone!(
            #[weak]
            item,
            move |_, _| item.imp().click_pending.set(false)
        ));
        item.add_controller(click);

        let key_ctrl = gtk::EventControllerKey::new();
        key_ctrl.connect_key_pressed(glib::clone!(
            #[weak(rename_to = view)]
            self,
            #[weak]
            item,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, keyval, _, _| view.on_item_key(keyval, &item)
        ));
        item.add_controller(key_ctrl);

        let drag_src = gtk::DragSource::new();
        drag_src.set_actions(gdk::DragAction::MOVE | gdk::DragAction::COPY);
        drag_src.connect_prepare(glib::clone!(
            #[weak(rename_to = view)]
            self,
            #[weak]
            item,
            #[upgrade_or]
            None,
            move |_, _, _| view.on_drag_prepare(&item)
        ));
        drag_src.connect_drag_begin(glib::clone!(
            #[weak(rename_to = view)]
            self,
            #[weak]
            item,
            move |src, _| view.on_drag_begin(src, &item)
        ));
        drag_src.connect_drag_end(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_, _, _| {
                let imp = view.imp();
                *imp.drag_item.borrow_mut() = None;
                *imp.drag_group.borrow_mut() = None;
            }
        ));
        item.add_controller(drag_src);
    }

    fn on_item_pressed(
        &self,
        gesture: &gtk::GestureClick,
        _n: i32,
        x: f64,
        y: f64,
        item: &IconItem,
    ) {
        self.ensure_keyboard();
        let button = gesture.current_button();
        if button == gdk::BUTTON_SECONDARY {
            item.imp().click_pending.set(false);
            if !self.imp().selection.borrow().contains(item) {
                self.select(Some(item));
            }
            self.show_item_menu(item, x, y);
            return;
        }
        if button == gdk::BUTTON_PRIMARY {
            let multi = {
                let sel = self.imp().selection.borrow();
                sel.contains(item) && sel.len() > 1
            };
            if multi {
                item.imp().collapse_on_release.set(true);
            } else {
                self.select(Some(item));
                item.imp().collapse_on_release.set(false);
            }
            item.imp().click_pending.set(true);
        }
    }

    fn on_item_released(
        &self,
        gesture: &gtk::GestureClick,
        n_press: i32,
        _x: f64,
        _y: f64,
        item: &IconItem,
    ) {
        if !item.imp().click_pending.get() {
            return;
        }
        item.imp().click_pending.set(false);
        if gesture.current_button() != gdk::BUTTON_PRIMARY {
            return;
        }
        if item.imp().collapse_on_release.get() {
            item.imp().collapse_on_release.set(false);
            self.select(Some(item));
            return;
        }
        let launch_single = self.settings().borrow().launch_single_click;
        let launch_at = if launch_single { 1 } else { 2 };
        if n_press == launch_at {
            self.launch_item(item);
        }
    }

    fn on_item_key(&self, keyval: gdk::Key, item: &IconItem) -> glib::Propagation {
        if keyval == gdk::Key::Return {
            self.launch_item(item);
            return glib::Propagation::Stop;
        }
        if keyval == gdk::Key::Delete {
            self.delete_item(item);
            return glib::Propagation::Stop;
        }
        if keyval == gdk::Key::F2 {
            self.rename_item(item);
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    }

    fn notify_activated(&self, path: &str) {
        let cb = self.imp().callbacks.borrow().on_icon_activated.clone();
        if let Some(cb) = cb {
            cb(path);
        }
    }

    fn launch_item(&self, item: &IconItem) {
        // Special items may open a URI (e.g. Trash → trash:///) rather than
        // their backing filesystem path.
        if let Some(uri) = item.open_uri() {
            info!("Launching URI: {}", uri);
            if open_uri_default(&uri) {
                self.notify_activated(&uri);
                self.release_focus();
            }
            return;
        }

        let path = item.file_path();
        info!("Launching: {}", path);
        let ext = Path::new(&path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase());

        let launched = if ext.as_deref() == Some("desktop") {
            // .desktop → run its Exec=, not "open the file".
            launch_desktop_entry(&path)
        } else if ext.as_deref() == Some("iso") {
            // .iso → mount (or reuse existing mount) then open mountpoint.
            self.open_iso(&path)
        } else {
            // Else → default app for its MIME via GFile (no URI → browser),
            // text forced to a text editor.
            open_with_default(&path)
        };

        if launched {
            self.notify_activated(&path);
            // Release keyboard focus from our layer-shell surface so the
            // compositor moves focus to the launched app's window.
            self.release_focus();
        }
    }

    fn set_keyboard(&self, mode: KeyboardMode) {
        if let Some(win) = self.parent_window()
            && win.keyboard_mode() != mode
        {
            win.set_keyboard_mode(mode);
        }
    }

    /// Drop GTK focus + layer-shell keyboard focus so a newly launched
    /// window receives focus instead of the desktop surface.
    fn release_focus(&self) {
        let imp = self.imp();
        if let Some(sel) = imp.selected.borrow().as_ref() {
            sel.set_selected(false);
        }
        *imp.selected.borrow_mut() = None;
        imp.selection.borrow_mut().clear();
        if let Some(win) = self.parent_window() {
            gtk::prelude::GtkWindowExt::set_focus(&win, None::<&gtk::Widget>);
        }
        // Relinquish Wayland keyboard focus so the compositor hands it
        // to the launched window immediately (not on next pointer move).
        self.set_keyboard(KeyboardMode::None);
    }

    /// Arm on-demand keyboard so desktop shortcuts work while the user is
    /// interacting with the desktop. Released again on focus loss.
    fn ensure_keyboard(&self) {
        self.set_keyboard(KeyboardMode::OnDemand);
    }

    fn open_iso(&self, iso: &str) -> bool {
        match mount_iso(iso) {
            Ok(mp) => {
                info!("ISO {} mounted at {}", iso, mp);
                open_with_default(&mp)
            }
            Err(e) => {
                error!("Mount ISO {} failed: {}", iso, e);
                self.alert("Mount failed", &e);
                false
            }
        }
    }

    fn iso_mount_action(&self, iso: &str) {
        match mount_iso(iso) {
            Ok(mp) => info!("ISO {} mounted at {}", iso, mp),
            Err(e) => {
                error!("Mount ISO {} failed: {}", iso, e);
                self.alert("Mount failed", &e);
            }
        }
    }

    fn iso_unmount_action(&self, iso: &str) {
        match unmount_iso(iso) {
            Ok(_) => info!("ISO {} unmounted", iso),
            Err(e) => {
                error!("Unmount ISO {} failed: {}", iso, e);
                self.alert("Unmount failed", &e);
            }
        }
    }

    fn alert(&self, message: &str, detail: &str) {
        let dialog = gtk::AlertDialog::builder()
            .message(message)
            .detail(detail)
            .buttons(["Close"])
            .default_button(0)
            .build();
        dialog.show(self.parent_window().as_ref());
    }

    fn select(&self, item: Option<&IconItem>) {
        let imp = self.imp();
        {
            let sel = imp.selection.borrow().clone();
            for it in sel.iter() {
                if Some(it) != item {
                    it.set_selected(false);
                }
            }
        }
        imp.selection.borrow_mut().clear();
        {
            let cur = imp.selected.borrow().clone();
            if let Some(cur) = cur
                && Some(&cur) != item
            {
                cur.set_selected(false);
            }
        }
        *imp.selected.borrow_mut() = item.cloned();
        if let Some(item) = item {
            imp.selection.borrow_mut().insert(item.clone());
            item.set_selected(true);
            item.grab_focus();
        }
    }

    // ---- keyboard navigation + clipboard ----

    fn selected_paths(&self) -> Vec<String> {
        let imp = self.imp();
        let sel = imp.selection.borrow();
        if !sel.is_empty() {
            return sel.iter().map(|i| i.file_path()).collect();
        }
        imp.selected
            .borrow()
            .as_ref()
            .map(|i| vec![i.file_path()])
            .unwrap_or_default()
    }

    fn on_view_key(&self, keyval: gdk::Key, state: gdk::ModifierType) -> glib::Propagation {
        if state.contains(gdk::ModifierType::CONTROL_MASK) {
            match keyval {
                gdk::Key::c | gdk::Key::C => self.copy_selection(false),
                gdk::Key::x | gdk::Key::X => self.copy_selection(true),
                gdk::Key::v | gdk::Key::V => self.paste_clipboard(),
                _ => return glib::Propagation::Proceed,
            }
            return glib::Propagation::Stop;
        }
        let dir = match keyval {
            gdk::Key::Left => (-1, 0),
            gdk::Key::Right => (1, 0),
            gdk::Key::Up => (0, -1),
            gdk::Key::Down => (0, 1),
            _ => return glib::Propagation::Proceed,
        };
        self.move_selection(dir.0, dir.1);
        glib::Propagation::Stop
    }

    fn move_selection(&self, dx: i32, dy: i32) {
        let icons = self.imp().icons.borrow().clone();
        if icons.is_empty() {
            return;
        }
        let cur = self.imp().selected.borrow().clone();
        let Some(cur) = cur else {
            self.select(icons.first());
            return;
        };
        let Some(cb) = cur.compute_bounds(self) else {
            return;
        };
        let (ccx, ccy) = (
            (cb.x() + cb.width() / 2.0) as f64,
            (cb.y() + cb.height() / 2.0) as f64,
        );

        let mut best: Option<(f64, IconItem)> = None;
        for it in &icons {
            if it == &cur {
                continue;
            }
            let Some(b) = it.compute_bounds(self) else {
                continue;
            };
            let (cx, cy) = (
                (b.x() + b.width() / 2.0) as f64,
                (b.y() + b.height() / 2.0) as f64,
            );
            let (main, off) = match (dx, dy) {
                (1, 0) => (cx - ccx, (cy - ccy).abs()),
                (-1, 0) => (ccx - cx, (cy - ccy).abs()),
                (0, 1) => (cy - ccy, (cx - ccx).abs()),
                (0, -1) => (ccy - cy, (cx - ccx).abs()),
                _ => continue,
            };
            if main <= 1.0 {
                continue; // not in the requested direction
            }
            let score = main + 2.0 * off;
            if best.as_ref().is_none_or(|(s, _)| score < *s) {
                best = Some((score, it.clone()));
            }
        }
        if let Some((_, it)) = best {
            self.select(Some(&it));
        }
    }

    fn copy_selection(&self, cut: bool) {
        let paths = self.selected_paths();
        if paths.is_empty() {
            return;
        }
        let uris: Vec<String> = paths
            .iter()
            .map(|p| format!("file://{}", percent_encode_path(p)))
            .collect();
        let op = if cut { "cut" } else { "copy" };
        let gnome = format!("{}\n{}", op, uris.join("\n"));
        let uri_list = uris.join("\r\n") + "\r\n";

        let provider = gdk::ContentProvider::new_union(&[
            gdk::ContentProvider::for_bytes(
                "x-special/gnome-copied-files",
                &glib::Bytes::from(gnome.as_bytes()),
            ),
            gdk::ContentProvider::for_bytes(
                "text/uri-list",
                &glib::Bytes::from(uri_list.as_bytes()),
            ),
            gdk::ContentProvider::for_bytes(
                "text/plain;charset=utf-8",
                &glib::Bytes::from(gnome.as_bytes()),
            ),
        ]);
        if let Err(e) = self.clipboard().set_content(Some(&provider)) {
            error!("Clipboard set failed: {}", e);
        } else {
            info!("Clipboard {}: {} item(s)", op, paths.len());
        }
    }

    fn paste_clipboard(&self) {
        let dest = self.imp().desktop_path.borrow().clone();
        self.clipboard().read_text_async(
            gio::Cancellable::NONE,
            glib::clone!(
                #[weak(rename_to = view)]
                self,
                move |res| {
                    let Ok(Some(text)) = res else { return };
                    let text = text.to_string();
                    let mut lines = text
                        .lines()
                        .map(|l| l.trim())
                        .filter(|l| !l.is_empty())
                        .peekable();
                    let cut = match lines.peek().copied() {
                        Some("cut") => {
                            lines.next();
                            true
                        }
                        Some("copy") => {
                            lines.next();
                            false
                        }
                        _ => false,
                    };
                    let sources: Vec<String> = lines
                        .map(|l| {
                            l.strip_prefix("file://")
                                .map(|r| {
                                    let p = match r.find('/') {
                                        Some(0) => r,
                                        Some(i) => &r[i..],
                                        None => r,
                                    };
                                    percent_decode(p)
                                })
                                .unwrap_or_else(|| l.to_string())
                        })
                        .collect();
                    view.paste_into(&sources, &dest, cut);
                }
            ),
        );
    }

    fn paste_into(&self, sources: &[String], dest_dir: &str, cut: bool) {
        let dest_real = std::fs::canonicalize(dest_dir).ok();
        let mut changed = false;
        for src in sources {
            if src.is_empty() || !Path::new(src).exists() {
                continue;
            }
            let base = Path::new(src)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let dst = unique_path(&Path::new(dest_dir).join(&base).to_string_lossy());

            if cut {
                let same_dir = std::fs::canonicalize(src)
                    .ok()
                    .as_deref()
                    .and_then(|p| p.parent())
                    .and_then(|p| std::fs::canonicalize(p).ok())
                    == dest_real;
                if same_dir {
                    continue; // cut + paste into same dir = no-op
                }
                match move_path(src, &dst) {
                    Ok(_) => {
                        info!("Pasted (move) {} → {}", src, dst);
                        changed = true;
                    }
                    Err(e) => error!("Paste move {} failed: {}", src, e),
                }
            } else {
                let sp = Path::new(src);
                let res = if sp.is_dir() {
                    copy_dir(sp, Path::new(&dst))
                } else {
                    std::fs::copy(src, &dst).map(|_| ())
                };
                match res {
                    Ok(_) => {
                        info!("Pasted (copy) {} → {}", src, dst);
                        changed = true;
                    }
                    Err(e) => error!("Paste copy {} failed: {}", src, e),
                }
            }
        }
        if cut && changed {
            self.clipboard()
                .set_content(None::<&gdk::ContentProvider>)
                .ok();
        }
        if changed {
            self.refresh();
        }
    }

    // ---- rubber-band ----

    fn on_rubber_begin(&self, x: f64, y: f64) {
        let picked = self.pick(x, y, gtk::PickFlags::DEFAULT);
        if let Some(p) = &picked
            && p.upcast_ref::<gtk::Widget>() != self.upcast_ref::<gtk::Widget>()
        {
            // a child was hit — let item handle; deny rubber
            return;
        }
        let imp = self.imp();
        imp.rubber_active.set(true);
        imp.rubber_start.set((x, y));
        imp.rubber_cur.set((x, y));
        self.select(None);
        self.queue_draw();
    }

    fn on_rubber_update(&self, dx: f64, dy: f64) {
        let imp = self.imp();
        if !imp.rubber_active.get() {
            return;
        }
        let (sx, sy) = imp.rubber_start.get();
        imp.rubber_cur.set((sx + dx, sy + dy));
        self.update_rubber_selection();
        self.queue_draw();
    }

    fn on_rubber_end(&self) {
        let imp = self.imp();
        if !imp.rubber_active.get() {
            return;
        }
        imp.rubber_active.set(false);
        self.queue_draw();
    }

    fn update_rubber_selection(&self) {
        let imp = self.imp();
        let (x0, y0) = imp.rubber_start.get();
        let (x1, y1) = imp.rubber_cur.get();
        let rx = x0.min(x1);
        let ry = y0.min(y1);
        let rw = (x1 - x0).abs();
        let rh = (y1 - y0).abs();
        let icons = imp.icons.borrow().clone();
        for item in &icons {
            let Some(b) = item.compute_bounds(self) else {
                continue;
            };
            let (ax, ay, aw, ah) = (
                b.x() as f64,
                b.y() as f64,
                b.width() as f64,
                b.height() as f64,
            );
            let intersects = !(ax + aw < rx || ax > rx + rw || ay + ah < ry || ay > ry + rh);
            let mut sel = imp.selection.borrow_mut();
            if intersects {
                if !sel.contains(item) {
                    sel.insert(item.clone());
                    item.set_selected(true);
                }
            } else if sel.contains(item) {
                sel.remove(item);
                item.set_selected(false);
            }
        }
        let need = {
            let sel = imp.selection.borrow();
            let cur = imp.selected.borrow();
            !sel.is_empty() && cur.as_ref().is_none_or(|c| !sel.contains(c))
        };
        if need {
            let first = imp.selection.borrow().iter().next().cloned();
            *imp.selected.borrow_mut() = first;
        }
    }

    // ---- empty-area click ----

    fn on_empty_click(&self, gesture: &gtk::GestureClick, _n: i32, x: f64, y: f64) {
        self.ensure_keyboard();
        let picked = self.pick(x, y, gtk::PickFlags::DEFAULT);
        if let Some(p) = &picked
            && p.upcast_ref::<gtk::Widget>() != self.upcast_ref::<gtk::Widget>()
        {
            return;
        }
        let button = gesture.current_button();
        if button == gdk::BUTTON_PRIMARY {
            self.select(None);
        } else if button == gdk::BUTTON_SECONDARY {
            self.select(None);
            self.show_desktop_menu(x, y);
        }
    }

    // ---- context menus ----

    fn build_sort_section(&self) -> gio::Menu {
        let sort = gio::Menu::new();
        sort.append(Some("Name"), Some("desktop.sort-by::name"));
        sort.append(Some("Date"), Some("desktop.sort-by::date"));
        sort.append(Some("Type"), Some("desktop.sort-by::type"));

        let order = gio::Menu::new();
        order.append(Some("Ascending"), Some("desktop.sort-order::asc"));
        order.append(Some("Descending"), Some("desktop.sort-order::desc"));

        let arrange = gio::Menu::new();
        arrange.append(Some("Auto-arrange"), Some("desktop.arrange-mode::auto"));
        arrange.append(Some("Free placement"), Some("desktop.arrange-mode::free"));

        let root = gio::Menu::new();
        root.append_submenu(Some("Sort by"), &sort);
        root.append_submenu(Some("Sort order"), &order);
        root.append_submenu(Some("Arrange mode"), &arrange);
        root.append(Some("Auto-arrange now"), Some("desktop.auto-arrange-now"));
        root
    }

    fn build_action_group(&self, item: Option<&IconItem>) -> gio::SimpleActionGroup {
        let group = gio::SimpleActionGroup::new();
        let s_type = glib::VariantTy::STRING;

        let make_stateful = |name: &str, cur: String| {
            gio::SimpleAction::new_stateful(name, Some(s_type), &cur.to_variant())
        };

        let settings = self.settings();
        let (sort_by, sort_order, arrange_mode) = {
            let s = settings.borrow();
            (
                s.sort_by.clone(),
                s.sort_order.clone(),
                s.arrange_mode.clone(),
            )
        };

        let a_sort_by = make_stateful("sort-by", sort_by);
        a_sort_by.connect_activate(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_, p| {
                if let Some(v) = p.and_then(|p| p.str()) {
                    view.set_sort("sort_by", v);
                }
            }
        ));
        group.add_action(&a_sort_by);

        let a_sort_order = make_stateful("sort-order", sort_order);
        a_sort_order.connect_activate(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_, p| {
                if let Some(v) = p.and_then(|p| p.str()) {
                    view.set_sort("sort_order", v);
                }
            }
        ));
        group.add_action(&a_sort_order);

        let a_arrange = make_stateful("arrange-mode", arrange_mode);
        a_arrange.connect_activate(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_, p| {
                if let Some(v) = p.and_then(|p| p.str()) {
                    view.set_arrange_mode(v);
                }
            }
        ));
        group.add_action(&a_arrange);

        let a_now = gio::SimpleAction::new("auto-arrange-now", None);
        a_now.connect_activate(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_, _| view.auto_arrange_now()
        ));
        group.add_action(&a_now);

        if let Some(item) = item {
            let open = gio::SimpleAction::new("open", None);
            open.connect_activate(glib::clone!(
                #[weak(rename_to = view)]
                self,
                #[weak]
                item,
                move |_, _| view.launch_item(&item)
            ));
            group.add_action(&open);

            let rename = gio::SimpleAction::new("rename", None);
            rename.connect_activate(glib::clone!(
                #[weak(rename_to = view)]
                self,
                #[weak]
                item,
                move |_, _| view.rename_item(&item)
            ));
            group.add_action(&rename);

            let del = gio::SimpleAction::new("delete", None);
            del.connect_activate(glib::clone!(
                #[weak(rename_to = view)]
                self,
                #[weak]
                item,
                move |_, _| view.delete_item(&item)
            ));
            group.add_action(&del);

            let props = gio::SimpleAction::new("properties", None);
            props.connect_activate(glib::clone!(
                #[weak(rename_to = view)]
                self,
                #[weak]
                item,
                move |_, _| view.show_properties(&item)
            ));
            group.add_action(&props);

            if item.file_path().to_ascii_lowercase().ends_with(".iso") {
                let mount = gio::SimpleAction::new("iso-mount", None);
                mount.connect_activate(glib::clone!(
                    #[weak(rename_to = view)]
                    self,
                    #[weak]
                    item,
                    move |_, _| view.iso_mount_action(&item.file_path())
                ));
                group.add_action(&mount);

                let unmount = gio::SimpleAction::new("iso-unmount", None);
                unmount.connect_activate(glib::clone!(
                    #[weak(rename_to = view)]
                    self,
                    #[weak]
                    item,
                    move |_, _| view.iso_unmount_action(&item.file_path())
                ));
                group.add_action(&unmount);
            }
        }

        group
    }

    fn show_desktop_menu(&self, x: f64, y: f64) {
        let menu = self.build_sort_section();
        self.popup_menu(menu.upcast(), None, x, y);
    }

    fn show_item_menu(&self, item: &IconItem, x: f64, y: f64) {
        let root = gio::Menu::new();
        let sect = gio::Menu::new();
        sect.append(Some("Open"), Some("desktop.open"));
        if item.is_special() {
            // Home/Trash: open only — no rename/delete/properties.
            root.append_section(None, &sect);
            root.append_section(None, &self.build_sort_section());
            let (vx, vy) = self.translate_item_to_view(item, x, y);
            self.popup_menu(root.upcast(), Some(item), vx, vy);
            return;
        }
        if item.file_path().to_ascii_lowercase().ends_with(".iso") {
            if iso_mountpoint(&item.file_path()).is_some() {
                sect.append(Some("Unmount"), Some("desktop.iso-unmount"));
            } else {
                sect.append(Some("Mount"), Some("desktop.iso-mount"));
            }
        }
        sect.append(Some("Rename"), Some("desktop.rename"));
        sect.append(Some("Delete"), Some("desktop.delete"));
        sect.append(Some("Properties"), Some("desktop.properties"));
        root.append_section(None, &sect);
        root.append_section(None, &self.build_sort_section());

        let (vx, vy) = self.translate_item_to_view(item, x, y);
        self.popup_menu(root.upcast(), Some(item), vx, vy);
    }

    fn translate_item_to_view(&self, item: &IconItem, x: f64, y: f64) -> (f64, f64) {
        let point = graphene::Point::new(x as f32, y as f32);
        if let Some(out) = item.compute_point(self, &point) {
            return (out.x() as f64, out.y() as f64);
        }
        (x, y)
    }

    fn popup_menu(&self, model: gio::MenuModel, item: Option<&IconItem>, x: f64, y: f64) {
        let popover = gtk::PopoverMenu::from_model(Some(&model));
        popover.set_has_arrow(false);
        popover.set_parent(self);

        let group = self.build_action_group(item);
        popover.insert_action_group("desktop", Some(&group));

        let rect = gdk::Rectangle::new(x as i32, y as i32, 1, 1);
        popover.set_pointing_to(Some(&rect));
        popover.popup();
    }

    // ---- settings mutations ----

    fn set_sort(&self, key: &str, value: &str) {
        {
            let s = self.settings();
            let mut s = s.borrow_mut();
            match key {
                "sort_by" => s.sort_by = value.to_string(),
                "sort_order" => s.sort_order = value.to_string(),
                _ => {}
            }
        }
        let imp = self.imp();
        imp.positions.borrow_mut().clear();
        imp.positions.borrow().save();
        self.emit_settings_changed();
        self.refresh();
    }

    fn set_arrange_mode(&self, mode: &str) {
        if mode != "auto" && mode != "free" {
            return;
        }
        let prev = self.settings().borrow().arrange_mode.clone();
        self.settings().borrow_mut().arrange_mode = mode.to_string();
        if mode == "auto" && prev != "auto" {
            let imp = self.imp();
            imp.positions.borrow_mut().clear();
            imp.positions.borrow().save();
        }
        self.emit_settings_changed();
        self.layout_all();
    }

    fn auto_arrange_now(&self) {
        let imp = self.imp();
        imp.positions.borrow_mut().clear();
        imp.positions.borrow().save();
        let mut icons = imp.icons.borrow().clone();
        icons.sort_by_cached_key(|it| self.sort_key(&it.filename()));
        *imp.icons.borrow_mut() = icons;
        self.layout_all();
    }

    fn sort_key(&self, filename: &str) -> String {
        let sort_by = self.settings().borrow().sort_by.clone();
        let dir = self.imp().desktop_path.borrow().clone();
        match sort_by.as_str() {
            "date" => std::fs::metadata(Path::new(&dir).join(filename))
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| format!("{:020}", d.as_secs()))
                .unwrap_or_else(|| "0".to_string()),
            "type" => Path::new(filename)
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default(),
            _ => filename.to_string(),
        }
    }

    // ---- file ops ----

    fn delete_item(&self, item: &IconItem) {
        if item.is_special() {
            return;
        }
        let path = item.file_path();
        let filename = item.filename();
        let window = self.parent_window();

        let do_delete = move || {
            let gfile = gio::File::for_path(&path);
            match gfile.trash(gio::Cancellable::NONE) {
                Ok(_) => info!("Trashed: {}", path),
                Err(e) => {
                    warn!("Trash failed ({}); attempting unlink", e);
                    let p = Path::new(&path);
                    let res = if p.is_dir() && !p.is_symlink() {
                        std::fs::remove_dir_all(p)
                    } else {
                        std::fs::remove_file(p)
                    };
                    if let Err(e2) = res {
                        error!("Delete failed: {}", e2);
                    }
                }
            }
        };

        let dialog = gtk::AlertDialog::builder()
            .message(format!("Move “{}” to trash?", filename))
            .buttons(["Cancel", "Move to Trash"])
            .cancel_button(0)
            .default_button(1)
            .build();
        let do_delete = Rc::new(do_delete);
        dialog.choose(
            window.as_ref(),
            gio::Cancellable::NONE,
            glib::clone!(
                #[strong]
                do_delete,
                move |res| {
                    if let Ok(idx) = res
                        && idx == 1
                    {
                        do_delete();
                    }
                }
            ),
        );
    }

    fn rename_item(&self, item: &IconItem) {
        if item.is_special() {
            return;
        }
        let old_path = item.file_path();
        let Some(dir) = Path::new(&old_path).parent().map(|d| d.to_path_buf()) else {
            error!("rename: no parent dir for {}", old_path);
            return;
        };
        let old_name = item.filename();

        let win = gtk::Window::builder()
            .title("Rename")
            .modal(true)
            .resizable(false)
            .default_width(360)
            .build();
        if let Some(parent) = self.parent_window() {
            win.set_transient_for(Some(&parent));
        }

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 8);
        vbox.set_margin_top(12);
        vbox.set_margin_bottom(12);
        vbox.set_margin_start(12);
        vbox.set_margin_end(12);

        let entry = gtk::Entry::new();
        entry.set_text(&old_name);
        entry.set_activates_default(true);
        // Pre-select the stem (name without extension), like file managers.
        let stem_len = Path::new(&old_name)
            .file_stem()
            .map(|s| s.to_string_lossy().chars().count())
            .unwrap_or_else(|| old_name.chars().count());
        entry.select_region(0, stem_len as i32);
        vbox.append(&entry);

        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        hbox.set_halign(gtk::Align::End);
        let cancel = gtk::Button::with_label("Cancel");
        let ok = gtk::Button::with_label("Rename");
        ok.add_css_class("suggested-action");
        hbox.append(&cancel);
        hbox.append(&ok);
        vbox.append(&hbox);

        win.set_child(Some(&vbox));
        win.set_default_widget(Some(&ok));

        cancel.connect_clicked(glib::clone!(
            #[weak]
            win,
            move |_| win.close()
        ));

        let do_rename = Rc::new(glib::clone!(
            #[weak(rename_to = view)]
            self,
            #[weak]
            win,
            #[weak]
            entry,
            move || {
                let new_name = entry.text().trim().to_string();
                if new_name.is_empty()
                    || new_name == old_name
                    || new_name.contains('/')
                    || new_name == "."
                    || new_name == ".."
                {
                    if new_name != old_name {
                        view.alert("Invalid name", "Name is empty or contains '/'.");
                    }
                    win.close();
                    return;
                }
                let dst = dir.join(&new_name);
                if dst.exists() {
                    view.alert("Rename failed", &format!("“{new_name}” already exists."));
                    win.close();
                    return;
                }
                match std::fs::rename(&old_path, &dst) {
                    Ok(_) => {
                        info!("Renamed {} → {}", old_path, dst.display());
                        let pos = view.imp().positions.borrow().get(&old_name);
                        {
                            let mut p = view.imp().positions.borrow_mut();
                            p.remove(&old_name);
                            if let Some((px, py)) = pos {
                                p.set(&new_name, px, py);
                            }
                        }
                        view.imp().positions.borrow().save();
                        view.refresh();
                    }
                    Err(e) => {
                        error!("Rename {} failed: {}", old_path, e);
                        view.alert("Rename failed", &e.to_string());
                    }
                }
                win.close();
            }
        ));

        ok.connect_clicked(glib::clone!(
            #[strong]
            do_rename,
            move |_| do_rename()
        ));
        entry.connect_activate(move |_| do_rename());

        win.present();
        entry.grab_focus();
    }

    fn show_properties(&self, item: &IconItem) {
        let path = item.file_path();
        let (size, mtime) = match std::fs::metadata(&path) {
            Ok(st) => {
                let size = st.len();
                let secs = st
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                let mtime = glib::DateTime::from_unix_local(secs)
                    .and_then(|d| d.format("%Y-%m-%d %H:%M:%S"))
                    .map(|g| g.to_string())
                    .unwrap_or_else(|_| "unknown".to_string());
                (size, mtime)
            }
            Err(e) => (0, format!("unknown ({})", e)),
        };

        let kind = if Path::new(&path).is_dir() {
            "Folder"
        } else {
            "File"
        };
        let detail = format!(
            "Name: {}\nPath: {}\nType: {}\nSize: {}\nModified: {}",
            item.filename(),
            path,
            kind,
            human_size(size),
            mtime
        );
        let window = self.parent_window();
        let dialog = gtk::AlertDialog::builder()
            .message("Properties")
            .detail(detail)
            .buttons(["Close"])
            .default_button(0)
            .build();
        dialog.show(window.as_ref());
    }

    // ---- drag source ----

    fn on_drag_prepare(&self, item: &IconItem) -> Option<gdk::ContentProvider> {
        // Special items (Home/Trash) are draggable for repositioning, but
        // carry no file URIs — they must never be exported as files to an
        // external drop target (or moved into a folder).
        let imp = self.imp();
        let (group, paths): (Option<DragGroup>, Vec<String>) = {
            let sel = imp.selection.borrow();
            if sel.contains(item) && sel.len() > 1 {
                let (ax, ay) = item
                    .compute_bounds(self)
                    .map(|r| (r.x(), r.y()))
                    .unwrap_or((0.0, 0.0));
                let by_name = imp.icons_by_name.borrow();
                let mut g = Vec::new();
                for it in sel.iter() {
                    let (bx, by) = it
                        .compute_bounds(self)
                        .map(|r| (r.x(), r.y()))
                        .unwrap_or((0.0, 0.0));
                    g.push((it.filename(), (bx - ax) as i32, (by - ay) as i32));
                }
                let p: Vec<String> = g
                    .iter()
                    .filter_map(|(f, _, _)| by_name.get(f))
                    .filter(|i| !i.is_special())
                    .map(|i| i.file_path())
                    .collect();
                (Some(g), p)
            } else if item.is_special() {
                (None, Vec::new())
            } else {
                (None, vec![item.file_path()])
            }
        };
        *imp.drag_group.borrow_mut() = group;
        *imp.drag_item.borrow_mut() = Some(item.clone());

        let name_provider = gdk::ContentProvider::for_value(&item.filename().to_value());

        // No real file paths (dragging only special items) → offer the name
        // token alone. A union with an empty uri-list would let the drop
        // target prefer FileList and lose the internal-reposition signal.
        if paths.is_empty() {
            return Some(name_provider);
        }

        let uri_lines = paths
            .iter()
            .map(|p| format!("file://{}", percent_encode_path(p)))
            .collect::<Vec<_>>()
            .join("\r\n")
            + "\r\n";
        let uri_provider = gdk::ContentProvider::for_bytes(
            "text/uri-list",
            &glib::Bytes::from(uri_lines.as_bytes()),
        );
        let text_provider = gdk::ContentProvider::for_bytes(
            "text/plain;charset=utf-8",
            &glib::Bytes::from(paths.join("\n").as_bytes()),
        );
        // name_provider first: with STRING as the drop target's preferred
        // type, this makes the internal reposition token win negotiation over
        // the file-path text. External consumers still pick uri-list/text.
        Some(gdk::ContentProvider::new_union(&[
            name_provider,
            uri_provider,
            text_provider,
        ]))
    }

    fn on_drag_begin(&self, source: &gtk::DragSource, item: &IconItem) {
        item.imp().click_pending.set(false);
        item.imp().collapse_on_release.set(false);
        if let Some(p) = item.paintable() {
            let half = self.settings().borrow().icon_size / 2;
            source.set_icon(Some(&p), half, half);
        }
    }

    // ---- drop ----

    /// IconItem under (x, y) in view coords that is a directory, if any.
    fn folder_at(&self, x: f64, y: f64) -> Option<IconItem> {
        let mut w = self.pick(x, y, gtk::PickFlags::DEFAULT)?;
        loop {
            if let Some(it) = w.downcast_ref::<IconItem>() {
                if it.is_special() {
                    return None;
                }
                return Path::new(&it.file_path()).is_dir().then(|| it.clone());
            }
            w = w.parent()?;
        }
    }

    /// Special item (Home/Trash) under (x, y) in view coords, if any.
    fn special_at(&self, x: f64, y: f64) -> Option<IconItem> {
        let mut w = self.pick(x, y, gtk::PickFlags::DEFAULT)?;
        loop {
            if let Some(it) = w.downcast_ref::<IconItem>() {
                return it.is_special().then(|| it.clone());
            }
            w = w.parent()?;
        }
    }

    /// Send paths to the trash (freedesktop). Returns true if any succeeded.
    fn trash_paths(&self, paths: &[String]) -> bool {
        let imp = self.imp();
        let mut any = false;
        for src in paths {
            let gfile = gio::File::for_path(src);
            match gfile.trash(gio::Cancellable::NONE) {
                Ok(_) => {
                    info!("Trashed: {}", src);
                    if let Some(base) = Path::new(src).file_name() {
                        imp.positions
                            .borrow_mut()
                            .remove(&base.to_string_lossy());
                    }
                    any = true;
                }
                Err(e) => error!("Trash {} failed: {}", src, e),
            }
        }
        if any {
            imp.positions.borrow().save();
            self.refresh();
        }
        any
    }

    fn set_drop_highlight(&self, item: Option<&IconItem>) {
        let mut hl = self.imp().drop_hl.borrow_mut();
        if hl.as_ref() == item {
            return;
        }
        if let Some(prev) = hl.take() {
            prev.remove_css_class("drop-target");
        }
        if let Some(it) = item {
            it.add_css_class("drop-target");
            *hl = Some(it.clone());
        }
    }

    /// Move each source path into `folder`. Skips a source that is the
    /// folder itself or an ancestor of it. Returns true if anything moved.
    fn move_into_folder(&self, sources: &[String], folder: &str) -> bool {
        let folder_real = match std::fs::canonicalize(folder) {
            Ok(p) => p,
            Err(e) => {
                error!("Drop target {} unresolved: {}", folder, e);
                return false;
            }
        };
        let mut moved = false;
        for src in sources {
            if src.is_empty() || !Path::new(src).exists() {
                continue;
            }
            if let Ok(src_real) = std::fs::canonicalize(src)
                && (folder_real == src_real || folder_real.starts_with(&src_real))
            {
                warn!("Refusing to move {} into itself", src);
                continue;
            }
            let base = Path::new(src)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let dst = unique_path(&Path::new(folder).join(&base).to_string_lossy());
            match move_path(src, &dst) {
                Ok(_) => {
                    info!("Moved {} → {}", src, dst);
                    self.imp().positions.borrow_mut().remove(&base);
                    moved = true;
                }
                Err(e) => error!("Move {} into {} failed: {}", src, folder, e),
            }
        }
        if moved {
            self.imp().positions.borrow().save();
            self.refresh();
        }
        moved
    }

    fn on_drop(&self, value: &glib::Value, x: f64, y: f64) -> bool {
        let imp = self.imp();
        self.set_drop_highlight(None);

        // Internal drag (an icon from this view being repositioned/moved)?
        // Recognised via drag_item, set on drag prepare — independent of the
        // DnD content type, which GTK mangles for the URI-less special items.
        let internal = imp.drag_item.borrow().clone();

        // Real filesystem paths being dragged. Special items (Home/Trash) are
        // synthetic and excluded — they can never be moved into a folder or
        // trashed, only repositioned.
        let sources: Vec<String> = match &internal {
            Some(item) => {
                let by = imp.icons_by_name.borrow();
                match imp.drag_group.borrow().clone() {
                    Some(g) => g
                        .iter()
                        .filter_map(|(f, _, _)| by.get(f))
                        .filter(|i| !i.is_special())
                        .map(|i| i.file_path())
                        .collect(),
                    None if item.is_special() => Vec::new(),
                    None => vec![item.file_path()],
                }
            }
            None => self.value_to_paths(value),
        };

        // Released over Home → move into the home dir; over Trash → trash.
        // (No real sources = dragging a special icon itself → reposition.)
        if let Some(special) = self.special_at(x, y) {
            if !sources.is_empty() {
                let handled = if special.open_uri().as_deref() == Some("trash:///") {
                    self.trash_paths(&sources)
                } else {
                    self.move_into_folder(&sources, &special.file_path())
                };
                if handled {
                    return true;
                }
            }
        } else if let Some(folder) = self.folder_at(x, y) {
            // Released over a folder icon → move source(s) into that folder.
            if !sources.is_empty() && self.move_into_folder(&sources, &folder.file_path()) {
                return true;
            }
        }

        // Internal drag not consumed by a folder/special target → reposition.
        if let Some(item) = internal {
            return self.reposition_drop(&item, x, y);
        }

        // External drop on empty desktop → import into the desktop folder.
        self.import_paths(&sources, x, y)
    }

    /// Extract filesystem paths from an external drop value.
    fn value_to_paths(&self, value: &glib::Value) -> Vec<String> {
        if let Ok(s) = value.get::<String>() {
            parse_uri_list(&s)
        } else if let Ok(list) = value.get::<gdk::FileList>() {
            list.files()
                .iter()
                .filter_map(|f| f.path())
                .map(|p| p.to_string_lossy().into_owned())
                .collect()
        } else if let Ok(file) = value.get::<gio::File>() {
            file.path()
                .map(|p| vec![p.to_string_lossy().into_owned()])
                .unwrap_or_default()
        } else {
            warn!("Drop: unsupported value type {}", value.type_());
            Vec::new()
        }
    }

    fn reposition_drop(&self, item: &IconItem, x: f64, y: f64) -> bool {
        let imp = self.imp();
        if self.settings().borrow().arrange_mode != "free" {
            self.settings().borrow_mut().arrange_mode = "free".to_string();
            self.emit_settings_changed();
            let icons = imp.icons.borrow().clone();
            for (i, it) in icons.iter().enumerate() {
                if imp.positions.borrow().get(&it.filename()).is_none() {
                    let p = self.grid_position(i as i32);
                    imp.positions.borrow_mut().set(&it.filename(), p.0, p.1);
                }
            }
        }
        let (cw, ch) = cell_size(self.settings().borrow().icon_size);
        let nx = (x as i32 - cw / 2).max(0);
        let ny = (y as i32 - ch / 2).max(0);

        let group = imp.drag_group.borrow().clone();
        if let Some(group) = group {
            for (fname, ox, oy) in group {
                let it = imp.icons_by_name.borrow().get(&fname).cloned();
                if let Some(it) = it {
                    let gx = (nx + ox).max(0);
                    let gy = (ny + oy).max(0);
                    imp.positions.borrow_mut().set(&fname, gx, gy);
                    self.move_(&it, gx as f64, gy as f64);
                }
            }
        } else {
            imp.positions.borrow_mut().set(&item.filename(), nx, ny);
            self.move_(item, nx as f64, ny as f64);
        }
        imp.positions.borrow().save();
        *imp.drag_group.borrow_mut() = None;
        true
    }

    fn import_paths(&self, paths: &[String], x: f64, y: f64) -> bool {
        let paths: Vec<&String> = paths.iter().filter(|p| !p.is_empty()).collect();
        if paths.is_empty() {
            return false;
        }
        let imp = self.imp();
        let dest_dir = imp.desktop_path.borrow().clone();
        let dest_real = std::fs::canonicalize(&dest_dir).ok();
        let (cw, ch) = cell_size(self.settings().borrow().icon_size);
        let free_mode = self.settings().borrow().arrange_mode == "free";

        let mut handled = false;
        for (i, src) in paths.iter().enumerate() {
            let src_real = std::fs::canonicalize(src).ok();
            if src_real
                .as_deref()
                .and_then(|p| p.parent())
                .and_then(|p| std::fs::canonicalize(p).ok())
                == dest_real
            {
                continue;
            }
            if !Path::new(src).exists() {
                warn!("Drop source missing: {}", src);
                continue;
            }
            let base = Path::new(src)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let dst = unique_path(&Path::new(&dest_dir).join(&base).to_string_lossy());
            match move_path(src, &dst) {
                Ok(_) => {
                    info!("Moved {} → {}", src, dst);
                    handled = true;
                }
                Err(e) => {
                    error!("Drop move failed for {}: {}", src, e);
                    continue;
                }
            }
            if free_mode {
                let dn = Path::new(&dst)
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                imp.positions.borrow_mut().set(
                    &dn,
                    (x as i32 - cw / 2 + i as i32 * 16).max(0),
                    (y as i32 - ch / 2 + i as i32 * 16).max(0),
                );
            }
        }
        if free_mode && handled {
            imp.positions.borrow().save();
        }
        handled
    }
}

// ---------------------------------------------------------------------------
// free helpers
// ---------------------------------------------------------------------------

fn human_size(n: u64) -> String {
    let mut f = n as f64;
    for unit in ["B", "KB", "MB", "GB", "TB"] {
        if f < 1024.0 || unit == "TB" {
            return if unit == "B" {
                format!("{} {}", f as i64, unit)
            } else {
                format!("{:.1} {}", f, unit)
            };
        }
        f /= 1024.0;
    }
    format!("{} B", n)
}

fn parse_uri_list(text: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("file://") {
            // strip optional host: file://host/path -> /path
            let path_part = match rest.find('/') {
                Some(0) => rest,
                Some(i) => &rest[i..],
                None => rest,
            };
            paths.push(percent_decode(path_part));
        }
    }
    paths
}

fn unique_path(path: &str) -> String {
    if !Path::new(path).exists() {
        return path.to_string();
    }
    let p = Path::new(path);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = p
        .extension()
        .map(|s| format!(".{}", s.to_string_lossy()))
        .unwrap_or_default();
    let dir = p.parent().map(|d| d.to_path_buf()).unwrap_or_default();
    let mut n = 1;
    loop {
        let cand = dir.join(format!("{} ({}){}", stem, n, ext));
        if !cand.exists() {
            return cand.to_string_lossy().into_owned();
        }
        n += 1;
    }
}

/// shutil.move equivalent — rename, fall back to copy+remove across devices.
fn move_path(src: &str, dst: &str) -> std::io::Result<()> {
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    let sp = Path::new(src);
    if sp.is_dir() {
        copy_dir(sp, Path::new(dst))?;
        std::fs::remove_dir_all(sp)
    } else {
        std::fs::copy(src, dst)?;
        std::fs::remove_file(src)
    }
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), to)?;
        }
    }
    Ok(())
}

const UNRESERVED: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.~/";

fn percent_encode_path(s: &str) -> String {
    let mut out = String::new();
    for &b in s.as_bytes() {
        if UNRESERVED.contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------------------------------------------------------------------
// open / mount helpers
// ---------------------------------------------------------------------------

/// Tokenize a .desktop `Exec=` value per the freedesktop spec: split on
/// unquoted whitespace, honor double quotes with `\\ \" \` \$` escapes,
/// and drop field codes (`%f %F %u %U %i %c %k %d …`; `%%` → literal `%`).
fn parse_exec(exec: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut has_tok = false;
    let mut in_quote = false;
    let mut chars = exec.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_quote = !in_quote;
                has_tok = true;
            }
            '\\' if in_quote => {
                if let Some(&n) = chars.peek() {
                    if matches!(n, '"' | '\\' | '`' | '$') {
                        cur.push(n);
                        chars.next();
                    } else {
                        cur.push('\\');
                    }
                }
            }
            '%' if !in_quote => {
                // %% → literal %, any other %x → dropped field code
                if chars.next() == Some('%') {
                    cur.push('%');
                }
            }
            c if c.is_whitespace() && !in_quote => {
                if has_tok {
                    args.push(std::mem::take(&mut cur));
                    has_tok = false;
                }
            }
            c => {
                cur.push(c);
                has_tok = true;
            }
        }
    }
    if has_tok {
        args.push(cur);
    }
    args
}

/// Parse `[Desktop Entry]` keys from a .desktop file.
fn parse_desktop_entry(path: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let Ok(content) = std::fs::read_to_string(path) else {
        return map;
    };
    let mut in_section = false;
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line == "[Desktop Entry]";
            continue;
        }
        if !in_section || line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            map.entry(k.trim().to_string())
                .or_insert_with(|| v.trim().to_string());
        }
    }
    map
}

/// `Command` for a child process with our startup `LD_PRELOAD` stripped.
///
/// main.rs re-execs with `LD_PRELOAD=libgtk4-layer-shell.so` so layer-shell
/// loads before libwayland. Children inherit it, which forces layer-shell
/// into unrelated GTK apps and crashes them. Strip it for every child.
fn clean_command(program: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new(program);
    cmd.env_remove("LD_PRELOAD");
    cmd
}

/// gdk launch context with `LD_PRELOAD` unset, for gio AppInfo launches
/// (same inheritance problem as spawned children).
fn clean_launch_context() -> Option<gdk::AppLaunchContext> {
    let ctx = gdk::Display::default()?.app_launch_context();
    ctx.unsetenv("LD_PRELOAD");
    Some(ctx)
}

/// Tell Hyprland to focus the launched window by PID. Needed because the
/// pointer stays over our desktop layer surface, so with
/// `focus_follows_mouse` the new toplevel isn't focused until the mouse
/// moves. Retried — the window isn't mapped immediately after spawn.
fn request_hypr_focus(pid: u32) {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
        return;
    }
    for delay_ms in [150u64, 450, 1000] {
        glib::timeout_add_local_once(std::time::Duration::from_millis(delay_ms), move || {
            let _ = clean_command("hyprctl")
                .args(["dispatch", "focuswindow", &format!("pid:{pid}")])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        });
    }
}

/// Run a .desktop entry as it intends: Type=Application → its `Exec=`
/// (honouring `Terminal=`/`Path=`); Type=Link → open its `URL=`.
fn launch_desktop_entry(path: &str) -> bool {
    let entry = parse_desktop_entry(path);
    if entry.is_empty() {
        error!("Invalid .desktop entry: {}", path);
        return false;
    }

    if entry.get("Type").map(String::as_str) == Some("Link") {
        return match entry.get("URL").filter(|u| !u.is_empty()) {
            Some(url) => open_with_default(url),
            None => {
                error!(".desktop {} is a Link with no URL", path);
                false
            }
        };
    }

    let Some(exec) = entry.get("Exec").filter(|e| !e.is_empty()) else {
        error!(".desktop {} has no Exec", path);
        return false;
    };
    let tokens = parse_exec(exec);
    let Some((program, prog_args)) = tokens.split_first() else {
        error!(".desktop {} has empty Exec", path);
        return false;
    };
    let cmdline = tokens.join(" ");

    let mut cmd;
    if entry.get("Terminal").map(String::as_str) == Some("true") {
        let term = std::env::var("TERMINAL").unwrap_or_else(|_| "xterm".to_string());
        cmd = clean_command(&term);
        cmd.arg("-e").arg(program).args(prog_args);
    } else {
        cmd = clean_command(program);
        cmd.args(prog_args);
    }
    if let Some(dir) = entry.get("Path").filter(|p| !p.is_empty()) {
        cmd.current_dir(dir);
    }
    match cmd.spawn() {
        Ok(child) => {
            info!("Launched .desktop: {} ({})", path, cmdline);
            request_hypr_focus(child.id());
            true
        }
        Err(e) => {
            error!("Launch .desktop {} failed: {}", path, e);
            false
        }
    }
}

/// `$XDG_DATA_HOME/Trash/files` (or `~/.local/share/Trash/files`) — the
/// freedesktop trash directory for the active user.
fn trash_files_dir() -> String {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| dirs::home_dir().map(|h| h.join(".local").join("share")))
        .unwrap_or_default();
    base.join("Trash")
        .join("files")
        .to_string_lossy()
        .into_owned()
}

/// True if the trash holds at least one entry (picks the full vs empty icon).
fn trash_has_files() -> bool {
    std::fs::read_dir(trash_files_dir())
        .map(|mut d| d.next().is_some())
        .unwrap_or(false)
}

/// Open a URI (e.g. `trash:///`) with its default handler, falling back to
/// `xdg-open`. Same `LD_PRELOAD`-clean launch context as file launches.
fn open_uri_default(uri: &str) -> bool {
    let ctx = clean_launch_context();
    if let Some(ctx) = &ctx {
        ctx.connect_launched(|_, _, platform_data| {
            let dict = glib::VariantDict::new(Some(platform_data));
            if let Ok(Some(pid)) = dict.lookup::<i32>("pid") {
                request_hypr_focus(pid as u32);
            }
        });
    }
    if gio::AppInfo::launch_default_for_uri(uri, ctx.as_ref()).is_ok() {
        return true;
    }
    match clean_command("xdg-open").arg(uri).spawn() {
        Ok(child) => {
            request_hypr_focus(child.id());
            true
        }
        Err(e) => {
            error!("xdg-open {} failed: {}", uri, e);
            false
        }
    }
}

/// Open with the MIME default app, passing a GFile (not a URI, which some
/// browsers turn into a download). Text types go to a text editor.
fn open_with_default(path: &str) -> bool {
    let p = Path::new(path);
    let ct = if p.is_dir() {
        "inode/directory".to_string()
    } else {
        gio::content_type_guess(Some(p), None).0.to_string()
    };
    let lookup = if gio::content_type_is_a(&ct, "text/plain") {
        "text/plain"
    } else {
        ct.as_str()
    };

    if let Some(app) = gio::AppInfo::default_for_type(lookup, false) {
        let file = gio::File::for_path(path);
        let ctx = clean_launch_context();
        if let Some(ctx) = &ctx {
            // "launched" carries the spawned pid in platform-data.
            ctx.connect_launched(|_, _, platform_data| {
                let dict = glib::VariantDict::new(Some(platform_data));
                if let Ok(Some(pid)) = dict.lookup::<i32>("pid") {
                    request_hypr_focus(pid as u32);
                }
            });
        }
        match app.launch(&[file], ctx.as_ref()) {
            Ok(_) => return true,
            Err(e) => warn!("default app for {} failed: {}; using xdg-open", lookup, e),
        }
    }
    match clean_command("xdg-open").arg(path).spawn() {
        Ok(child) => {
            request_hypr_focus(child.id());
            true
        }
        Err(e) => {
            error!("xdg-open {} failed: {}", path, e);
            false
        }
    }
}

fn cmd_lines(program: &str, args: &[&str]) -> Option<Vec<String>> {
    let out = clean_command(program).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
    )
}

/// Loop device currently backing `iso`, if any (`losetup -j`).
fn iso_loop_dev(iso: &str) -> Option<String> {
    cmd_lines("losetup", &["-j", iso, "-O", "NAME", "--noheadings"])?
        .into_iter()
        .next()
}

/// Mountpoint of a backing device, if mounted (`findmnt -S <dev>`).
fn dev_mountpoint(dev: &str) -> Option<String> {
    cmd_lines("findmnt", &["-n", "-o", "TARGET", "-S", dev])?
        .into_iter()
        .next()
}

/// Current mountpoint of `iso` if it is looped and mounted.
fn iso_mountpoint(iso: &str) -> Option<String> {
    dev_mountpoint(&iso_loop_dev(iso)?)
}

fn udisksctl(args: &[&str]) -> Result<(), String> {
    let out = clean_command("udisksctl")
        .args(args)
        .output()
        .map_err(|e| format!("udisksctl: {e} (is udisks2 installed?)"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Ensure `iso` is mounted; returns its mountpoint. Reuses an existing
/// loop device / mount if already set up.
fn mount_iso(iso: &str) -> Result<String, String> {
    if let Some(mp) = iso_mountpoint(iso) {
        return Ok(mp);
    }
    let dev = match iso_loop_dev(iso) {
        Some(d) => d,
        None => {
            udisksctl(&["loop-setup", "-f", iso])?;
            iso_loop_dev(iso).ok_or_else(|| "loop device not found after setup".to_string())?
        }
    };
    if let Some(mp) = dev_mountpoint(&dev) {
        return Ok(mp);
    }
    udisksctl(&["mount", "-b", &dev])?;
    dev_mountpoint(&dev).ok_or_else(|| "mountpoint not found after mount".to_string())
}

/// Unmount `iso` and detach its loop device.
fn unmount_iso(iso: &str) -> Result<(), String> {
    let dev = iso_loop_dev(iso).ok_or_else(|| "ISO is not mounted".to_string())?;
    if dev_mountpoint(&dev).is_some() {
        udisksctl(&["unmount", "-b", &dev])?;
    }
    udisksctl(&["loop-delete", "-b", &dev])
}
