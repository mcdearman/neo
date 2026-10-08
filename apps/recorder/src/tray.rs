//! The Recorder's icon in the menu bar or system tray, with a small menu.
//!
//! macOS and Windows use the `tray-icon` crate. Linux uses the
//! StatusNotifierItem protocol over D-Bus, which KDE, and GNOME with the
//! AppIndicator extension, show in their trays.

/// What the user chose from the tray menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayAction {
    Show,
    /// Bring up the area frame to take a screenshot.
    Screenshot,
    /// Start recording, or stop the recording in progress.
    ToggleRecording,
    ToggleAutostart,
    Quit,
}

/// What the tray reflects.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrayState {
    pub recording: bool,
    pub autostart: bool,
}

fn toggle_label(recording: bool) -> &'static str {
    if recording { "Stop Recording" } else { "Start Recording" }
}

pub use imp::Tray;

#[cfg(any(target_os = "macos", windows))]
mod imp {
    use super::*;
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

    pub struct Tray {
        icon: TrayIcon,
        toggle: MenuItem,
        autostart: CheckMenuItem,
    }

    /// Decodes one of the PNGs drawn by `cargo xtask icons`.
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

    fn icon_for(recording: bool) -> Result<Icon, String> {
        if recording { icon(include_bytes!("../../../dist/icons/tray/recorder-recording.png")) } else { icon(include_bytes!("../../../dist/icons/tray/recorder.png")) }
    }

    impl Tray {
        /// Adds the icon. On macOS this must run on the main thread once the
        /// app's event loop has started.
        pub fn new(state: TrayState, send: impl Fn(TrayAction) + Send + Sync + 'static) -> Result<Self, String> {
            let show = MenuItem::new("Show Recorder", true, None);
            let shot = MenuItem::new("Take Screenshot…", true, None);
            let toggle = MenuItem::new(toggle_label(state.recording), true, None);
            let autostart = CheckMenuItem::new("Launch at Startup", true, state.autostart, None);
            let quit = MenuItem::new("Quit NeoCap", true, None);
            let menu = Menu::new();
            menu.append_items(&[&show, &shot, &toggle, &PredefinedMenuItem::separator(), &autostart, &PredefinedMenuItem::separator(), &quit]).map_err(|e| e.to_string())?;
            let (show_id, shot_id, toggle_id, autostart_id, quit_id) = (show.id().clone(), shot.id().clone(), toggle.id().clone(), autostart.id().clone(), quit.id().clone());
            // What was chosen, by the entry's ID.
            let chosen = move |id: &tray_icon::menu::MenuId| {
                let action = if *id == show_id {
                    TrayAction::Show
                } else if *id == shot_id {
                    TrayAction::Screenshot
                } else if *id == toggle_id {
                    TrayAction::ToggleRecording
                } else if *id == autostart_id {
                    TrayAction::ToggleAutostart
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
            // A template icon is tinted by the system to suit a light or dark menu bar.
            let icon = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("NeoCap");
            #[cfg(target_os = "macos")]
            let icon = if state.recording { icon.with_icon(icon_for(true)?) } else { icon.with_icon_templated(icon_for(false)?) };
            #[cfg(not(target_os = "macos"))]
            let icon = icon.with_icon(icon_for(state.recording)?);
            let icon = icon.build().map_err(|e| e.to_string())?;
            Ok(Self { icon, toggle, autostart })
        }

        pub fn update(&self, state: TrayState) {
            self.toggle.set_text(toggle_label(state.recording));
            // Clicking a check item flips it, so set it to the real state.
            self.autostart.set_checked(state.autostart);
            if let Ok(icon) = icon_for(state.recording) {
                // The red recording dot keeps its colour; the idle glyph is tinted.
                #[cfg(target_os = "macos")]
                let _ = if state.recording { self.icon.set_icon(Some(icon)) } else { self.icon.set_icon_templated(Some(icon)) };
                #[cfg(not(target_os = "macos"))]
                let _ = self.icon.set_icon(Some(icon));
            }
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use super::*;
    use ksni::blocking::TrayMethods;
    use ksni::menu::{CheckmarkItem, StandardItem};

    struct Item {
        state: TrayState,
        send: Box<dyn Fn(TrayAction) + Send + Sync>,
    }

    impl ksni::Tray for Item {
        fn id(&self) -> String {
            "org.neo.Recorder".into()
        }

        fn title(&self) -> String {
            "NeoCap".into()
        }

        fn icon_name(&self) -> String {
            // The app's own icon, installed by `cargo xtask install`, or the
            // theme's record symbol while recording.
            if self.state.recording { "media-record".into() } else { "org.neo.Recorder".into() }
        }

        fn activate(&mut self, _x: i32, _y: i32) {
            (self.send)(TrayAction::Show);
        }

        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            let item = |label: &str, action: TrayAction| -> ksni::MenuItem<Self> { StandardItem { label: label.into(), activate: Box::new(move |t: &mut Self| (t.send)(action)), ..Default::default() }.into() };
            vec![
                item("Show Recorder", TrayAction::Show),
                item("Take Screenshot…", TrayAction::Screenshot),
                item(toggle_label(self.state.recording), TrayAction::ToggleRecording),
                ksni::MenuItem::Separator,
                CheckmarkItem { label: "Launch at Startup".into(), checked: self.state.autostart, activate: Box::new(|t: &mut Self| (t.send)(TrayAction::ToggleAutostart)), ..Default::default() }.into(),
                ksni::MenuItem::Separator,
                item("Quit NeoCap", TrayAction::Quit),
            ]
        }
    }

    pub struct Tray(ksni::blocking::Handle<Item>);

    impl Tray {
        pub fn new(state: TrayState, send: impl Fn(TrayAction) + Send + Sync + 'static) -> Result<Self, String> {
            Item { state, send: Box::new(send) }.spawn().map(Tray).map_err(|e| e.to_string())
        }

        pub fn update(&self, state: TrayState) {
            self.0.update(|t| t.state = state);
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use super::*;

    pub struct Tray;

    impl Tray {
        pub fn new(_state: TrayState, _send: impl Fn(TrayAction) + Send + Sync + 'static) -> Result<Self, String> {
            Err("this platform has no tray".into())
        }

        pub fn update(&self, _state: TrayState) {}
    }
}
