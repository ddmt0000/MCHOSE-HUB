use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Instant;

use adw::prelude::*;
use gtk::{gdk, glib};

use super::{button_flow, connect_search, section, AppUi};
use crate::macro_table::{
    macro_action, macro_ref, Chip, MacroDef, MacroEvent, MacroType, NodeKind, DEFAULT_DELAY_MS,
    MAX_DELAY_MS, MAX_NAME_CHARS, MIN_NAME_CHARS,
};
use crate::model::Model;
use crate::protocol::KeyLayer;

/// Key names without a printable character of their own.
const KEY_NAMES: &[(&str, u8)] = &[
    ("Escape", 41),
    ("BackSpace", 42),
    ("Tab", 43),
    ("Return", 40),
    ("CapsLock", 57),
    ("Print", 70),
    ("ScrollLock", 71),
    ("Pause", 72),
    ("Insert", 73),
    ("Home", 74),
    ("Page_Up", 75),
    ("Delete", 76),
    ("End", 77),
    ("Page_Down", 78),
    ("Right", 79),
    ("Left", 80),
    ("Down", 81),
    ("Up", 82),
    ("NumLock", 83),
    ("Menu", 101),
    ("ContextMenu", 101),
    ("Shift_L", 225),
    ("Shift_R", 229),
    ("Control_L", 224),
    ("Control_R", 228),
    ("Alt_L", 226),
    ("Alt_R", 230),
    ("Meta_L", 227),
    ("Meta_R", 231),
    ("Super_L", 227),
    ("Super_R", 231),
    ("KP_Multiply", 85),
    ("KP_Add", 87),
    ("KP_Subtract", 86),
    ("KP_Decimal", 99),
    ("KP_Comma", 99),
    ("KP_Divide", 84),
    ("KP_Enter", 88),
];

thread_local! {
    /// Keeps the open editor alive; rows only hold a weak handle.
    static OPEN: RefCell<Option<Rc<Editor>>> = const { RefCell::new(None) };
}

/// HID usage of a key event. Shifted symbols fold onto their base key the way
/// the HUB does, so Shift+1 records as "1".
fn usage_from_keyval(keyval: gdk::Key) -> Option<u8> {
    if let Some(usage) = keyval.to_unicode().and_then(usage_from_char) {
        return Some(usage);
    }
    let name = keyval.name()?;
    let name = name.as_str();
    if let Some(digit) = name.strip_prefix("KP_").and_then(|rest| rest.chars().next()) {
        if let Some(value) = digit.to_digit(10) {
            return Some(if value == 0 { 98 } else { 88 + value as u8 });
        }
    }
    if let Some(function) = name.strip_prefix('F') {
        if let Ok(index) = function.parse::<u8>() {
            if (1..=12).contains(&index) {
                return Some(57 + index);
            }
        }
    }
    KEY_NAMES
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, usage)| *usage)
}

fn usage_from_char(c: char) -> Option<u8> {
    let usage: u32 = match c {
        'a'..='z' => 4 + c as u32 - 'a' as u32,
        'A'..='Z' => 4 + c as u32 - 'A' as u32,
        '1'..='9' => 30 + c as u32 - '1' as u32,
        // The shifted digits of the same row.
        '!' => 30,
        '@' => 31,
        '#' => 32,
        '$' => 33,
        '%' => 34,
        '^' => 35,
        '&' => 36,
        '*' => 37,
        '(' => 38,
        '0' | ')' => 39,
        ' ' => 44,
        '\t' => 43,
        '\r' | '\n' => 40,
        '\u{8}' => 42,
        '-' | '_' => 45,
        '=' | '+' => 46,
        '[' | '{' => 47,
        ']' | '}' => 48,
        '\\' | '|' => 49,
        ';' | ':' => 51,
        '\'' | '"' => 52,
        '`' | '~' => 53,
        ',' | '<' => 54,
        '.' | '>' => 55,
        '/' | '?' => 56,
        _ => return None,
    };
    u8::try_from(usage).ok()
}

fn event_label(model: &Model, event: MacroEvent) -> String {
    let state = if event.down { "按下" } else { "抬起" };
    format!("{}\n{state}", model.node_label(event.kind))
}

pub struct Editor {
    ui: Rc<AppUi>,
    model: Model,
    slot: usize,
    dialog: adw::Dialog,
    chips: RefCell<Vec<Chip>>,
    recording: Cell<bool>,
    last_event: RefCell<Option<Instant>>,
    flow: gtk::Box,
    status: gtk::Label,
    name: gtk::Entry,
    kind: adw::ComboRow,
    standard: adw::SwitchRow,
    gap: adw::SpinRow,
    record_button: gtk::Button,
    paused: Vec<gtk::Widget>,
    here: RefCell<Weak<Editor>>,
}

impl Editor {
    fn weak(&self) -> Weak<Editor> {
        self.here.borrow().clone()
    }

    fn gap_ms(&self) -> u32 {
        (self.gap.value().round() as u32).clamp(1, MAX_DELAY_MS)
    }

    fn selected_type(&self) -> MacroType {
        MacroType::ALL
            .get(self.kind.selected() as usize)
            .copied()
            .unwrap_or(MacroType::HoldLoop)
    }

    /// Gap written in front of a new event: the standard delay when switched on,
    /// otherwise the time since the previously recorded event.
    fn next_gap(&self) -> u32 {
        if self.standard.is_active() {
            return self.gap_ms();
        }
        match *self.last_event.borrow() {
            Some(at) => Instant::now().duration_since(at).as_millis() as u32,
            None => return self.gap_ms(),
        }
        .clamp(1, MAX_DELAY_MS)
    }

    /// A key that is already held must not add a second press node, which also
    /// filters out keyboard auto-repeat.
    fn is_held(&self, kind: NodeKind) -> bool {
        let mut held = false;
        for chip in self.chips.borrow().iter() {
            if let Chip::Event(event) = chip {
                if event.kind == kind {
                    held = event.down;
                }
            }
        }
        held
    }

    fn insert(&self, index: usize, chip: Chip) {
        let mut chips = self.chips.borrow_mut();
        let index = index.min(chips.len());
        chips.insert(index, chip);
        drop(chips);
        self.render();
    }

    fn remove(&self, index: usize) {
        let mut chips = self.chips.borrow_mut();
        if index < chips.len() {
            chips.remove(index);
        }
        drop(chips);
        self.render();
    }

    fn toggle(&self, index: usize) {
        if let Some(Chip::Event(event)) = self.chips.borrow_mut().get_mut(index) {
            event.down = !event.down;
        }
        self.render();
    }

    fn set_delay(&self, index: usize, ms: u32) {
        if let Some(slot @ Chip::Delay(_)) = self.chips.borrow_mut().get_mut(index) {
            *slot = Chip::Delay(ms);
        }
    }

    fn record(&self, kind: NodeKind, down: bool) {
        if self.is_held(kind) == down {
            return;
        }
        {
            let mut chips = self.chips.borrow_mut();
            if !chips.is_empty() {
                let gap = self.next_gap();
                chips.push(Chip::Delay(gap));
            }
            chips.push(Chip::Event(MacroEvent { kind, down }));
        }
        *self.last_event.borrow_mut() = Some(Instant::now());
        self.render();
    }

    fn set_recording(&self, on: bool) {
        self.recording.set(on);
        *self.last_event.borrow_mut() = None;
        self.record_button.set_label(if on { "结束录制" } else { "开始录制" });
        self.record_button.remove_css_class("destructive-action");
        if on {
            self.record_button.add_css_class("destructive-action");
        }
        self.flow.set_sensitive(!on);
        for widget in &self.paused {
            widget.set_sensitive(!on);
        }
        self.status.set_text(if on {
            "录制中：按键顺序与抬起自动记录，按 Esc 结束。"
        } else {
            "鼠标移到节点上可插入或删除。"
        });
    }

    fn summary(&self) -> String {
        let chips = self.chips.borrow();
        let events = chips
            .iter()
            .filter(|chip| matches!(chip, Chip::Event(_)))
            .count();
        let total: u32 = chips
            .iter()
            .map(|chip| match chip {
                Chip::Delay(ms) => *ms,
                Chip::Event(_) => 0,
            })
            .sum();
        if events == 0 {
            "还没有节点，录制或手动插入。".to_string()
        } else {
            format!("节点 {events} · 一轮约 {total} ms")
        }
    }

    fn render(&self) {
        while let Some(child) = self.flow.first_child() {
            self.flow.remove(&child);
        }
        if let Some(editor) = self.weak().upgrade() {
            let count = editor.chips.borrow().len();
            if count == 0 {
                let hint = gtk::Label::new(Some("录制或点 + 插入节点"));
                hint.add_css_class("dim-label");
                hint.add_css_class("macro-hint");
                self.flow.append(&hint);
                self.flow.append(&insert_button(&editor, 0));
            }
            for index in 0..count {
                if index == 0 {
                    self.flow.append(&insert_button(&editor, 0));
                }
                let chip = editor.chips.borrow()[index];
                self.flow.append(&node_widget(&editor, index, chip));
                self.flow.append(&insert_button(&editor, index + 1));
            }
        }
        self.status.set_text(&self.summary());
        self.flow.set_sensitive(!self.recording.get());
    }

    fn choose(self: &Rc<Self>, index: usize, which: u8) {
        match which {
            2 => {
                let gap = self.gap_ms();
                self.insert(index, Chip::Delay(gap));
            }
            mouse => self.pick_key(index, mouse == 1),
        }
    }

    fn pick_key(self: &Rc<Self>, index: usize, mouse: bool) {
        let choices = self.model.macro_choices(mouse);
        let labels: Vec<String> = choices.iter().map(|(label, _)| label.clone()).collect();
        let picker = adw::Dialog::builder()
            .title(if mouse {
                "插入鼠标按键"
            } else {
                "插入键盘按键"
            })
            .content_width(560)
            .build();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
        content.set_margin_top(12);
        content.set_margin_bottom(12);
        content.set_margin_start(12);
        content.set_margin_end(12);
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("搜索按键…"));
        content.append(&search);

        let query = Rc::new(RefCell::new(String::new()));
        let weak = Rc::downgrade(self);
        let close = picker.clone();
        let flow = button_flow(
            &labels,
            &query,
            None,
            Rc::new(move |pick: usize| {
                if let Some(editor) = weak.upgrade() {
                    if let Some((_, kind)) = choices.get(pick) {
                        editor.insert(index, Chip::Event(MacroEvent { kind: *kind, down: true }));
                    }
                }
                close.close();
            }),
        );
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .propagate_natural_height(true)
            .max_content_height(300)
            .child(&flow)
            .build();
        content.append(&scroll);
        picker.set_child(Some(&content));
        connect_search(&search, &query, &[flow]);
        picker.present(Some(&self.ui.window));
    }

    fn save(&self) {
        let name = self.name.text().trim().to_string();
        let chars = name.chars().count();
        if !(MIN_NAME_CHARS..=MAX_NAME_CHARS).contains(&chars) {
            self.ui
                .toast(&format!("名称需要 {MIN_NAME_CHARS}–{MAX_NAME_CHARS} 个字符"));
            return;
        }
        if !self.chips.borrow().iter().any(|chip| matches!(chip, Chip::Event(_))) {
            self.ui.toast("还没有录制按键");
            return;
        }
        let macros = self.ui.state.borrow().macros.clone();
        if macros
            .iter()
            .enumerate()
            .any(|(index, item)| index != self.slot && item.name == name)
        {
            self.ui.toast("名称重复，请修改名称");
            return;
        }
        let mut updated = macros;
        let macro_def = MacroDef {
            name,
            chips: self.chips.borrow().clone(),
        };
        if self.slot < updated.len() {
            updated[self.slot] = macro_def;
        } else {
            updated.push(macro_def);
        }
        let kind = self.selected_type();
        let layers = retyped_layers(&self.ui, self.slot, kind);
        {
            let mut state = self.ui.state.borrow_mut();
            if state.macro_types.len() <= self.slot {
                state.macro_types.resize(self.slot + 1, MacroType::HoldLoop);
            }
            state.macro_types[self.slot] = kind;
        }
        if !self.ui.state.borrow().connected {
            self.ui.toast("设备未连接，指令已存在本机");
        }
        self.ui.commit_commands(updated, layers);
        self.dialog.close();
    }
}

/// The run mode lives in the key action, so keys already holding this slot are
/// rewritten with the newly chosen type.
fn retyped_layers(ui: &Rc<AppUi>, slot: usize, kind: MacroType) -> Vec<(u8, KeyLayer)> {
    let state = ui.state.borrow();
    let mut out = Vec::new();
    for layer in 0..3u8 {
        let Some(keys) = state.keys[layer as usize].clone() else {
            continue;
        };
        let mut changed = keys.clone();
        let mut dirty = false;
        for value in &mut changed.keys {
            if let Some((found, _)) = macro_ref(*value) {
                if found == slot {
                    *value = macro_action(slot, kind);
                    dirty = true;
                }
            }
        }
        if dirty {
            out.push((layer, changed));
        }
    }
    out
}

/// One node of the chain: a chip, plus a delete handle in its top-right corner.
fn node_widget(editor: &Rc<Editor>, index: usize, chip: Chip) -> gtk::Overlay {
    let overlay = gtk::Overlay::new();
    overlay.add_css_class("macro-node");

    let body: gtk::Widget = match chip {
        Chip::Event(event) => {
            let button = gtk::Button::with_label(&event_label(&editor.model, event));
            button.add_css_class("macro-chip");
            button.set_tooltip_text(Some("点击切换按下 / 抬起"));
            let weak = Rc::downgrade(editor);
            button.connect_clicked(move |_| {
                if let Some(editor) = weak.upgrade() {
                    editor.toggle(index);
                }
            });
            button.upcast()
        }
        Chip::Delay(ms) => {
            let card = gtk::Box::new(gtk::Orientation::Vertical, 2);
            card.add_css_class("macro-delay");
            let caption = gtk::Label::new(Some("延时"));
            caption.add_css_class("dim-label");
            caption.add_css_class("caption");
            let entry = gtk::Entry::new();
            entry.set_text(&ms.to_string());
            entry.set_width_chars(4);
            entry.set_max_width_chars(6);
            entry.set_placeholder_text(Some("ms"));
            gtk::prelude::EditableExt::set_alignment(&entry, 0.5);
            let weak = Rc::downgrade(editor);
            entry.connect_changed(move |entry| {
                let Some(editor) = weak.upgrade() else {
                    return;
                };
                let value = entry
                    .text()
                    .trim()
                    .parse::<u32>()
                    .unwrap_or(0)
                    .min(MAX_DELAY_MS);
                editor.set_delay(index, value);
            });
            card.append(&caption);
            card.append(&entry);
            card.upcast()
        }
    };
    overlay.set_child(Some(&body));

    let remove = gtk::Button::from_icon_name("window-close-symbolic");
    remove.add_css_class("flat");
    remove.add_css_class("macro-x");
    remove.set_halign(gtk::Align::End);
    remove.set_valign(gtk::Align::Start);
    remove.set_tooltip_text(Some("删除节点"));
    let weak = Rc::downgrade(editor);
    remove.connect_clicked(move |_| {
        if let Some(editor) = weak.upgrade() {
            editor.remove(index);
        }
    });
    overlay.add_overlay(&remove);
    overlay
}

fn insert_button(editor: &Rc<Editor>, index: usize) -> gtk::MenuButton {
    let button = gtk::MenuButton::new();
    button.set_icon_name("list-add-symbolic");
    button.add_css_class("flat");
    button.add_css_class("macro-insert");
    button.set_tooltip_text(Some("在此插入节点"));

    let popover = gtk::Popover::new();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 2);
    content.set_margin_top(6);
    content.set_margin_bottom(6);
    content.set_margin_start(6);
    content.set_margin_end(6);
    popover.set_child(Some(&content));

    for (label, which) in [("插入键盘按键", 0u8), ("插入鼠标按键", 1), ("插入延时", 2)] {
        let item = gtk::Button::with_label(label);
        item.add_css_class("flat");
        item.set_halign(gtk::Align::Fill);
        let weak = Rc::downgrade(editor);
        let popover = popover.clone();
        item.connect_clicked(move |_| {
            popover.popdown();
            if let Some(editor) = weak.upgrade() {
                editor.choose(index, which);
            }
        });
        content.append(&item);
    }
    button.set_popover(Some(&popover));
    button
}

pub fn open(ui: &Rc<AppUi>, slot: Option<usize>) {
    let stored = slot.and_then(|index| ui.state.borrow().macros.get(index).cloned());
    let editing = slot.is_some();
    let slot = slot.unwrap_or_else(|| ui.state.borrow().macros.len());
    let (initial_chips, initial_name) = match stored {
        Some(def) => (def.chips, def.name),
        None => (Vec::new(), String::new()),
    };
    let kind = if editing {
        ui.macro_type(slot)
    } else {
        MacroType::HoldLoop
    };

    let dialog = adw::Dialog::builder()
        .title("快捷指令")
        .content_width(640)
        .build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);

    let naming = section(&content, "指令");
    let name_row = adw::ActionRow::builder().title("名称").build();
    let name = gtk::Entry::new();
    name.set_width_chars(18);
    name.set_max_length(MAX_NAME_CHARS as i32);
    name.set_valign(gtk::Align::Center);
    name.set_placeholder_text(Some("2–15 个字符"));
    let counter = gtk::Label::new(Some("0/15"));
    counter.add_css_class("dim-label");
    counter.add_css_class("caption");
    counter.set_valign(gtk::Align::Center);
    name_row.add_suffix(&counter);
    name_row.add_suffix(&name);
    naming.add(&name_row);

    let kind_row = adw::ComboRow::new();
    kind_row.set_title("指令类型");
    kind_row.set_model(Some(&gtk::StringList::new(&MacroType::ALL.map(MacroType::label))));
    kind_row.set_subtitle(kind.hint());
    naming.add(&kind_row);

    let standard = adw::SwitchRow::builder()
        .title("标准延迟")
        .subtitle("统一使用同一个间隔")
        .build();
    standard.set_active(true);
    naming.add(&standard);

    let gap = adw::SpinRow::with_range(1.0, MAX_DELAY_MS as f64, 10.0);
    gap.set_title("间隔（ms）");
    gap.set_value(DEFAULT_DELAY_MS as f64);
    naming.add(&gap);

    let flow = section(&content, "节点流程");
    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    toolbar.set_margin_top(14);
    toolbar.set_margin_bottom(6);
    toolbar.set_margin_start(6);
    toolbar.set_margin_end(6);
    let record_button = gtk::Button::with_label("开始录制");
    toolbar.append(&record_button);
    let status = gtk::Label::new(None);
    status.add_css_class("dim-label");
    status.add_css_class("caption");
    status.set_xalign(0.0);
    status.set_hexpand(true);
    toolbar.append(&status);
    flow.add(&toolbar);

    let chain_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    chain_box.add_css_class("macro-flow");
    chain_box.set_valign(gtk::Align::Center);
    let chain_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(168)
        .hexpand(true)
        .child(&chain_box)
        .build();
    chain_scroll.add_css_class("card");
    flow.add(&chain_scroll);

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("取消");
    let save = gtk::Button::with_label("保存");
    save.add_css_class("suggested-action");
    footer.append(&cancel);
    footer.append(&save);
    content.append(&footer);
    dialog.set_child(Some(&content));

    let paused = vec![
        name.clone().upcast::<gtk::Widget>(),
        kind_row.clone().upcast(),
        standard.clone().upcast(),
        gap.clone().upcast(),
    ];
    let editor = Rc::new(Editor {
        ui: ui.clone(),
        model: ui.model.clone(),
        slot,
        dialog: dialog.clone(),
        chips: RefCell::new(initial_chips),
        recording: Cell::new(false),
        last_event: RefCell::new(None),
        flow: chain_box,
        status,
        name: name.clone(),
        kind: kind_row.clone(),
        standard: standard.clone(),
        gap: gap.clone(),
        record_button: record_button.clone(),
        paused,
        here: RefCell::new(Weak::new()),
    });
    editor.here.replace(Rc::downgrade(&editor));

    let counter_for_name = counter.clone();
    name.connect_changed(move |entry| {
        counter_for_name.set_text(&format!(
            "{}/{}",
            entry.text().trim().chars().count(),
            MAX_NAME_CHARS
        ));
    });
    name.set_text(&initial_name);

    let kind_for_hint = kind_row.clone();
    kind_row.connect_selected_notify(move |row| {
        let hint = MacroType::ALL
            .get(row.selected() as usize)
            .map_or("", |known| known.hint());
        kind_for_hint.set_subtitle(hint);
    });
    kind_row.set_selected(
        MacroType::ALL
            .iter()
            .position(|known| *known == kind)
            .unwrap_or(0) as u32,
    );

    let weak = editor.weak();
    record_button.connect_clicked(move |_| {
        if let Some(editor) = weak.upgrade() {
            let on = !editor.recording.get();
            editor.set_recording(on);
        }
    });
    let weak = editor.weak();
    cancel.connect_clicked(move |_| {
        if let Some(editor) = weak.upgrade() {
            editor.dialog.close();
        }
    });
    let weak = editor.weak();
    save.connect_clicked(move |_| {
        if let Some(editor) = weak.upgrade() {
            editor.save();
        }
    });
    dialog.connect_closed(|_| OPEN.with(|cell| *cell.borrow_mut() = None));

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = editor.weak();
    keys.connect_key_pressed(move |_, keyval, _keycode, _state| {
        let Some(editor) = weak.upgrade() else {
            return glib::Propagation::Proceed;
        };
        if !editor.recording.get() {
            return glib::Propagation::Proceed;
        }
        if keyval.to_unicode() == Some('\u{1b}') {
            editor.set_recording(false);
        } else if let Some(usage) = usage_from_keyval(keyval) {
            editor.record(NodeKind::Key(usage), true);
        }
        glib::Propagation::Stop
    });
    let weak = editor.weak();
    keys.connect_key_released(move |_, keyval, _keycode, _state| {
        let Some(editor) = weak.upgrade() else {
            return;
        };
        if !editor.recording.get() {
            return;
        }
        if let Some(usage) = usage_from_keyval(keyval) {
            editor.record(NodeKind::Key(usage), false);
        }
    });
    content.add_controller(keys);

    editor.render();
    OPEN.with(|cell| *cell.borrow_mut() = Some(editor));
    dialog.present(Some(&ui.window));
}
