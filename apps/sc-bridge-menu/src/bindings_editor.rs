use std::collections::BTreeSet;

use desktop_bindings::{
    default_store_path, load_or_create_store, save_store, BindableControl, BindingAction,
    BindingProfile, BindingStore, ControlBindings, KeyboardKey, Modifier, MouseButton,
};
use eframe::egui;

pub fn run() -> Result<(), String> {
    let path = default_store_path()?;
    let store = load_or_create_store(&path)?;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([760.0, 620.0]),
        ..eframe::NativeOptions::default()
    };
    eframe::run_native(
        "Steam Controller Bridge Bindings",
        options,
        Box::new(move |_creation| Ok(Box::new(BindingsEditor::new(path, store)))),
    )
    .map_err(|error| error.to_string())
}

struct BindingsEditor {
    path: std::path::PathBuf,
    store: BindingStore,
    selected: usize,
    capturing: Option<BindableControl>,
    message: Option<String>,
}

impl BindingsEditor {
    fn new(path: std::path::PathBuf, store: BindingStore) -> Self {
        Self {
            path,
            store,
            selected: 0,
            capturing: None,
            message: None,
        }
    }

    fn unique_name(&self, base: &str) -> String {
        if !self
            .store
            .profiles
            .iter()
            .any(|profile| profile.name.eq_ignore_ascii_case(base))
        {
            return base.to_owned();
        }
        (2..=desktop_bindings::MAX_PROFILES + 1)
            .map(|number| format!("{base} {number}"))
            .find(|name| {
                !self
                    .store
                    .profiles
                    .iter()
                    .any(|profile| profile.name.eq_ignore_ascii_case(name))
            })
            .expect("an unused bounded profile name exists")
    }

    fn add_profile(&mut self) {
        if self.store.profiles.len() >= desktop_bindings::MAX_PROFILES {
            self.message = Some("At most 32 profiles are supported".to_owned());
            return;
        }
        let id = self.store.next_profile_id();
        let name = self.unique_name("New Profile");
        self.store.profiles.push(BindingProfile {
            id,
            name,
            bindings: ControlBindings::default(),
        });
        self.selected = self.store.profiles.len() - 1;
    }

    fn duplicate_profile(&mut self) {
        if self.store.profiles.len() >= desktop_bindings::MAX_PROFILES {
            self.message = Some("At most 32 profiles are supported".to_owned());
            return;
        }
        let Some(source) = self.store.profiles.get(self.selected).cloned() else {
            return;
        };
        let mut duplicate = source;
        duplicate.id = self.store.next_profile_id();
        duplicate.name = self.unique_name(&format!("{} Copy", duplicate.name));
        self.store.profiles.push(duplicate);
        self.selected = self.store.profiles.len() - 1;
    }

    fn delete_profile(&mut self) {
        if self.store.profiles.len() == 1 {
            self.message = Some("The last profile cannot be deleted".to_owned());
            return;
        }
        self.store.profiles.remove(self.selected);
        self.selected = self.selected.min(self.store.profiles.len() - 1);
        self.capturing = None;
    }

    fn capture_key(&mut self, ctx: &egui::Context) {
        let Some(control) = self.capturing else {
            return;
        };
        let captured = ctx.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Key {
                    key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } => keyboard_key(*key).map(|key| (key, *modifiers)),
                _ => None,
            })
        });
        let Some((key, modifiers)) = captured else {
            return;
        };
        let mut binding_modifiers = BTreeSet::new();
        if modifiers.mac_cmd {
            binding_modifiers.insert(Modifier::Command);
        }
        if modifiers.ctrl {
            binding_modifiers.insert(Modifier::Control);
        }
        if modifiers.alt {
            binding_modifiers.insert(Modifier::Option);
        }
        if modifiers.shift {
            binding_modifiers.insert(Modifier::Shift);
        }
        *self.store.profiles[self.selected].bindings.get_mut(control) =
            Some(BindingAction::KeyChord {
                key,
                modifiers: binding_modifiers,
            });
        self.capturing = None;
    }

    fn binding_row(&mut self, ui: &mut egui::Ui, control: BindableControl) {
        let current = self.store.profiles[self.selected]
            .bindings
            .get(control)
            .cloned();
        let mut kind = match current {
            None => 0,
            Some(BindingAction::KeyChord { .. }) => 1,
            Some(BindingAction::MouseButton { .. }) => 2,
        };
        ui.horizontal(|ui| {
            ui.label(control.label());
            egui::ComboBox::from_id_salt(("binding-kind", control))
                .selected_text(match kind {
                    1 => "Key chord",
                    2 => "Mouse button",
                    _ => "Unbound",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut kind, 0, "Unbound");
                    ui.selectable_value(&mut kind, 1, "Key chord");
                    ui.selectable_value(&mut kind, 2, "Mouse button");
                });

            let mut replacement = match (kind, current) {
                (1, Some(action @ BindingAction::KeyChord { .. }))
                | (2, Some(action @ BindingAction::MouseButton { .. })) => Some(action),
                (1, _) => Some(BindingAction::KeyChord {
                    key: KeyboardKey::F5,
                    modifiers: BTreeSet::new(),
                }),
                (2, _) => Some(BindingAction::MouseButton {
                    button: MouseButton::Middle,
                }),
                _ => None,
            };

            match replacement.as_mut() {
                Some(BindingAction::KeyChord { key, modifiers }) => {
                    egui::ComboBox::from_id_salt(("key", control))
                        .selected_text(key.label())
                        .show_ui(ui, |ui| {
                            for candidate in KeyboardKey::ALL {
                                ui.selectable_value(key, *candidate, candidate.label());
                            }
                        });
                    for modifier in Modifier::ALL {
                        let mut selected = modifiers.contains(&modifier);
                        if ui.checkbox(&mut selected, modifier.label()).changed() {
                            if selected {
                                modifiers.insert(modifier);
                            } else {
                                modifiers.remove(&modifier);
                            }
                        }
                    }
                    let capture_label = if self.capturing == Some(control) {
                        "Press a key…"
                    } else {
                        "Capture"
                    };
                    if ui.button(capture_label).clicked() {
                        self.capturing = Some(control);
                    }
                }
                Some(BindingAction::MouseButton { button }) => {
                    egui::ComboBox::from_id_salt(("mouse", control))
                        .selected_text(button.label())
                        .show_ui(ui, |ui| {
                            for candidate in MouseButton::ALL {
                                ui.selectable_value(button, candidate, candidate.label());
                            }
                        });
                }
                None => {}
            }
            *self.store.profiles[self.selected].bindings.get_mut(control) = replacement;
        });
    }
}

impl eframe::App for BindingsEditor {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.capture_key(ui.ctx());
        ui.heading("Desktop bindings");
        ui.label(
            "Map the extra grips and Quick Access button to held keyboard chords or mouse buttons.",
        );
        ui.separator();
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.set_width(180.0);
                for (index, profile) in self.store.profiles.iter().enumerate() {
                    if ui
                        .selectable_label(index == self.selected, &profile.name)
                        .clicked()
                    {
                        self.selected = index;
                        self.capturing = None;
                    }
                }
                ui.horizontal(|ui| {
                    if ui.button("New").clicked() {
                        self.add_profile();
                    }
                    if ui.button("Duplicate").clicked() {
                        self.duplicate_profile();
                    }
                });
                if ui.button("Delete").clicked() {
                    self.delete_profile();
                }
            });
            ui.separator();
            ui.vertical(|ui| {
                ui.label("Profile name");
                ui.text_edit_singleline(&mut self.store.profiles[self.selected].name);
                ui.add_space(8.0);
                for control in BindableControl::ALL {
                    self.binding_row(ui, control);
                    ui.separator();
                }
            });
        });
        if let Some(message) = &self.message {
            ui.colored_label(egui::Color32::RED, message);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Cancel").clicked() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
            if ui.button("Save").clicked() {
                for profile in &mut self.store.profiles {
                    profile.name = profile.name.trim().to_owned();
                }
                match save_store(&self.path, &self.store) {
                    Ok(()) => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                    Err(error) => self.message = Some(error),
                }
            }
        });
    }
}

#[allow(clippy::too_many_lines)]
const fn keyboard_key(key: egui::Key) -> Option<KeyboardKey> {
    Some(match key {
        egui::Key::A => KeyboardKey::A,
        egui::Key::B => KeyboardKey::B,
        egui::Key::C => KeyboardKey::C,
        egui::Key::D => KeyboardKey::D,
        egui::Key::E => KeyboardKey::E,
        egui::Key::F => KeyboardKey::F,
        egui::Key::G => KeyboardKey::G,
        egui::Key::H => KeyboardKey::H,
        egui::Key::I => KeyboardKey::I,
        egui::Key::J => KeyboardKey::J,
        egui::Key::K => KeyboardKey::K,
        egui::Key::L => KeyboardKey::L,
        egui::Key::M => KeyboardKey::M,
        egui::Key::N => KeyboardKey::N,
        egui::Key::O => KeyboardKey::O,
        egui::Key::P => KeyboardKey::P,
        egui::Key::Q => KeyboardKey::Q,
        egui::Key::R => KeyboardKey::R,
        egui::Key::S => KeyboardKey::S,
        egui::Key::T => KeyboardKey::T,
        egui::Key::U => KeyboardKey::U,
        egui::Key::V => KeyboardKey::V,
        egui::Key::W => KeyboardKey::W,
        egui::Key::X => KeyboardKey::X,
        egui::Key::Y => KeyboardKey::Y,
        egui::Key::Z => KeyboardKey::Z,
        egui::Key::Num0 => KeyboardKey::Digit0,
        egui::Key::Num1 => KeyboardKey::Digit1,
        egui::Key::Num2 => KeyboardKey::Digit2,
        egui::Key::Num3 => KeyboardKey::Digit3,
        egui::Key::Num4 => KeyboardKey::Digit4,
        egui::Key::Num5 => KeyboardKey::Digit5,
        egui::Key::Num6 => KeyboardKey::Digit6,
        egui::Key::Num7 => KeyboardKey::Digit7,
        egui::Key::Num8 => KeyboardKey::Digit8,
        egui::Key::Num9 => KeyboardKey::Digit9,
        egui::Key::F1 => KeyboardKey::F1,
        egui::Key::F2 => KeyboardKey::F2,
        egui::Key::F3 => KeyboardKey::F3,
        egui::Key::F4 => KeyboardKey::F4,
        egui::Key::F5 => KeyboardKey::F5,
        egui::Key::F6 => KeyboardKey::F6,
        egui::Key::F7 => KeyboardKey::F7,
        egui::Key::F8 => KeyboardKey::F8,
        egui::Key::F9 => KeyboardKey::F9,
        egui::Key::F10 => KeyboardKey::F10,
        egui::Key::F11 => KeyboardKey::F11,
        egui::Key::F12 => KeyboardKey::F12,
        egui::Key::F13 => KeyboardKey::F13,
        egui::Key::F14 => KeyboardKey::F14,
        egui::Key::F15 => KeyboardKey::F15,
        egui::Key::F16 => KeyboardKey::F16,
        egui::Key::F17 => KeyboardKey::F17,
        egui::Key::F18 => KeyboardKey::F18,
        egui::Key::F19 => KeyboardKey::F19,
        egui::Key::F20 => KeyboardKey::F20,
        egui::Key::F21 => KeyboardKey::F21,
        egui::Key::F22 => KeyboardKey::F22,
        egui::Key::F23 => KeyboardKey::F23,
        egui::Key::F24 => KeyboardKey::F24,
        egui::Key::Escape => KeyboardKey::Escape,
        egui::Key::Tab => KeyboardKey::Tab,
        egui::Key::Enter => KeyboardKey::Return,
        egui::Key::Space => KeyboardKey::Space,
        egui::Key::Backspace => KeyboardKey::Backspace,
        egui::Key::Delete => KeyboardKey::Delete,
        egui::Key::Insert => KeyboardKey::Insert,
        egui::Key::Home => KeyboardKey::Home,
        egui::Key::End => KeyboardKey::End,
        egui::Key::PageUp => KeyboardKey::PageUp,
        egui::Key::PageDown => KeyboardKey::PageDown,
        egui::Key::ArrowLeft => KeyboardKey::ArrowLeft,
        egui::Key::ArrowRight => KeyboardKey::ArrowRight,
        egui::Key::ArrowUp => KeyboardKey::ArrowUp,
        egui::Key::ArrowDown => KeyboardKey::ArrowDown,
        egui::Key::Backtick => KeyboardKey::Grave,
        egui::Key::Minus => KeyboardKey::Minus,
        egui::Key::Equals | egui::Key::Plus => KeyboardKey::Equal,
        egui::Key::OpenBracket => KeyboardKey::LeftBracket,
        egui::Key::CloseBracket => KeyboardKey::RightBracket,
        egui::Key::Backslash | egui::Key::Pipe => KeyboardKey::Backslash,
        egui::Key::Semicolon | egui::Key::Colon => KeyboardKey::Semicolon,
        egui::Key::Quote => KeyboardKey::Quote,
        egui::Key::Comma => KeyboardKey::Comma,
        egui::Key::Period => KeyboardKey::Period,
        egui::Key::Slash | egui::Key::Questionmark => KeyboardKey::Slash,
        _ => return None,
    })
}
