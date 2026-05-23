use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use relm4::gtk::gio;
use relm4::gtk::gio::prelude::*;
use relm4::gtk::glib;
use tracing::{debug, error, info};

pub type Callback = Rc<dyn Fn(&str, &str)>;

struct Inner {
    pending: RefCell<std::collections::HashSet<String>>,
    debounce: RefCell<Option<glib::SourceId>>,
}

pub struct FileWatcher {
    path: PathBuf,
    callback: Callback,
    monitor: RefCell<Option<gio::FileMonitor>>,
    inner: Rc<Inner>,
}

impl FileWatcher {
    pub fn new(path: &str, callback: Callback) -> Rc<Self> {
        Rc::new(Self {
            path: PathBuf::from(path),
            callback,
            monitor: RefCell::new(None),
            inner: Rc::new(Inner {
                pending: RefCell::new(std::collections::HashSet::new()),
                debounce: RefCell::new(None),
            }),
        })
    }

    pub fn start(self: &Rc<Self>) -> bool {
        if !self.path.exists() {
            error!("Watch path does not exist: {}", self.path.display());
            return false;
        }
        let file = gio::File::for_path(&self.path);
        let monitor = match file
            .monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
        {
            Ok(m) => m,
            Err(e) => {
                error!("Failed to start monitor on {}: {}", self.path.display(), e);
                return false;
            }
        };

        let this = Rc::downgrade(self);
        monitor.connect_changed(move |_m, file, _other, event| {
            let Some(this) = this.upgrade() else { return };
            this.on_changed(file, event);
        });
        *self.monitor.borrow_mut() = Some(monitor);
        info!("FileWatcher started: {}", self.path.display());
        true
    }

    fn on_changed(self: &Rc<Self>, file: &gio::File, event: gio::FileMonitorEvent) {
        let filename = match file.basename() {
            Some(b) => b.to_string_lossy().into_owned(),
            None => return,
        };
        if filename.starts_with('.') {
            return;
        }

        let event_name = match event {
            gio::FileMonitorEvent::Created => "created",
            gio::FileMonitorEvent::Deleted => "deleted",
            gio::FileMonitorEvent::Changed => "changed",
            gio::FileMonitorEvent::Renamed => "renamed",
            gio::FileMonitorEvent::MovedIn => "moved_in",
            gio::FileMonitorEvent::MovedOut => "moved_out",
            _ => "changed",
        };
        debug!("FS event: {} → {}", event_name, filename);
        self.inner
            .pending
            .borrow_mut()
            .insert(event_name.to_string());

        if let Some(src) = self.inner.debounce.borrow_mut().take() {
            src.remove();
        }
        let inner = self.inner.clone();
        let callback = self.callback.clone();
        let src = glib::timeout_add_local(Duration::from_millis(100), move || {
            let events: Vec<String> = inner.pending.borrow().iter().cloned().collect();
            inner.pending.borrow_mut().clear();
            *inner.debounce.borrow_mut() = None;
            debug!("Debounce fired, events: {:?}", events);
            callback("changed", "directory");
            glib::ControlFlow::Break
        });
        *self.inner.debounce.borrow_mut() = Some(src);
    }

    pub fn stop(&self) {
        if let Some(src) = self.inner.debounce.borrow_mut().take() {
            src.remove();
        }
        if let Some(m) = self.monitor.borrow_mut().take() {
            m.cancel();
        }
        info!("FileWatcher stopped: {}", self.path.display());
    }
}
