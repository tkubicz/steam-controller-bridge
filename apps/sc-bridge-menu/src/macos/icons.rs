use super::{
    FillRule, Icon, LineCap, LineJoin, MainThreadMarker, NSImage, NSStatusBarButton, Paint,
    PathBuilder, Pixmap, Retained, SkiaPath, Stroke, Transform, TrayIcon, TrayState,
};

pub(super) struct NativeTrayIcons {
    pub(super) button: Retained<NSStatusBarButton>,
    pub(super) off: Retained<NSImage>,
    pub(super) waiting: Retained<NSImage>,
    pub(super) ready: Retained<NSImage>,
    pub(super) error: Retained<NSImage>,
}

impl NativeTrayIcons {
    pub(super) fn capture(tray: &TrayIcon) -> Result<Self, String> {
        let mtm =
            MainThreadMarker::new().ok_or("menu-bar icons must be created on the main thread")?;
        let status_item = tray
            .ns_status_item()
            .ok_or("tray-icon did not create a native macOS status item")?;
        let button = status_item
            .button(mtm)
            .ok_or("the native macOS status item has no button")?;
        let waiting = button
            .image()
            .ok_or("tray-icon did not install the initial menu-bar image")?;
        let render = |state| {
            tray.set_icon_with_as_template(Some(template_icon(state)?), true)
                .map_err(|error| error.to_string())?;
            button
                .image()
                .ok_or_else(|| "tray-icon did not install a menu-bar status image".to_owned())
        };
        let off = render(TrayState::Off)?;
        let ready = render(TrayState::Ready)?;
        let error = render(TrayState::Error)?;

        let icons = Self {
            button,
            off,
            waiting,
            ready,
            error,
        };
        icons.install(TrayState::Waiting);
        Ok(icons)
    }

    pub(super) fn install(&self, state: TrayState) {
        let image = match state {
            TrayState::Off => &self.off,
            TrayState::Waiting => &self.waiting,
            TrayState::Ready => &self.ready,
            TrayState::Error => &self.error,
        };
        self.button.setImage(Some(image));
    }
}

const ICON_LOGICAL_WIDTH: u32 = 24;
const ICON_LOGICAL_HEIGHT: u32 = 18;
pub(super) const ICON_RENDER_SCALE: u32 = 4;
const ICON_RENDER_SCALE_F32: f32 = 4.0;
pub(super) const ICON_WIDTH: u32 = ICON_LOGICAL_WIDTH * ICON_RENDER_SCALE;
pub(super) const ICON_HEIGHT: u32 = ICON_LOGICAL_HEIGHT * ICON_RENDER_SCALE;

pub(super) fn template_icon(state: TrayState) -> Result<Icon, String> {
    Icon::from_rgba(template_icon_rgba(state), ICON_WIDTH, ICON_HEIGHT)
        .map_err(|error| error.to_string())
}

pub(super) fn template_icon_rgba(state: TrayState) -> Vec<u8> {
    let mut pixmap =
        Pixmap::new(ICON_WIDTH, ICON_HEIGHT).expect("the fixed menu icon dimensions are valid");
    let mut paint = Paint::default();
    paint.set_color_rgba8(0, 0, 0, 255);
    paint.anti_alias = true;
    let transform = Transform::from_scale(ICON_RENDER_SCALE_F32, ICON_RENDER_SCALE_F32);

    stroke_icon_path(&mut pixmap, &controller_outline(), &paint, 1.4, transform);
    stroke_icon_path(&mut pixmap, &d_pad(), &paint, 1.3, transform);
    fill_icon_circle(&mut pixmap, &paint, 12.2, 7.1, 0.72, transform);
    fill_icon_circle(&mut pixmap, &paint, 14.2, 8.7, 0.72, transform);

    match state {
        TrayState::Off => {
            stroke_icon_path(&mut pixmap, &off_badge(), &paint, 1.5, transform);
        }
        TrayState::Waiting => {
            for x in [19.3, 21.2, 23.1] {
                fill_icon_circle(&mut pixmap, &paint, x, 9.0, 0.53, transform);
            }
        }
        TrayState::Ready => {
            stroke_icon_path(&mut pixmap, &ready_badge(), &paint, 1.55, transform);
        }
        TrayState::Error => {
            stroke_icon_path(&mut pixmap, &error_badge(), &paint, 1.55, transform);
            fill_icon_circle(&mut pixmap, &paint, 21.2, 12.8, 0.68, transform);
        }
    }

    pixmap.take()
}

pub(super) fn controller_outline() -> SkiaPath {
    let mut path = PathBuilder::new();
    path.move_to(5.5, 2.4);
    path.cubic_to(3.7, 2.4, 2.3, 3.6, 1.9, 5.3);
    path.line_to(0.65, 11.6);
    path.cubic_to(0.2, 13.8, 1.3, 15.9, 3.05, 16.45);
    path.cubic_to(4.5, 16.9, 5.5, 15.8, 6.25, 14.35);
    path.line_to(7.25, 12.5);
    path.cubic_to(7.55, 11.9, 7.95, 11.7, 8.55, 11.7);
    path.line_to(9.35, 11.7);
    path.cubic_to(9.95, 11.7, 10.35, 11.9, 10.65, 12.5);
    path.line_to(11.65, 14.35);
    path.cubic_to(12.4, 15.8, 13.4, 16.9, 14.85, 16.45);
    path.cubic_to(16.6, 15.9, 17.7, 13.8, 17.25, 11.6);
    path.line_to(16.0, 5.3);
    path.cubic_to(15.6, 3.6, 14.2, 2.4, 12.4, 2.4);
    path.close();
    path.finish()
        .expect("the static controller outline is a valid path")
}

pub(super) fn d_pad() -> SkiaPath {
    let mut path = PathBuilder::new();
    path.move_to(5.15, 6.4);
    path.line_to(5.15, 9.6);
    path.move_to(3.55, 8.0);
    path.line_to(6.75, 8.0);
    path.finish().expect("the static d-pad is a valid path")
}

pub(super) fn off_badge() -> SkiaPath {
    let mut path = PathBuilder::new();
    path.move_to(19.3, 7.1);
    path.line_to(22.9, 10.9);
    path.move_to(22.9, 7.1);
    path.line_to(19.3, 10.9);
    path.finish().expect("the static off badge is a valid path")
}

pub(super) fn ready_badge() -> SkiaPath {
    let mut path = PathBuilder::new();
    path.move_to(19.1, 9.1);
    path.line_to(20.7, 10.7);
    path.line_to(23.1, 6.8);
    path.finish()
        .expect("the static ready badge is a valid path")
}

pub(super) fn error_badge() -> SkiaPath {
    let mut path = PathBuilder::new();
    path.move_to(21.2, 5.5);
    path.line_to(21.2, 10.2);
    path.finish()
        .expect("the static error badge is a valid path")
}

pub(super) fn stroke_icon_path(
    pixmap: &mut Pixmap,
    path: &SkiaPath,
    paint: &Paint<'_>,
    width: f32,
    transform: Transform,
) {
    let stroke = Stroke {
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    pixmap.stroke_path(path, paint, &stroke, transform, None);
}

pub(super) fn fill_icon_circle(
    pixmap: &mut Pixmap,
    paint: &Paint<'_>,
    x: f32,
    y: f32,
    radius: f32,
    transform: Transform,
) {
    let path =
        PathBuilder::from_circle(x, y, radius).expect("the static icon circle is a valid path");
    pixmap.fill_path(&path, paint, FillRule::Winding, transform, None);
}
