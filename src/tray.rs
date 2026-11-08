use crate::config::{Config, Edge};
use crate::state::{Shared, Status};
use ksni::blocking::TrayMethods;
use ksni::menu::{CheckmarkItem, StandardItem, SubMenu};
use ksni::{Icon, MenuItem};
use std::sync::Arc;
use std::sync::atomic::Ordering;

const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

pub struct Tray {
    shared: Arc<Shared>,
    icon: Vec<Icon>,
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "vc-overlay".into()
    }

    fn title(&self) -> String {
        "VC Overlay".into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        self.icon.clone()
    }

    /// Left click toggles edit mode.
    fn activate(&mut self, _x: i32, _y: i32) {
        toggle_edit(&self.shared);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let (status, channel) = {
            let v = self.shared.voice();
            (v.status.clone(), v.channel_name.clone())
        };
        let status_label = match (&status, channel) {
            (Status::NoDiscord, _) => "Waiting for Discord…".to_string(),
            (Status::Authorizing, _) => "Approve the prompt in Discord".to_string(),
            (Status::Error(e), _) => e.clone(),
            (Status::Connected, Some(c)) => format!("In voice: {c}"),
            (Status::Connected, None) => "Connected, not in voice".to_string(),
        };
        let cfg = self.shared.config().clone();
        let editing = self.shared.is_editing();

        vec![
            StandardItem { label: status_label, enabled: false, ..Default::default() }.into(),
            MenuItem::Separator,
            StandardItem {
                label: if editing { "Lock position".into() } else { "Move overlay".into() },
                activate: Box::new(|t: &mut Self| toggle_edit(&t.shared)),
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: "Only show who's talking".into(),
                checked: cfg.only_speaking,
                activate: Box::new(|t: &mut Self| t.shared.update_config(|c| c.only_speaking = !c.only_speaking)),
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: "Show names".into(),
                checked: cfg.show_names,
                activate: Box::new(|t: &mut Self| t.shared.update_config(|c| c.show_names = !c.show_names)),
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: "Show channel name".into(),
                checked: cfg.show_channel,
                activate: Box::new(|t: &mut Self| t.shared.update_config(|c| c.show_channel = !c.show_channel)),
                ..Default::default()
            }
            .into(),
            choice(
                "Screen edge",
                &Edge::ALL.map(|e| (e.label(), e)),
                cfg.edge,
                |c, e| {
                    c.edge = e;
                },
            ),
            choice(
                "Max people",
                &[("3", 3), ("5", 5), ("8", 8), ("12", 12), ("20", 20), ("No limit", 0)],
                cfg.max_entries,
                |c, n| c.max_entries = n,
            ),
            choice(
                "Avatar size",
                &[("Small", 24), ("Medium", 32), ("Large", 40), ("Huge", 52)],
                cfg.avatar_size.round() as u32,
                |c, n| c.avatar_size = n as f32,
            ),
            SubMenu { label: "Monitor".into(), submenu: self.monitor_items(cfg.output.as_deref()), ..Default::default() }
                .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Reconnect to Discord (re-authorize)".into(),
                activate: Box::new(|t: &mut Self| t.shared.reauth.store(true, Ordering::Relaxed)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|t: &mut Self| {
                    t.shared.quit.store(true, Ordering::Relaxed);
                    t.shared.repaint();
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

impl Tray {
    fn monitor_items(&self, current: Option<&str>) -> Vec<MenuItem<Self>> {
        let outputs = self.shared.outputs.lock().map(|o| o.clone()).unwrap_or_default();
        let mut items: Vec<MenuItem<Self>> = vec![
            CheckmarkItem {
                label: "Automatic".into(),
                checked: current.is_none(),
                activate: Box::new(|t: &mut Self| t.shared.update_config(|c| c.output = None)),
                ..Default::default()
            }
            .into(),
        ];
        for (name, desc) in outputs {
            let checked = current == Some(name.as_str());
            items.push(
                CheckmarkItem {
                    label: desc,
                    checked,
                    activate: Box::new(move |t: &mut Self| {
                        let name = name.clone();
                        t.shared.update_config(|c| c.output = Some(name));
                    }),
                    ..Default::default()
                }
                .into(),
            );
        }
        items
    }
}

/// A submenu of mutually exclusive options.
fn choice<V: Copy + PartialEq + Send + Sync + 'static>(
    label: &str,
    options: &[(&str, V)],
    current: V,
    set: fn(&mut Config, V),
) -> MenuItem<Tray> {
    let submenu = options
        .iter()
        .map(|&(name, value)| {
            CheckmarkItem {
                label: name.into(),
                checked: value == current,
                activate: Box::new(move |t: &mut Tray| t.shared.update_config(|c| set(c, value))),
                ..Default::default()
            }
            .into()
        })
        .collect();
    SubMenu { label: label.into(), submenu, ..Default::default() }.into()
}

fn toggle_edit(shared: &Shared) {
    shared.editing.fetch_xor(true, Ordering::Relaxed);
    shared.repaint();
}

fn load_icon() -> Vec<Icon> {
    let Ok(img) = image::load_from_memory(ICON_PNG) else { return Vec::new() };
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    // StatusNotifierItem wants ARGB32 in network byte order.
    let data = rgba.pixels().flat_map(|p| [p[3], p[0], p[1], p[2]]).collect();
    vec![Icon { width: w as i32, height: h as i32, data }]
}

/// Starts the tray icon; returns a handle used to refresh the menu when state changes.
pub fn spawn(shared: Arc<Shared>) -> Option<ksni::blocking::Handle<Tray>> {
    let tray = Tray { shared, icon: load_icon() };
    match tray.spawn() {
        Ok(h) => Some(h),
        Err(e) => {
            eprintln!("vc-overlay: tray unavailable ({e}); edit with --edit instead");
            None
        }
    }
}
