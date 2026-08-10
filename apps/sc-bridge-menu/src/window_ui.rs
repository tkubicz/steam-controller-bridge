use eframe::egui;
use objc2::MainThreadMarker;
use objc2_app_kit::NSApplication;
use ui_theme::{ACCENT, BORDER, PANEL, SURFACE, SURFACE_RAISED, TEXT};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ReleaseNotes {
    pub(crate) title: InlineText,
    pub(crate) sections: Vec<ReleaseSection>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ReleaseSection {
    pub(crate) title: String,
    pub(crate) notes: Vec<InlineText>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct InlineText {
    pub(crate) spans: Vec<InlineSpan>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct InlineSpan {
    pub(crate) text: String,
    pub(crate) url: Option<String>,
}

pub(crate) fn configure_window_style(ctx: &egui::Context) {
    ui_theme::configure_ui(ctx);
    let theme = ctx.theme();
    let mut style = (*ctx.style_of(theme)).clone();
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(14.0, 8.0);
    style
        .text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(15.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
    ctx.set_style_of(theme, style);
}

#[allow(deprecated)] // `activate()` requires macOS 14; the app supports macOS 13.
pub(crate) fn activate_window() {
    let mtm = MainThreadMarker::new().expect("eframe starts on the main thread");
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
}

pub(crate) fn hero_transition(ui: &mut egui::Ui) {
    const HEIGHT: f32 = 28.0;
    const STEPS: u8 = 14;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), HEIGHT),
        egui::Sense::hover(),
    );
    let start = egui::Rgba::from(SURFACE);
    let end = egui::Rgba::from(PANEL);
    for step in 0..STEPS {
        let top = f32::from(step) / f32::from(STEPS);
        let bottom = f32::from(step + 1) / f32::from(STEPS);
        let strip = egui::Rect::from_min_max(
            egui::pos2(rect.left(), egui::lerp(rect.top()..=rect.bottom(), top)),
            egui::pos2(rect.right(), egui::lerp(rect.top()..=rect.bottom(), bottom)),
        );
        ui.painter().rect_filled(
            strip,
            0.0,
            egui::Color32::from(egui::lerp(start..=end, bottom)),
        );
    }
}

pub(crate) fn full_width_card<R>(
    ui: &mut egui::Ui,
    padding: i8,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let inner_width = (ui.available_width() - f32::from(padding) * 2.0 - 2.0).max(0.0);
    egui::Frame::new()
        .fill(SURFACE_RAISED)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(12)
        .inner_margin(egui::Margin::same(padding))
        .show(ui, |ui| {
            ui.set_width(inner_width);
            add_contents(ui)
        })
}

pub(crate) fn load_texture(ctx: &egui::Context, name: &str, png: &[u8]) -> egui::TextureHandle {
    let icon = eframe::icon_data::from_png_bytes(png).expect("embedded PNG asset is valid");
    let size = [icon.width as usize, icon.height as usize];
    let image = egui::ColorImage::from_rgba_unmultiplied(size, &icon.rgba);
    ctx.load_texture(
        name,
        image,
        egui::TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear)),
    )
}

pub(crate) fn parse_release_notes(markdown: &str) -> Vec<ReleaseNotes> {
    let mut releases = Vec::new();
    let mut release: Option<ReleaseNotes> = None;
    let mut section_index: Option<usize> = None;

    for line in markdown.lines() {
        if let Some(title) = line.strip_prefix("## ") {
            if let Some(previous) = release.take() {
                releases.push(previous);
            }
            release = Some(ReleaseNotes {
                title: parse_inline(title),
                sections: Vec::new(),
            });
            section_index = None;
        } else if let Some(title) = line.strip_prefix("### ") {
            let Some(current) = release.as_mut() else {
                continue;
            };
            current.sections.push(ReleaseSection {
                title: parse_inline(title).plain(),
                notes: Vec::new(),
            });
            section_index = Some(current.sections.len() - 1);
        } else if line.starts_with("* ") || line.starts_with("- ") {
            let Some(current) = release.as_mut() else {
                continue;
            };
            let index = *section_index.get_or_insert_with(|| {
                current.sections.push(ReleaseSection {
                    title: "Overview".to_owned(),
                    notes: Vec::new(),
                });
                current.sections.len() - 1
            });
            current.sections[index].notes.push(parse_inline(&line[2..]));
        }
    }
    if let Some(last) = release {
        releases.push(last);
    }
    releases
}

fn parse_inline(markdown: &str) -> InlineText {
    let mut spans = Vec::new();
    let mut rest = markdown;
    while let Some(open) = rest.find('[') {
        push_span(&mut spans, &rest[..open], None);
        let after_open = &rest[open + 1..];
        let Some(close) = after_open.find("](") else {
            push_span(&mut spans, &rest[open..], None);
            rest = "";
            break;
        };
        let after_label = &after_open[close + 2..];
        let Some(end) = after_label.find(')') else {
            push_span(&mut spans, &rest[open..], None);
            rest = "";
            break;
        };
        push_span(
            &mut spans,
            &after_open[..close],
            Some(after_label[..end].to_owned()),
        );
        rest = &after_label[end + 1..];
    }
    push_span(&mut spans, rest, None);
    InlineText { spans }
}

fn push_span(spans: &mut Vec<InlineSpan>, text: &str, url: Option<String>) {
    let text = text.replace("**", "").replace('`', "");
    if !text.is_empty() {
        spans.push(InlineSpan { text, url });
    }
}

pub(crate) fn render_inline(ui: &mut egui::Ui, inline: &InlineText, size: f32, strong: bool) {
    for span in &inline.spans {
        let mut text = egui::RichText::new(&span.text).size(size);
        if strong {
            text = text.strong();
        }
        if let Some(url) = &span.url {
            ui.hyperlink_to(text.color(ACCENT), url);
        } else {
            ui.label(text.color(TEXT));
        }
    }
}

impl InlineText {
    pub(crate) fn plain(&self) -> String {
        self.spans.iter().map(|span| span.text.as_str()).collect()
    }
}
