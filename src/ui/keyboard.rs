use std::cell::RefCell;
use std::rc::Rc;

use adw::StyleManager;
use gtk::cairo;
use gtk::prelude::*;

use crate::model::Model;
use crate::protocol::KeyLayer;

#[derive(Debug, Clone)]
struct KeyVisual {
    matrix: usize,
    label: String,
    sub: String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

pub struct KeyboardState {
    keys: Vec<KeyVisual>,
    bindings: Vec<[u8; 4]>,
    defaults: Vec<[u8; 4]>,
    selected: Option<usize>,
    bounds: (f64, f64),
}

impl KeyboardState {
    fn is_modified(&self, matrix: usize) -> bool {
        self.bindings.get(matrix).copied().unwrap_or([0; 4])
            != self.defaults.get(matrix).copied().unwrap_or([0; 4])
    }
}

pub struct KeyboardView {
    pub area: gtk::DrawingArea,
    pub frame: gtk::AspectFrame,
    state: Rc<RefCell<KeyboardState>>,
}

impl KeyboardView {
    pub fn new(model: &Model, on_select: impl Fn(usize) + 'static) -> Self {
        let keys = model
            .keys
            .iter()
            .map(|key| {
                let (label, sub) = key.label_parts();
                KeyVisual {
                    matrix: key.matrix,
                    label: label.to_string(),
                    sub: sub.to_string(),
                    x: key.x,
                    y: key.y,
                    w: key.w,
                    h: key.h,
                }
            })
            .collect();
        let state = Rc::new(RefCell::new(KeyboardState {
            keys,
            bindings: vec![[0; 4]; model.matrix.len()],
            defaults: vec![[0; 4]; model.matrix.len()],
            selected: None,
            bounds: (model.bounds.w, model.bounds.h),
        }));

        let area = gtk::DrawingArea::new();
        area.set_content_width(model.bounds.w as i32);
        area.set_content_height(model.bounds.h as i32);
        area.set_hexpand(true);
        area.add_css_class("keyboard-drawing");

        let draw_state = state.clone();
        area.set_draw_func(move |_, cr, width, height| {
            draw_keyboard(cr, width, height, &draw_state.borrow());
        });

        let click = gtk::GestureClick::new();
        let click_state = state.clone();
        let click_area = area.clone();
        let callback: Rc<dyn Fn(usize)> = Rc::new(on_select);
        click.connect_pressed(move |_, _, x, y| {
            let width = click_area.width() as f64;
            let height = click_area.height() as f64;
            let Some(key) = hit_test(&click_state.borrow(), x, y, width, height) else {
                return;
            };
            callback(key);
        });
        area.add_controller(click);

        let style_area = area.downgrade();
        StyleManager::default().connect_dark_notify(move |_| {
            if let Some(area) = style_area.upgrade() {
                area.queue_draw();
            }
        });

        let frame = gtk::AspectFrame::builder()
            .obey_child(false)
            .ratio((model.bounds.w / model.bounds.h) as f32)
            .xalign(0.5)
            .yalign(0.5)
            .hexpand(true)
            .build();
        frame.set_child(Some(&area));
        frame.add_css_class("keyboard-frame");

        Self { area, frame, state }
    }

    pub fn set_layer(&self, model: &Model, layer: usize, keys: &KeyLayer) {
        let mut state = self.state.borrow_mut();
        state.bindings = keys.keys.clone();
        let mut by_matrix = vec![[0u8; 4]; model.matrix.len()];
        for key in &model.keys {
            by_matrix[key.matrix] = match layer {
                0 => key.default,
                1 => key.fn_layer,
                _ => key.fn2,
            };
        }
        state.defaults = by_matrix;
        state.selected = state.selected.filter(|matrix| *matrix < keys.keys.len());
        drop(state);
        self.area.queue_draw();
    }

    pub fn set_selected(&self, selected: Option<usize>) {
        self.state.borrow_mut().selected = selected;
        self.area.queue_draw();
    }

    pub fn binding(&self, matrix: usize) -> Option<[u8; 4]> {
        self.state.borrow().bindings.get(matrix).copied()
    }

    pub fn queue_draw(&self) {
        self.area.queue_draw();
    }
}

fn hit_test(state: &KeyboardState, x: f64, y: f64, width: f64, height: f64) -> Option<usize> {
    let (s, ox, oy) = scale_for(state, width, height);
    let dx = (x - ox) / s;
    let dy = (y - oy) / s;
    state
        .keys
        .iter()
        .find(|k| dx >= k.x && dx <= k.x + k.w && dy >= k.y && dy <= k.y + k.h)
        .map(|k| k.matrix)
}

fn scale_for(state: &KeyboardState, width: f64, height: f64) -> (f64, f64, f64) {
    let s = (width / state.bounds.0).min(height / state.bounds.1);
    let ox = (width - state.bounds.0 * s) / 2.0;
    let oy = (height - state.bounds.1 * s) / 2.0;
    (s, ox, oy)
}

fn rounded_rect(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0);
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, std::f64::consts::FRAC_PI_2);
    cr.arc(
        x + r,
        y + h - r,
        r,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    );
    cr.arc(
        x + r,
        y + r,
        r,
        std::f64::consts::PI,
        1.5 * std::f64::consts::PI,
    );
    cr.close_path();
}

fn draw_keyboard(cr: &cairo::Context, width: i32, height: i32, state: &KeyboardState) {
    let width = width as f64;
    let height = height as f64;
    let (s, ox, oy) = scale_for(state, width, height);
    let dark = StyleManager::default().is_dark();
    let (bg, frame_border, key_bg, key_border, text, sub, selected_border, modified_border) =
        if dark {
            (
                (0.15, 0.15, 0.16),
                (0.28, 0.28, 0.31),
                (0.20, 0.20, 0.22),
                (0.31, 0.31, 0.34),
                (0.95, 0.95, 0.96),
                (0.66, 0.66, 0.70),
                (0.55, 0.75, 1.0),
                (0.96, 0.83, 0.35),
            )
        } else {
            (
                (0.97, 0.97, 0.96),
                (0.82, 0.82, 0.82),
                (1.0, 1.0, 1.0),
                (0.79, 0.79, 0.79),
                (0.12, 0.12, 0.13),
                (0.42, 0.42, 0.45),
                (0.20, 0.45, 0.80),
                (0.66, 0.47, 0.0),
            )
        };

    rounded_rect(cr, ox, oy, state.bounds.0 * s, state.bounds.1 * s, 16.0 * s);
    cr.set_source_rgb(bg.0, bg.1, bg.2);
    let _ = cr.fill();
    cr.set_line_width(1.0);
    cr.set_source_rgba(frame_border.0, frame_border.1, frame_border.2, 0.7);
    let _ = cr.stroke();

    for key in &state.keys {
        let x = ox + key.x * s + 1.5;
        let y = oy + key.y * s + 1.5;
        let w = key.w * s - 3.0;
        let h = key.h * s - 3.0;
        if w <= 2.0 || h <= 2.0 {
            continue;
        }
        let selected = state.selected == Some(key.matrix);
        let modified = state.is_modified(key.matrix);
        rounded_rect(cr, x, y, w, h, (6.0 * s).clamp(3.0, 10.0));
        if selected {
            cr.set_source_rgb(0.208, 0.518, 0.894);
        } else {
            cr.set_source_rgb(key_bg.0, key_bg.1, key_bg.2);
        }
        let _ = cr.fill();
        if selected {
            cr.set_source_rgba(selected_border.0, selected_border.1, selected_border.2, 0.9);
            cr.set_line_width(1.5);
        } else if modified {
            cr.set_source_rgba(modified_border.0, modified_border.1, modified_border.2, 0.8);
            cr.set_line_width(1.2);
        } else {
            cr.set_source_rgba(key_border.0, key_border.1, key_border.2, 0.85);
            cr.set_line_width(1.0);
        }
        rounded_rect(cr, x, y, w, h, (6.0 * s).clamp(3.0, 10.0));
        let _ = cr.stroke();

        let cx = x + w / 2.0;
        let cy = y + h / 2.0;
        cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
        if selected {
            cr.set_source_rgb(1.0, 1.0, 1.0);
        } else {
            cr.set_source_rgb(text.0, text.1, text.2);
        }
        if key.sub.is_empty() {
            let size = fit_font(&key.label, w, 11.0 * s);
            cr.set_font_size(size);
            draw_text_centered(cr, &key.label, cx, cy);
        } else {
            let size = fit_font(&key.label, w, 10.5 * s).min(12.0);
            cr.set_font_size(size);
            draw_text_centered(cr, &key.label, cx, cy - 3.0 * s);
            if selected {
                cr.set_source_rgba(0.9, 0.95, 1.0, 1.0);
            } else {
                cr.set_source_rgba(sub.0, sub.1, sub.2, 1.0);
            }
            let subsize = (8.0 * s).max(6.0);
            cr.set_font_size(subsize);
            draw_text_centered(cr, &key.sub, cx, cy + 9.0 * s);
        }
    }
}

fn fit_font(label: &str, width: f64, desired: f64) -> f64 {
    let approx = width / (label.chars().count().max(1) as f64 * 0.62);
    desired.min(approx).max(5.5)
}

fn draw_text_centered(cr: &cairo::Context, text: &str, cx: f64, cy: f64) {
    let Ok(ext) = cr.text_extents(text) else {
        return;
    };
    cr.move_to(
        cx - ext.width() / 2.0 - ext.x_bearing(),
        cy - ext.height() / 2.0 - ext.y_bearing(),
    );
    let _ = cr.show_text(text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_font_shrinks_for_long_labels() {
        assert!(fit_font("A", 40.0, 11.0) > fit_font("Backspace", 40.0, 11.0));
    }
}
