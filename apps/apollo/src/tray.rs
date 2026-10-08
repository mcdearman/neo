//! Apollo's icon in the menu bar or system tray, with a small menu.
//!
//! Apollo runs out of sight, ready for the search bar, so this is where
//! it is to be found: to open its window, to lock its memory, to quit it.
//!
//! macOS and Windows use the `tray-icon` crate. Linux uses the
//! StatusNotifierItem protocol over D-Bus, which KDE, and GNOME with the
//! AppIndicator extension, show in their trays.

/// What the user chose from the menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayAction {
    /// Show Apollo's window.
    Open,
    /// Lock the memory now, not when it has sat unused a while.
    Lock,
    /// Look through the folders now.
    Read,
    Quit,
}

/// What the menu reflects.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrayState {
    /// What Apollo is doing, in a line: "Up to date", "Looking at 3 of 40".
    pub status: String,
    pub unlocked: bool,
    /// A reading can be started: the model is there and none is under way.
    pub can_read: bool,
}

pub use imp::Tray;

#[cfg(any(target_os = "macos", windows))]
mod imp {
    use super::*;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

    pub struct Tray {
        // Kept so that the icon stays.
        _icon: TrayIcon,
        status: MenuItem,
        lock: MenuItem,
        read: MenuItem,
    }

    /// Decodes the PNG drawn by `cargo xtask icons`.
    fn icon(png: &[u8]) -> Result<Icon, String> {
        let mut reader = png::Decoder::new(std::io::Cursor::new(png)).read_info().map_err(|e| e.to_string())?;
        let mut pixels = vec![0; reader.output_buffer_size().ok_or("the tray icon is too large")?];
        let info = reader.next_frame(&mut pixels).map_err(|e| e.to_string())?;
        if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
            return Err("the tray icon is not 8-bit RGBA".into());
        }
        pixels.truncate(info.buffer_size());
        Icon::from_rgba(pixels, info.width, info.height).map_err(|e| e.to_string())
    }

    impl Tray {
        /// Adds the icon. On macOS this must run on the main thread once the
        /// app's event loop has started.
        pub fn new(state: &TrayState, send: impl Fn(TrayAction) + Send + Sync + 'static) -> Result<Self, String> {
            let open = MenuItem::new("Open Apollo", true, None);
            // What it is doing: said, not something to choose.
            let status = MenuItem::new(&state.status, false, None);
            let read = MenuItem::new("Read Folders Now", state.can_read, None);
            let lock = MenuItem::new("Lock Memory", state.unlocked, None);
            let quit = MenuItem::new("Quit Apollo", true, None);
            let menu = Menu::new();
            menu.append_items(&[&open, &PredefinedMenuItem::separator(), &status, &read, &lock, &PredefinedMenuItem::separator(), &quit]).map_err(|e| e.to_string())?;
            let (open_id, read_id, lock_id, quit_id) = (open.id().clone(), read.id().clone(), lock.id().clone(), quit.id().clone());
            // What was chosen, by the entry's ID.
            let chosen = move |id: &tray_icon::menu::MenuId| {
                let action = if *id == open_id {
                    TrayAction::Open
                } else if *id == read_id {
                    TrayAction::Read
                } else if *id == lock_id {
                    TrayAction::Lock
                } else if *id == quit_id {
                    TrayAction::Quit
                } else {
                    return;
                };
                send(action);
            };
            // On macOS the framework has the program's one menu handler, for the
            // menu bar, and passes on what is chosen; elsewhere it is ours to set.
            if cfg!(target_os = "macos") {
                neo::on_menu_chosen(move |id| chosen(&tray_icon::menu::MenuId::new(id)));
            } else {
                MenuEvent::set_event_handler(Some(move |e: MenuEvent| chosen(e.id())));
            }
            let glyph = icon(include_bytes!("../../../dist/icons/tray/apollo.png"))?;
            let icon = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("Apollo");
            // A template icon is tinted by the system to suit a light or dark menu bar.
            #[cfg(target_os = "macos")]
            let icon = icon.with_icon_templated(glyph);
            #[cfg(not(target_os = "macos"))]
            let icon = icon.with_icon(glyph);
            Ok(Self { _icon: icon.build().map_err(|e| e.to_string())?, status, lock, read })
        }

        pub fn update(&self, state: &TrayState) {
            self.status.set_text(&state.status);
            self.lock.set_enabled(state.unlocked);
            self.read.set_enabled(state.can_read);
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use super::*;
    use ksni::blocking::TrayMethods;
    use ksni::menu::StandardItem;

    struct Item {
        state: TrayState,
        send: Box<dyn Fn(TrayAction) + Send + Sync>,
    }

    impl ksni::Tray for Item {
        fn id(&self) -> String {
            "org.neo.Apollo".into()
        }

        fn title(&self) -> String {
            "Apollo".into()
        }

        fn icon_name(&self) -> String {
            // The app's own icon, installed by `cargo xtask install`.
            "org.neo.Apollo".into()
        }

        fn activate(&mut self, _x: i32, _y: i32) {
            (self.send)(TrayAction::Open);
        }

        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            let item = |label: &str, enabled: bool, action: TrayAction| -> ksni::MenuItem<Self> { StandardItem { label: label.into(), enabled, activate: Box::new(move |t: &mut Self| (t.send)(action)), ..Default::default() }.into() };
            vec![item("Open Apollo", true, TrayAction::Open), ksni::MenuItem::Separator, StandardItem { label: self.state.status.clone(), enabled: false, ..Default::default() }.into(), item("Read Folders Now", self.state.can_read, TrayAction::Read), item("Lock Memory", self.state.unlocked, TrayAction::Lock), ksni::MenuItem::Separator, item("Quit Apollo", true, TrayAction::Quit)]
        }
    }

    pub struct Tray(ksni::blocking::Handle<Item>);

    impl Tray {
        pub fn new(state: &TrayState, send: impl Fn(TrayAction) + Send + Sync + 'static) -> Result<Self, String> {
            Item { state: state.clone(), send: Box::new(send) }.spawn().map(Tray).map_err(|e| e.to_string())
        }

        pub fn update(&self, state: &TrayState) {
            let state = state.clone();
            self.0.update(move |t| t.state = state);
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use super::*;

    pub struct Tray;

    impl Tray {
        pub fn new(_state: &TrayState, _send: impl Fn(TrayAction) + Send + Sync + 'static) -> Result<Self, String> {
            Err("this platform has no tray".into())
        }

        pub fn update(&self, _state: &TrayState) {}
    }
}
