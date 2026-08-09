use super::{
    egui, BindableControl, BindingAction, EditorSelection, KeyboardKey, Modifier, MouseButton,
    PadFeedbackConfig, PadFeedbackStrength, PadSide, MUTED_TEXT,
};

pub(super) fn mouse_button_editor(
    ui: &mut egui::Ui,
    control: BindableControl,
    button: &mut MouseButton,
) {
    ui.label("Mouse button");
    egui::ComboBox::from_id_salt(("mouse", control))
        .width(ui.available_width())
        .selected_text(button.label())
        .show_ui(ui, |ui| {
            for candidate in MouseButton::ALL {
                ui.selectable_value(button, candidate, candidate.label());
            }
        });
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new("The button stays held for as long as the controller control is held.")
            .small()
            .color(MUTED_TEXT),
    );
}

pub(super) fn selection_description(selection: EditorSelection) -> &'static str {
    match selection {
        EditorSelection::Button(BindableControl::L4) => "Upper left rear grip",
        EditorSelection::Button(BindableControl::L5) => "Lower left rear grip",
        EditorSelection::Button(BindableControl::R4) => "Upper right rear grip",
        EditorSelection::Button(BindableControl::R5) => "Lower right rear grip",
        EditorSelection::Button(BindableControl::QuickAccess) => "Front Quick Access button",
        EditorSelection::Button(BindableControl::LeftPadClick) => "Left trackpad click",
        EditorSelection::Button(BindableControl::RightPadClick) => "Right trackpad click",
        EditorSelection::Pad(PadSide::Left) => "Two-axis smooth desktop scrolling, bindable click",
        EditorSelection::Pad(PadSide::Right) => "Relative desktop pointer movement, bindable click",
    }
}

pub(super) fn binding_summary(action: Option<&BindingAction>) -> String {
    match action {
        None => "Unbound".to_owned(),
        Some(BindingAction::MouseButton { button }) => format!("{} Mouse", button.label()),
        Some(BindingAction::KeyChord { key, modifiers }) => {
            let mut summary = String::new();
            for modifier in Modifier::ALL {
                if modifiers.contains(&modifier) {
                    summary.push_str(match modifier {
                        Modifier::Command => "⌘",
                        Modifier::Control => "⌃",
                        Modifier::Option => "⌥",
                        Modifier::Shift => "⇧",
                    });
                }
            }
            summary.push_str(&key.label());
            summary
        }
    }
}

// ---------------------------------------------------------------------------
// Canvas layout
// ---------------------------------------------------------------------------

pub(super) fn pad_feedback_editor(ui: &mut egui::Ui, feedback: &mut PadFeedbackConfig) {
    ui.checkbox(&mut feedback.enabled, "Pad feedback");
    ui.label(
        egui::RichText::new(
            "Emit a click tick and subtle movement ticks that become faster with motion.",
        )
        .small()
        .color(MUTED_TEXT),
    );
    ui.add_space(12.0);
    ui.add_enabled_ui(feedback.enabled, |ui| {
        ui.label("Feedback strength");
        ui.horizontal(|ui| {
            for strength in PadFeedbackStrength::ALL {
                ui.selectable_value(&mut feedback.strength, strength, strength.label());
            }
        });
    });
}

#[allow(clippy::too_many_lines)]
pub(super) const fn keyboard_key(key: egui::Key) -> Option<KeyboardKey> {
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
