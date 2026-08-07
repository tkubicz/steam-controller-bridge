//! Shared visual tokens for the menu app's two eframe surfaces.

use eframe::egui;

pub const ACCENT: egui::Color32 = egui::Color32::from_rgb(84, 211, 224);
pub const SURFACE: egui::Color32 = egui::Color32::from_rgb(29, 34, 42);
pub const SURFACE_RAISED: egui::Color32 = egui::Color32::from_rgb(36, 42, 51);
pub const MUTED_TEXT: egui::Color32 = egui::Color32::from_rgb(157, 166, 177);
#[cfg(feature = "overlay")]
pub const TEXT: egui::Color32 = egui::Color32::from_rgb(232, 237, 243);
pub const ON_ACCENT: egui::Color32 = egui::Color32::from_rgb(8, 28, 32);
