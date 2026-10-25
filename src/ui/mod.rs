pub mod keyboard;

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::device::{DeviceService, Discovery, Event, Presence, Request, Transport};
use crate::model::Model;
use crate::protocol::{Battery, KeyLayer, LightState, Performance};

use self::keyboard::KeyboardView;

thread_local! {
    static CURRENT_UI: RefCell<Option<Rc<AppUi>>> = const { RefCell::new(None) };
}

const UDEV_RULE: &str =
    "SUBSYSTEM==\"hidraw\", ATTRS{idVendor}==\"05ac\", ATTRS{idProduct}==\"024f\", TAG+=\"uaccess\"";

#[derive(Default)]
struct UiState {
    connected: bool,
    searching: bool,
    transport: Option<Transport>,
    discovery: Option<Discovery>,
    battery: Option<Battery>,
    performance: Option<Performance>,
    light: Option<LightState>,
    keys: [Option<KeyLayer>; 3],
    mac_layout: bool,
    layer: u8,
    selected: Option<usize>,
    loading: bool,
    busy_text: String,
    error: Option<String>,
}

impl UiState {
    fn new() -> Self {
        Self {
            searching: true,
            ..Default::default()
        }
    }
}

pub struct AppUi {
    model: Model,
    service: DeviceService,
    window: adw::ApplicationWindow,
    toast: adw::ToastOverlay,
    stack: adw::ViewStack,
    banner: adw::Banner,
    busy_box: gtk::Box,
    spinner: gtk::Spinner,
    busy_label: gtk::Label,
    state: Rc<RefCell<UiState>>,
    device_page: DevicePage,
    keys_page: KeysPage,
    lighting_page: LightingPage,
    performance_page: PerformancePage,
    settings_page: SettingsPage,
    pending_lighting: RefCell<Option<(Performance, LightState)>>,
    lighting_scheduled: Cell<bool>,
}

pub fn build_window(
    app: &adw::Application,
    model: Model,
    simulate: bool,
) -> adw::ApplicationWindow {
    load_css();
    if let Some(display) = gdk::Display::default() {
        let theme = gtk::IconTheme::for_display(&display);
        theme.add_resource_path("/top/ddmt/mchosehub/icons");
    }
    let service = DeviceService::start(model.clone(), simulate);
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("MCHOSE HUB")
        .default_width(1120)
        .default_height(780)
        .build();
    window.set_size_request(920, 620);

    let ui = Rc::new_cyclic(|weak| {
        let device_page = DevicePage::new(weak, &model);
        let keys_page = KeysPage::new(weak, &model);
        let lighting_page = LightingPage::new(weak, &model);
        let performance_page = PerformancePage::new(weak);
        let settings_page = SettingsPage::new(weak, &model);

        let stack = adw::ViewStack::new();
        stack.set_vexpand(true);
        stack.set_hexpand(true);
        stack.add_titled_with_icon(
            &device_page.root,
            Some("device"),
            "设备",
            "mchose-device-symbolic",
        );
        stack.add_titled_with_icon(
            &keys_page.root,
            Some("keys"),
            "按键",
            "mchose-keys-symbolic",
        );
        stack.add_titled_with_icon(
            &lighting_page.root,
            Some("lighting"),
            "灯效",
            "mchose-light-symbolic",
        );
        stack.add_titled_with_icon(
            &performance_page.root,
            Some("performance"),
            "性能",
            "mchose-performance-symbolic",
        );
        stack.add_titled_with_icon(
            &settings_page.root,
            Some("settings"),
            "设置",
            "mchose-settings-symbolic",
        );

        let header = adw::HeaderBar::new();
        let switcher = adw::ViewSwitcher::builder()
            .stack(&stack)
            .policy(adw::ViewSwitcherPolicy::Wide)
            .build();
        header.set_title_widget(Some(&switcher));

        let spinner = gtk::Spinner::new();
        let busy_label = gtk::Label::new(None);
        busy_label.add_css_class("dim-label");
        busy_label.add_css_class("caption");
        let busy_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        busy_box.append(&spinner);
        busy_box.append(&busy_label);
        busy_box.set_visible(false);
        header.pack_start(&busy_box);

        let refresh = gtk::Button::from_icon_name("mchose-refresh-symbolic");
        refresh.set_tooltip_text(Some("刷新设备数据"));
        {
            let weak = weak.clone();
            refresh.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    ui.send(Request::Refresh);
                    let on_keys = ui.stack.visible_child_name().is_some_and(|n| n == "keys");
                    if on_keys {
                        ui.send(Request::LoadKeys);
                    }
                }
            });
        }
        header.pack_end(&refresh);

        let menu = gio::Menu::new();
        menu.append(Some("关于 MCHOSE HUB"), Some("app.about"));
        menu.append(Some("退出"), Some("app.quit"));
        let menu_button = gtk::MenuButton::builder()
            .icon_name("mchose-menu-symbolic")
            .menu_model(&menu)
            .build();
        header.pack_end(&menu_button);

        let banner = adw::Banner::new("");
        banner.set_button_label(Some("重试"));
        banner.set_revealed(false);
        {
            let weak = weak.clone();
            banner.connect_button_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    ui.state.borrow_mut().error = None;
                    ui.update_header();
                    ui.send(Request::Refresh);
                }
            });
        }

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.set_vexpand(true);
        body.append(&banner);
        body.append(&stack);
        toolbar.set_content(Some(&body));

        let toast = adw::ToastOverlay::new();
        toast.set_child(Some(&toolbar));
        window.set_content(Some(&toast));

        {
            let weak = weak.clone();
            stack.connect_notify_local(Some("visible-child-name"), move |stack, _| {
                if let Some(ui) = weak.upgrade() {
                    if stack
                        .visible_child_name()
                        .is_some_and(|n| n == "keys" || n == "performance")
                    {
                        ui.ensure_keys();
                    }
                }
            });
        }

        AppUi {
            model,
            service,
            window: window.clone(),
            toast,
            stack,
            banner,
            busy_box,
            spinner,
            busy_label,
            state: Rc::new(RefCell::new(UiState::new())),
            device_page,
            keys_page,
            lighting_page,
            performance_page,
            settings_page,
            pending_lighting: RefCell::new(None),
            lighting_scheduled: Cell::new(false),
        }
    });

    ui.start_events();
    ui.refresh_all();
    ui.window.connect_close_request(|_| {
        CURRENT_UI.with(|slot| *slot.borrow_mut() = None);
        glib::Propagation::Proceed
    });
    CURRENT_UI.with(|slot| *slot.borrow_mut() = Some(ui.clone()));
    ui.window.present();
    ui.window.clone()
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(include_str!("style.css"));
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

impl AppUi {
    fn send(&self, request: Request) {
        self.service.send(request);
    }

    fn toast(&self, message: &str) {
        self.toast.add_toast(adw::Toast::new(message));
    }

    fn start_events(self: &Rc<Self>) {
        let events = self.service.events();
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(80), move || {
            while let Ok(event) = events.try_recv() {
                if let Some(ui) = weak.upgrade() {
                    ui.handle_event(event);
                } else {
                    return glib::ControlFlow::Break;
                }
            }
            glib::ControlFlow::Continue
        });
    }

    fn handle_event(self: &Rc<Self>, event: Event) {
        match event {
            Event::Connected { transport } => {
                let mut state = self.state.borrow_mut();
                state.connected = true;
                state.searching = false;
                state.transport = Some(transport);
                state.keys = [None, None, None];
                state.selected = None;
                state.error = None;
                drop(state);
                if self.stack.visible_child_name().is_some_and(|n| n == "keys") {
                    self.ensure_keys();
                }
            }
            Event::Detected(discovery) => {
                self.state.borrow_mut().discovery = Some(discovery);
            }
            Event::Disconnected => {
                let mut state = self.state.borrow_mut();
                state.connected = false;
                state.searching = false;
                state.battery = None;
                state.performance = None;
                state.light = None;
                state.keys = [None, None, None];
                state.selected = None;
                drop(state);
            }
            Event::Battery(battery) => {
                self.state.borrow_mut().battery = Some(battery);
            }
            Event::Performance(performance) => {
                self.state.borrow_mut().performance = Some(performance);
            }
            Event::Light(light) => {
                self.state.borrow_mut().light = Some(light);
            }
            Event::Keys(layer, keys) => {
                let inferred = if layer == 0 {
                    self.model.mac_layout_state(&keys.keys)
                } else {
                    None
                };
                if let Some(slot) = self.state.borrow_mut().keys.get_mut(layer as usize) {
                    *slot = Some(keys);
                }
                if let Some(mac_layout) = inferred {
                    self.state.borrow_mut().mac_layout = mac_layout;
                }
            }
            Event::Healthy => {
                self.state.borrow_mut().error = None;
            }
            Event::Saved(message) => {
                self.state.borrow_mut().error = None;
                self.toast(message);
            }
            Event::Error(message) => {
                let mut text = message;
                if text.contains("无响应") || text.contains("超时") {
                    text.push_str("（键盘可能已休眠，请按任意键唤醒后重试）");
                }
                let mut state = self.state.borrow_mut();
                state.error = Some(text);
                state.searching = false;
                state.loading = false;
                state.busy_text.clear();
                drop(state);
            }
            Event::Busy(loading, text) => {
                let mut state = self.state.borrow_mut();
                state.loading = loading;
                state.busy_text = text;
                drop(state);
            }
        }
        self.refresh_all();
    }

    fn refresh_all(&self) {
        self.device_page.update(self);
        self.keys_page.update(self);
        self.lighting_page.update(self);
        self.performance_page.update(self);
        self.settings_page.update(self);
        self.update_header();
    }

    fn update_header(&self) {
        let (loading, busy, error, connected, searching) = {
            let state = self.state.borrow();
            (
                state.loading,
                state.busy_text.clone(),
                state.error.clone(),
                state.connected,
                state.searching,
            )
        };
        self.busy_box.set_visible(loading);
        if loading {
            self.spinner.start();
            self.busy_label.set_text(&busy);
        } else {
            self.spinner.stop();
            self.busy_label.set_text("");
        }
        if let Some(error) = error {
            self.banner.set_title(&error);
            self.banner.set_revealed(true);
        } else if !connected && !searching {
            self.banner
                .set_title("设备已断开，请检查 USB 线或 2.4G 接收器");
            self.banner.set_revealed(true);
        } else {
            self.banner.set_revealed(false);
        }
    }

    fn ensure_keys(&self) {
        let state = self.state.borrow();
        if state.connected && state.keys[0].is_none() {
            drop(state);
            self.send(Request::LoadKeys);
        }
    }

    fn set_layer(&self, layer: u8) {
        if layer > 2 {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            if state.layer == layer {
                return;
            }
            state.layer = layer;
            state.selected = None;
        }
        self.keys_page.clear_selection();
        self.ensure_keys();
        self.refresh_all();
    }

    fn select_key(&self, matrix: usize) {
        {
            let mut state = self.state.borrow_mut();
            state.selected = Some(matrix);
        }
        self.keys_page.keyboard.set_selected(Some(matrix));
        self.keys_page.select_visual();
        self.keys_page.update(self);
    }

    fn apply_key_action(self: &Rc<Self>, matrix: usize, value: [u8; 4]) {
        let layer = self.state.borrow().layer;
        let Some(mut keys) = self.state.borrow().keys[layer as usize].clone() else {
            self.toast("按键数据尚未读取");
            return;
        };
        if matrix >= keys.keys.len() {
            return;
        }
        keys.keys[matrix] = value;
        self.send(Request::SetKeys(layer, keys));
    }

    fn reset_selected_key(self: &Rc<Self>) {
        let (layer, selected) = {
            let state = self.state.borrow();
            (state.layer, state.selected)
        };
        let Some(matrix) = selected else {
            return;
        };
        let Some(mut keys) = self.state.borrow().keys[layer as usize].clone() else {
            return;
        };
        let value = self
            .model
            .key_for_matrix(matrix)
            .map(|key| match layer {
                0 => key.default,
                1 => key.fn_layer,
                _ => key.fn2,
            })
            .unwrap_or([0, 0, 0, 0]);
        keys.keys[matrix] = value;
        self.send(Request::SetKeys(layer, keys));
    }

    fn set_mac_layout(self: &Rc<Self>, active: bool) {
        let Some(mut layer) = self.state.borrow().keys[0].clone() else {
            self.send(Request::LoadKeys);
            self.toast("正在读取按键…");
            return;
        };
        if layer.keys.len() <= 17 {
            return;
        }
        layer.keys.swap(11, 17);
        log::info!(
            "mac layout active={active} win11={:?} alt17={:?}",
            layer.keys[11],
            layer.keys[17]
        );
        self.state.borrow_mut().mac_layout = active;
        self.send(Request::SetKeys(0, layer));
    }

    fn open_action_picker(self: &Rc<Self>, matrix: usize) {
        let current = self.keys_page.keyboard.binding(matrix).unwrap_or([0; 4]);
        open_action_picker(self, matrix, current);
    }

    fn schedule_lighting(self: &Rc<Self>, performance: Performance, light: LightState) {
        *self.pending_lighting.borrow_mut() = Some((performance, light));
        if self.lighting_scheduled.get() {
            return;
        }
        self.lighting_scheduled.set(true);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(350), move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            ui.lighting_scheduled.set(false);
            let pending = ui.pending_lighting.borrow_mut().take();
            if let Some((performance, light)) = pending {
                ui.send(Request::SetLighting(performance, light));
            }
        });
    }

    fn lighting_select_mode(self: &Rc<Self>, mode: u8) {
        let (performance, light) = {
            let state = self.state.borrow();
            (state.performance.clone(), state.light.clone())
        };
        let (Some(performance), Some(light)) = (performance, light) else {
            return;
        };
        self.lighting_page.mode.set(mode);
        self.lighting_page
            .apply_state(self, mode, &performance, &light);
        if let Some((performance, light)) = self.lighting_page.desired(self) {
            self.schedule_lighting(performance, light);
        }
    }
}

fn page_container(title: &str, subtitle: &str) -> (gtk::ScrolledWindow, gtk::Box) {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
    content.set_margin_top(24);
    content.set_margin_bottom(36);
    content.set_margin_start(24);
    content.set_margin_end(24);

    let title_label = gtk::Label::new(Some(title));
    title_label.add_css_class("title-1");
    title_label.set_xalign(0.0);
    content.append(&title_label);

    if !subtitle.is_empty() {
        let sub = gtk::Label::new(Some(subtitle));
        sub.add_css_class("dim-label");
        sub.set_wrap(true);
        sub.set_xalign(0.0);
        content.append(&sub);
    }

    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_vexpand(true);
    scroll.set_child(Some(&content));
    (scroll, content)
}

fn section(content: &gtk::Box, title: &str) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder().title(title).build();
    content.append(&group);
    group
}

fn pill_button(label: &str) -> gtk::Button {
    let button = gtk::Button::with_label(label);
    button.set_halign(gtk::Align::Start);
    button.add_css_class("pill");
    button.set_margin_top(8);
    button.set_margin_bottom(8);
    button.set_margin_start(6);
    button.set_margin_end(6);
    button
}

fn value_row(group: &adw::PreferencesGroup, title: &str) -> gtk::Label {
    let row = adw::ActionRow::builder().title(title).build();
    let value = gtk::Label::new(Some("—"));
    value.add_css_class("dim-label");
    row.add_suffix(&value);
    group.add(&row);
    value
}

fn confirm(
    ui: &Rc<AppUi>,
    heading: &str,
    body: &str,
    action_label: &str,
    action: impl Fn(&Rc<AppUi>) + 'static,
) {
    let dialog = adw::AlertDialog::new(Some(heading), Some(body));
    dialog.add_response("cancel", "取消");
    dialog.add_response("ok", action_label);
    dialog.set_response_appearance("ok", adw::ResponseAppearance::Destructive);
    dialog.set_close_response("cancel");
    let weak = Rc::downgrade(ui);
    dialog.connect_response(None, move |_, response| {
        if response == "ok" {
            if let Some(ui) = weak.upgrade() {
                action(&ui);
            }
        }
    });
    dialog.present(Some(&ui.window));
}

fn presence_text(presence: &Presence) -> String {
    match presence {
        Presence::Available(info) => format!("已检测到 · {}", info.hidraw),
        Presence::Denied(_) => "已检测到，但无访问权限".to_string(),
        Presence::Absent => "未检测到".to_string(),
    }
}

struct DevicePage {
    root: gtk::ScrolledWindow,
    stack: gtk::Stack,
    local_spinner: gtk::Spinner,
    empty_title: gtk::Label,
    empty_body: gtk::Label,
    model_value: gtk::Label,
    connection_value: gtk::Label,
    wired_value: gtk::Label,
    wireless_value: gtk::Label,
    battery_bar: gtk::ProgressBar,
    rate_value: gtk::Label,
    effect_value: gtk::Label,
    sleep_value: gtk::Label,
    win_value: gtk::Label,
    mac_value: gtk::Label,
}

impl DevicePage {
    fn new(weak: &Weak<AppUi>, model: &Model) -> Self {
        let (root, page) = page_container("设备", "查看键盘状态与当前配置。");
        let details = gtk::Box::new(gtk::Orientation::Vertical, 18);
        details.set_vexpand(true);

        let info = section(&details, "设备信息");
        let model_value = value_row(&info, "型号");
        let connection_value = value_row(&info, "连接");
        let wired_value = value_row(&info, "USB 有线");
        let wireless_value = value_row(&info, "2.4G 接收器");
        let battery_row = adw::ActionRow::builder().title("电量").build();
        let battery_bar = gtk::ProgressBar::new();
        battery_bar.set_show_text(true);
        battery_bar.set_size_request(180, -1);
        battery_row.add_suffix(&battery_bar);
        info.add(&battery_row);

        let current = section(&details, "当前配置");
        let rate_value = value_row(&current, "回报率");
        let effect_value = value_row(&current, "灯效");
        let sleep_value = value_row(&current, "休眠");
        let win_value = value_row(&current, "Win 键锁");
        let mac_value = value_row(&current, "Mac 布局");

        let status = section(&details, "设备状态");
        let note = adw::ActionRow::builder()
            .title("提示")
            .subtitle("键盘休眠时厂商通道不会响应，按任意键唤醒后点击刷新。")
            .build();
        status.add(&note);
        let rescan = pill_button("刷新设备数据");
        {
            let weak = weak.clone();
            rescan.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    ui.send(Request::Refresh);
                }
            });
        }
        status.add(&rescan);

        let empty = gtk::Box::new(gtk::Orientation::Vertical, 12);
        empty.set_valign(gtk::Align::Center);
        empty.set_halign(gtk::Align::Center);
        empty.set_vexpand(true);
        empty.set_margin_top(48);
        empty.set_margin_bottom(48);
        let icon = gtk::Image::from_icon_name("mchose-device-symbolic");
        icon.set_pixel_size(72);
        icon.add_css_class("dim-label");
        empty.append(&icon);
        let empty_title = gtk::Label::new(Some("正在查找设备…"));
        empty_title.add_css_class("title-1");
        empty.append(&empty_title);
        let empty_body = gtk::Label::new(Some(
            "请用 USB 线连接 K99 V2，或插入 2.4G 接收器，并按任意键唤醒键盘。",
        ));
        empty_body.add_css_class("dim-label");
        empty_body.set_wrap(true);
        empty_body.set_justify(gtk::Justification::Center);
        empty_body.set_max_width_chars(46);
        empty.append(&empty_body);
        let local_spinner = gtk::Spinner::new();
        local_spinner.set_size_request(24, 24);
        empty.append(&local_spinner);

        let stack = gtk::Stack::new();
        stack.set_vexpand(true);
        stack.set_hexpand(true);
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&details, Some("content"));
        stack.set_visible_child_name("empty");
        page.append(&stack);

        let _ = model;
        Self {
            root,
            stack,
            local_spinner,
            empty_title,
            empty_body,
            model_value,
            connection_value,
            wired_value,
            wireless_value,
            battery_bar,
            rate_value,
            effect_value,
            sleep_value,
            win_value,
            mac_value,
        }
    }

    fn update(&self, ui: &AppUi) {
        let state = ui.state.borrow();
        if state.connected {
            self.stack.set_visible_child_name("content");
            self.local_spinner.stop();
        } else {
            self.stack.set_visible_child_name("empty");
            if state.searching {
                self.local_spinner.start();
                self.empty_title.set_text("正在查找设备…");
            } else {
                self.local_spinner.stop();
                self.empty_title.set_text("未检测到设备");
            }
            if let Some(error) = &state.error {
                self.empty_body.set_text(error);
            } else {
                self.empty_body
                    .set_text("请用 USB 线连接 K99 V2，或插入 2.4G 接收器，并按任意键唤醒键盘。");
            }
        }

        self.model_value.set_text(&ui.model.name);
        let connection = state
            .transport
            .map(Transport::label)
            .unwrap_or("—")
            .to_string();
        self.connection_value.set_text(&connection);
        match &state.discovery {
            Some(discovery) => {
                self.wired_value.set_text(&presence_text(&discovery.wired));
                self.wireless_value
                    .set_text(&presence_text(&discovery.wireless));
            }
            None => {
                self.wired_value.set_text("—");
                self.wireless_value.set_text("—");
            }
        }

        let battery = state.battery;
        if let Some(battery) = battery {
            self.battery_bar.set_fraction(battery.level as f64 / 100.0);
            self.battery_bar
                .set_text(Some(&format!("{}%", battery.level)));
        } else {
            self.battery_bar.set_fraction(0.0);
            self.battery_bar.set_text(None);
        }

        if let Some(performance) = &state.performance {
            self.rate_value.set_text(
                &performance
                    .rate_hz()
                    .map_or_else(|| "—".to_string(), |rate| format!("{rate} Hz")),
            );
            let mode = performance.light_mode();
            self.effect_value.set_text(
                &ui.model
                    .effect(mode)
                    .map(|effect| effect.name.clone())
                    .unwrap_or_else(|| format!("模式 {mode}")),
            );
            let minutes = performance.sleep_minutes();
            self.sleep_value.set_text(&if minutes == 0 {
                "从不".to_string()
            } else {
                format!("{minutes} 分钟")
            });
            self.win_value.set_text(if performance.win_lock() {
                "开启"
            } else {
                "关闭"
            });
            let mac_layout = ui.state.borrow().mac_layout;
            self.mac_value
                .set_text(if mac_layout { "开启" } else { "关闭" });
        } else {
            for label in [
                &self.rate_value,
                &self.effect_value,
                &self.sleep_value,
                &self.win_value,
                &self.mac_value,
            ] {
                label.set_text("—");
            }
        }
    }
}

struct KeysPage {
    root: gtk::ScrolledWindow,
    layer_combo: adw::ComboRow,
    reset_layer_btn: gtk::Button,
    keyboard: KeyboardView,
    key_row: adw::ActionRow,
    action_row: adw::ActionRow,
    change_btn: gtk::Button,
    reset_key_btn: gtk::Button,
    hint: gtk::Label,
    updating: Cell<bool>,
}

impl KeysPage {
    fn new(weak: &Weak<AppUi>, model: &Model) -> Self {
        let (root, content) = page_container(
            "按键",
            "点击键盘上的按键，然后为当前图层指定功能。修改会立即写入键盘。",
        );

        let layer_group = section(&content, "图层");
        let layer_combo = adw::ComboRow::new();
        layer_combo.set_title("当前图层");
        let layers = gtk::StringList::new(&["默认层", "Fn 层", "Fn2 层"]);
        layer_combo.set_model(Some(&layers));
        layer_combo.set_selected(0);
        layer_group.add(&layer_combo);

        let reset_layer_btn = pill_button("恢复本层默认");
        {
            let weak = weak.clone();
            reset_layer_btn.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    let layer = ui.state.borrow().layer;
                    confirm(
                        &ui,
                        "恢复本层默认？",
                        "当前图层的全部按键会恢复为出厂设置。",
                        "恢复",
                        move |ui| ui.send(Request::ResetKeys(layer)),
                    );
                }
            });
        }
        layer_group.add(&reset_layer_btn);

        let keyboard_group = section(&content, "键盘");
        let keyboard = KeyboardView::new(model, {
            let weak = weak.clone();
            move |matrix| {
                if let Some(ui) = weak.upgrade() {
                    ui.select_key(matrix);
                }
            }
        });
        keyboard.frame.set_margin_top(6);
        keyboard.frame.set_margin_bottom(6);
        keyboard.frame.set_margin_start(6);
        keyboard.frame.set_margin_end(6);
        keyboard_group.add(&keyboard.frame);

        let hint = gtk::Label::new(Some("点击按键进行编辑。"));
        hint.add_css_class("dim-label");
        hint.set_xalign(0.0);
        keyboard_group.add(&hint);

        let selected_group = section(&content, "当前按键");
        let key_row = adw::ActionRow::builder()
            .title("未选择按键")
            .subtitle("点击上方键盘选择")
            .build();
        selected_group.add(&key_row);
        let action_row = adw::ActionRow::builder()
            .title("当前功能")
            .subtitle("—")
            .build();
        selected_group.add(&action_row);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        actions.set_halign(gtk::Align::Start);
        actions.set_margin_top(8);
        actions.set_margin_bottom(8);
        actions.set_margin_start(6);
        actions.set_margin_end(6);
        let change_btn = gtk::Button::with_label("更改功能…");
        change_btn.add_css_class("suggested-action");
        let reset_key_btn = gtk::Button::with_label("恢复此键");
        actions.append(&change_btn);
        actions.append(&reset_key_btn);
        selected_group.add(&actions);
        change_btn.set_sensitive(false);
        reset_key_btn.set_sensitive(false);

        {
            let weak = weak.clone();
            change_btn.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    if let Some(matrix) = ui.state.borrow().selected {
                        ui.open_action_picker(matrix);
                    }
                }
            });
        }
        {
            let weak = weak.clone();
            reset_key_btn.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    ui.reset_selected_key();
                }
            });
        }
        {
            let weak = weak.clone();
            layer_combo.connect_selected_notify(move |combo| {
                if let Some(ui) = weak.upgrade() {
                    if ui.keys_page.updating.get() {
                        return;
                    }
                    ui.set_layer(combo.selected() as u8);
                }
            });
        }

        Self {
            root,
            layer_combo,
            reset_layer_btn,
            keyboard,
            key_row,
            action_row,
            change_btn,
            reset_key_btn,
            hint,
            updating: Cell::new(false),
        }
    }

    fn clear_selection(&self) {
        self.keyboard.set_selected(None);
    }

    fn select_visual(&self) {
        self.keyboard.queue_draw();
    }

    fn update(&self, ui: &AppUi) {
        let state = ui.state.borrow();
        let layer = state.layer as usize;
        let connected = state.connected;
        let keys = state.keys[layer].clone();
        let selected = state.selected;
        drop(state);

        self.updating.set(true);
        self.layer_combo.set_selected(layer as u32);
        self.updating.set(false);

        if let Some(keys) = &keys {
            self.keyboard.set_layer(&ui.model, layer, keys);
            self.keyboard.set_selected(selected);
            self.hint.set_text("点击按键进行编辑。");
        } else {
            self.hint.set_text(if connected {
                "正在读取按键…"
            } else {
                "设备未连接。"
            });
        }

        let available = keys.is_some() && connected;
        self.reset_layer_btn.set_sensitive(available);
        self.change_btn
            .set_sensitive(available && selected.is_some());
        self.reset_key_btn
            .set_sensitive(available && selected.is_some());

        if let Some(matrix) = selected {
            let key_name = ui
                .model
                .key_for_matrix(matrix)
                .map(|key| key.name.clone())
                .unwrap_or_else(|| format!("#{matrix}"));
            self.key_row.set_title(&key_name);
            self.key_row.set_subtitle(match layer {
                0 => "默认层",
                1 => "Fn 层",
                _ => "Fn2 层",
            });
            if let (Some(keys), true) = (&keys, available) {
                let value = keys.keys.get(matrix).copied().unwrap_or([0; 4]);
                self.action_row.set_subtitle(&ui.model.action_label(value));
            } else {
                self.action_row.set_subtitle("—");
            }
        } else {
            self.key_row.set_title("未选择按键");
            self.key_row.set_subtitle("点击上方键盘选择");
            self.action_row.set_subtitle("—");
        }
    }
}

struct LightingPage {
    root: gtk::ScrolledWindow,
    effect_list: gtk::ListBox,
    checks: Vec<gtk::Image>,
    mode: Cell<u8>,
    brightness_row: adw::ActionRow,
    brightness: gtk::Scale,
    speed_row: adw::ActionRow,
    speed: gtk::Scale,
    multi_row: adw::SwitchRow,
    color_row: adw::ActionRow,
    color: gtk::ColorDialogButton,
    note: gtk::Label,
    updating: Cell<bool>,
}

impl LightingPage {
    fn new(weak: &Weak<AppUi>, model: &Model) -> Self {
        let (root, content) = page_container(
            "灯效",
            "选择灯效并调整颜色、亮度与速度。修改会立即写入键盘。",
        );

        let effects_group = section(&content, "灯效");
        let effect_list = gtk::ListBox::new();
        effect_list.set_selection_mode(gtk::SelectionMode::None);
        effect_list.add_css_class("boxed-list");
        let mut checks = Vec::new();
        for effect in &model.effects {
            let row = gtk::ListBoxRow::new();
            row.set_activatable(true);
            let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            row_box.set_margin_top(10);
            row_box.set_margin_bottom(10);
            row_box.set_margin_start(12);
            row_box.set_margin_end(12);
            let icon = gtk::Image::from_icon_name(if effect.mode == 0 {
                "mchose-light-off-symbolic"
            } else {
                "mchose-light-symbolic"
            });
            icon.set_pixel_size(18);
            icon.add_css_class("dim-label");
            row_box.append(&icon);
            let label = gtk::Label::new(Some(&effect.name));
            label.set_xalign(0.0);
            row_box.append(&label);
            let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            spacer.set_hexpand(true);
            row_box.append(&spacer);
            let check = gtk::Image::from_icon_name("mchose-check-symbolic");
            check.set_visible(false);
            row_box.append(&check);
            row.set_child(Some(&row_box));
            effect_list.append(&row);
            checks.push(check);
        }
        {
            let weak = weak.clone();
            effect_list.connect_row_activated(move |_, row| {
                if let Some(ui) = weak.upgrade() {
                    if ui.lighting_page.updating.get() {
                        return;
                    }
                    let index = row.index();
                    if let Some(effect) = ui.model.effects.get(index as usize) {
                        ui.lighting_select_mode(effect.mode);
                    }
                }
            });
        }
        effects_group.add(&effect_list);

        let params = section(&content, "参数");
        let brightness_row = adw::ActionRow::builder().title("亮度").build();
        let brightness = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 5.0, 1.0);
        brightness.set_draw_value(true);
        brightness.set_size_request(220, -1);
        brightness_row.add_suffix(&brightness);
        params.add(&brightness_row);

        let speed_row = adw::ActionRow::builder().title("速度").build();
        let speed = gtk::Scale::with_range(gtk::Orientation::Horizontal, 1.0, 5.0, 1.0);
        speed.set_draw_value(true);
        speed.set_size_request(220, -1);
        speed_row.add_suffix(&speed);
        params.add(&speed_row);

        let multi_row = adw::SwitchRow::builder()
            .title("多彩")
            .subtitle("使用多彩渐变")
            .build();
        params.add(&multi_row);

        let color_row = adw::ActionRow::builder().title("颜色").build();
        let color = gtk::ColorDialogButton::new(Some(gtk::ColorDialog::new()));
        color.set_valign(gtk::Align::Center);
        color_row.add_suffix(&color);
        params.add(&color_row);

        let note = gtk::Label::new(None);
        note.add_css_class("dim-label");
        note.set_wrap(true);
        note.set_xalign(0.0);
        content.append(&note);

        let reset_group = section(&content, "恢复");
        let reset = pill_button("恢复灯效颜色默认值");
        {
            let weak = weak.clone();
            reset.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    confirm(
                        &ui,
                        "恢复灯效颜色？",
                        "所有灯效模式的颜色会恢复为出厂设置。",
                        "恢复",
                        |ui| ui.send(Request::ResetLight),
                    );
                }
            });
        }
        reset_group.add(&reset);

        {
            let weak = weak.clone();
            brightness.connect_value_changed(move |_| {
                if let Some(ui) = weak.upgrade() {
                    if ui.lighting_page.updating.get() {
                        return;
                    }
                    ui.lighting_page.schedule(&ui);
                }
            });
        }
        {
            let weak = weak.clone();
            speed.connect_value_changed(move |_| {
                if let Some(ui) = weak.upgrade() {
                    if ui.lighting_page.updating.get() {
                        return;
                    }
                    ui.lighting_page.schedule(&ui);
                }
            });
        }
        {
            let weak = weak.clone();
            multi_row.connect_active_notify(move |_| {
                if let Some(ui) = weak.upgrade() {
                    if ui.lighting_page.updating.get() {
                        return;
                    }
                    ui.lighting_page.schedule(&ui);
                }
            });
        }
        {
            let weak = weak.clone();
            color.connect_rgba_notify(move |_| {
                if let Some(ui) = weak.upgrade() {
                    if ui.lighting_page.updating.get() {
                        return;
                    }
                    ui.lighting_page.schedule(&ui);
                }
            });
        }

        Self {
            root,
            effect_list,
            checks,
            mode: Cell::new(0),
            brightness_row,
            brightness,
            speed_row,
            speed,
            multi_row,
            color_row,
            color,
            note,
            updating: Cell::new(false),
        }
    }

    fn schedule(&self, ui: &Rc<AppUi>) {
        if let Some((performance, light)) = self.desired(ui) {
            ui.schedule_lighting(performance, light);
        }
    }

    fn desired(&self, ui: &AppUi) -> Option<(Performance, LightState)> {
        let state = ui.state.borrow();
        let mut performance = state.performance.clone()?;
        let mut light = state.light.clone()?;
        drop(state);
        let mode = self.mode.get();
        performance.set_light_mode(mode);
        if let Some(effect) = ui.model.effect(mode) {
            if effect.brightness || effect.speed || effect.multi {
                let mut settings =
                    performance
                        .effect_settings(mode)
                        .unwrap_or(crate::protocol::EffectSettings {
                            brightness: 3,
                            speed: 3,
                            multi_color: true,
                        });
                if effect.brightness {
                    settings.brightness = self.brightness.value().round() as u8;
                }
                if effect.speed {
                    settings.speed = self.speed.value().round() as u8;
                }
                if effect.multi {
                    settings.multi_color = self.multi_row.is_active();
                }
                performance.set_effect_settings(mode, settings);
            }
            if effect.color {
                let rgba = self.color.rgba();
                light.set_color(
                    mode,
                    [
                        (rgba.red() * 255.0).round() as u8,
                        (rgba.green() * 255.0).round() as u8,
                        (rgba.blue() * 255.0).round() as u8,
                    ],
                );
            }
        }
        Some((performance, light))
    }

    fn apply_state(&self, ui: &AppUi, mode: u8, performance: &Performance, light: &LightState) {
        self.updating.set(true);
        self.mode.set(mode);
        for (index, effect) in ui.model.effects.iter().enumerate() {
            if let Some(check) = self.checks.get(index) {
                check.set_visible(effect.mode == mode);
            }
        }
        let effect = ui.model.effect(mode);
        let settings = performance.effect_settings(mode);
        if let Some(settings) = settings {
            self.brightness.set_value(settings.brightness as f64);
            self.speed.set_value(settings.speed as f64);
            self.multi_row.set_active(settings.multi_color);
        }
        if let Some(color) = light.color(mode) {
            self.color.set_rgba(&gdk::RGBA::new(
                color[0] as f32 / 255.0,
                color[1] as f32 / 255.0,
                color[2] as f32 / 255.0,
                1.0,
            ));
        }
        let (brightness, speed, multi, color, enabled) = effect
            .map_or((false, false, false, false, false), |e| {
                (e.brightness, e.speed, e.multi, e.color, true)
            });
        self.brightness_row.set_sensitive(enabled && brightness);
        self.speed_row.set_sensitive(enabled && speed);
        self.multi_row.set_sensitive(enabled && multi);
        self.color_row.set_sensitive(enabled && color);
        if !enabled {
            if mode == 0 {
                self.note.set_text("灯效已关闭。");
            } else {
                self.note
                    .set_text(&format!("当前灯效（{mode}）不在支持列表中。"));
            }
        } else if mode == 21 {
            self.note
                .set_text("自定义灯效：逐键颜色编辑暂未支持，当前会保留键盘中的自定义灯效。");
        } else if !color && !brightness && !speed && !multi {
            self.note.set_text("该灯效为动态效果，不提供参数调节。");
        } else {
            self.note.set_text("");
        }
        self.note.set_visible(!self.note.text().is_empty());
        self.effect_list.unselect_all();
        self.updating.set(false);
    }

    fn update(&self, ui: &AppUi) {
        let state = ui.state.borrow();
        let performance = state.performance.clone();
        let light = state.light.clone();
        let connected = state.connected;
        drop(state);
        if let (Some(performance), Some(light)) = (performance, light) {
            let mode = performance.light_mode();
            self.apply_state(ui, mode, &performance, &light);
            self.effect_list.set_sensitive(true);
        } else {
            self.updating.set(true);
            self.effect_list.set_sensitive(false);
            self.brightness_row.set_sensitive(false);
            self.speed_row.set_sensitive(false);
            self.multi_row.set_sensitive(false);
            self.color_row.set_sensitive(false);
            self.note.set_text(if connected {
                "等待设备数据…"
            } else {
                "设备未连接。"
            });
            self.note.set_visible(true);
            self.updating.set(false);
        }
    }
}

struct PerformancePage {
    root: gtk::ScrolledWindow,
    rate: adw::ComboRow,
    latency: adw::SwitchRow,
    sleep: adw::SpinRow,
    win_lock: adw::SwitchRow,
    mac: adw::SwitchRow,
    updating: Cell<bool>,
}

impl PerformancePage {
    fn new(weak: &Weak<AppUi>) -> Self {
        let (root, content) = page_container("性能", "调整回报率、低延迟、休眠与系统键。");

        let group = section(&content, "键盘性能");
        let rate = adw::ComboRow::new();
        rate.set_title("回报率");
        rate.set_subtitle("更高的回报率可以降低输入延迟");
        let rates = gtk::StringList::new(&["250 Hz", "500 Hz", "1000 Hz"]);
        rate.set_model(Some(&rates));
        group.add(&rate);

        let latency = adw::SwitchRow::builder()
            .title("低延迟模式")
            .subtitle("优先降低按键延迟，可能增加功耗")
            .build();
        group.add(&latency);

        let sleep = adw::SpinRow::with_range(0.0, 30.0, 1.0);
        sleep.set_title("休眠时间");
        sleep.set_subtitle("分钟；0 表示从不休眠");
        group.add(&sleep);

        let win_lock = adw::SwitchRow::builder()
            .title("Win 键锁")
            .subtitle("锁定 Windows 键，避免游戏中误触")
            .build();
        group.add(&win_lock);

        let mac = adw::SwitchRow::builder()
            .title("Mac 布局")
            .subtitle("软件交换 Alt/Win；固件切换用 Fn+A / Fn+S")
            .build();
        group.add(&mac);

        let reset_group = section(&content, "恢复");
        let reset_btn = pill_button("恢复性能默认值");
        reset_btn.add_css_class("destructive-action");
        {
            let weak = weak.clone();
            reset_btn.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    confirm(
                        &ui,
                        "恢复性能默认值？",
                        "回报率、低延迟、休眠和系统键设置会恢复为出厂设置。",
                        "恢复",
                        |ui| ui.send(Request::ResetPerformance),
                    );
                }
            });
        }
        reset_group.add(&reset_btn);

        {
            let weak = weak.clone();
            rate.connect_selected_notify(move |combo| {
                if let Some(ui) = weak.upgrade() {
                    if ui.performance_page.updating.get() {
                        return;
                    }
                    let Some(mut performance) = ui.state.borrow().performance.clone() else {
                        return;
                    };
                    performance.set_rate_index(combo.selected() as u8 + 1);
                    ui.send(Request::SetPerformance(performance));
                }
            });
        }
        {
            let weak = weak.clone();
            latency.connect_active_notify(move |row| {
                if let Some(ui) = weak.upgrade() {
                    if ui.performance_page.updating.get() {
                        return;
                    }
                    let Some(mut performance) = ui.state.borrow().performance.clone() else {
                        return;
                    };
                    performance.set_latency_on(row.is_active());
                    ui.send(Request::SetPerformance(performance));
                }
            });
        }
        {
            let weak = weak.clone();
            sleep.connect_value_notify(move |row| {
                if let Some(ui) = weak.upgrade() {
                    if ui.performance_page.updating.get() {
                        return;
                    }
                    let Some(mut performance) = ui.state.borrow().performance.clone() else {
                        return;
                    };
                    performance.set_sleep_minutes(row.value().round() as u16);
                    ui.send(Request::SetPerformance(performance));
                }
            });
        }
        {
            let weak = weak.clone();
            win_lock.connect_active_notify(move |row| {
                if let Some(ui) = weak.upgrade() {
                    if ui.performance_page.updating.get() {
                        return;
                    }
                    let Some(mut performance) = ui.state.borrow().performance.clone() else {
                        return;
                    };
                    performance.set_win_lock(row.is_active());
                    ui.send(Request::SetPerformance(performance));
                }
            });
        }
        {
            let weak = weak.clone();
            mac.connect_active_notify(move |row| {
                if let Some(ui) = weak.upgrade() {
                    if ui.performance_page.updating.get() {
                        return;
                    }
                    ui.set_mac_layout(row.is_active());
                }
            });
        }

        Self {
            root,
            rate,
            latency,
            sleep,
            win_lock,
            mac,
            updating: Cell::new(false),
        }
    }

    fn update(&self, ui: &AppUi) {
        let state = ui.state.borrow();
        let performance = state.performance.clone();
        let mac_layout = state.mac_layout;
        let mac_ready = state.keys[0].is_some() && !state.loading;
        drop(state);
        self.updating.set(true);
        if let Some(performance) = performance {
            let selected = match performance.rate_index() {
                1 => 0,
                2 => 1,
                3 => 2,
                _ => gtk::INVALID_LIST_POSITION,
            };
            self.rate.set_selected(selected);
            self.latency.set_active(performance.latency_on());
            self.sleep.set_value(performance.sleep_minutes() as f64);
            self.win_lock.set_active(performance.win_lock());
            self.mac.set_active(mac_layout);
            for row in [
                self.rate.upcast_ref::<gtk::Widget>(),
                self.latency.upcast_ref::<gtk::Widget>(),
                self.sleep.upcast_ref::<gtk::Widget>(),
                self.win_lock.upcast_ref::<gtk::Widget>(),
            ] {
                row.set_sensitive(true);
            }
            self.mac.set_sensitive(mac_ready);
        } else {
            self.rate.set_selected(gtk::INVALID_LIST_POSITION);
            self.latency.set_active(false);
            self.sleep.set_value(0.0);
            self.win_lock.set_active(false);
            self.mac.set_active(false);
            for row in [
                self.rate.upcast_ref::<gtk::Widget>(),
                self.latency.upcast_ref::<gtk::Widget>(),
                self.sleep.upcast_ref::<gtk::Widget>(),
                self.win_lock.upcast_ref::<gtk::Widget>(),
                self.mac.upcast_ref::<gtk::Widget>(),
            ] {
                row.set_sensitive(false);
            }
        }
        self.updating.set(false);
    }
}

struct SettingsPage {
    root: gtk::ScrolledWindow,
    simulate_value: gtk::Label,
}

impl SettingsPage {
    fn new(weak: &Weak<AppUi>, model: &Model) -> Self {
        let (root, content) = page_container("设置", "支持范围、权限与应用信息。");

        let support = section(&content, "支持范围");
        let model_value = value_row(&support, "型号");
        model_value.set_text(&format!("{}（USB 有线 / 2.4G 无线）", model.name));
        let detail = adw::ActionRow::builder()
            .title("当前版本支持")
            .subtitle("K99 V2 的设置项；固件刷写、云端配置和音频驱动不在支持范围内。")
            .build();
        detail.set_subtitle_lines(3);
        support.add(&detail);

        let simulate = section(&content, "运行状态");
        let simulate_value = value_row(&simulate, "模拟设备");

        let permissions = section(&content, "HID 权限");
        let explain = adw::ActionRow::builder()
            .title("权限不足时")
            .subtitle("读取设备提示无权限时，请安装以下 udev 规则并重新插拔键盘。")
            .build();
        explain.set_subtitle_lines(2);
        permissions.add(&explain);

        let rule_row = adw::ActionRow::builder()
            .title("udev 规则")
            .subtitle(UDEV_RULE)
            .build();
        rule_row.set_subtitle_lines(2);
        rule_row.add_css_class("udev-rule");
        let copy = gtk::Button::from_icon_name("mchose-copy-symbolic");
        copy.set_tooltip_text(Some("复制 udev 规则"));
        copy.set_valign(gtk::Align::Center);
        copy.add_css_class("flat");
        {
            let weak = weak.clone();
            copy.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    ui.window.clipboard().set_text(UDEV_RULE);
                    ui.toast("已复制 udev 规则");
                }
            });
        }
        rule_row.add_suffix(&copy);
        permissions.add(&rule_row);

        let about = section(&content, "关于");
        let version = adw::ActionRow::builder()
            .title("版本")
            .subtitle(crate::config::VERSION)
            .build();
        about.add(&version);
        let disclaimer = adw::ActionRow::builder()
            .title("非官方项目")
            .subtitle("本项目与迈从科技无隶属关系，协议为兼容设备而实现。")
            .build();
        disclaimer.set_subtitle_lines(2);
        about.add(&disclaimer);
        let about_btn = pill_button("关于 MCHOSE HUB");
        {
            let weak = weak.clone();
            about_btn.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    let _ = gtk::prelude::WidgetExt::activate_action(&ui.window, "app.about", None);
                }
            });
        }
        about.add(&about_btn);

        Self {
            root,
            simulate_value,
        }
    }

    fn update(&self, ui: &AppUi) {
        let state = ui.state.borrow();
        self.simulate_value
            .set_text(if state.transport == Some(Transport::Simulated) {
                "是"
            } else {
                "否"
            });
    }
}

fn open_action_picker(ui: &Rc<AppUi>, matrix: usize, current: [u8; 4]) {
    let dialog = adw::Dialog::builder()
        .title("选择功能")
        .content_width(760)
        .content_height(640)
        .build();

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);

    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("搜索功能…"));
    content.append(&search);

    let stack = gtk::Stack::new();
    stack.set_vexpand(true);
    let switcher = gtk::StackSwitcher::new();
    switcher.set_stack(Some(&stack));
    switcher.set_halign(gtk::Align::Center);
    content.append(&switcher);

    let all_buttons: Rc<RefCell<Vec<(gtk::Button, String)>>> = Rc::new(RefCell::new(Vec::new()));
    let categories: [(&str, &str, &Vec<crate::model::ActionItem>); 4] = [
        ("basic", "键盘", &ui.model.actions.basic),
        ("mouse", "鼠标", &ui.model.actions.mouse),
        ("media", "媒体", &ui.model.actions.media),
        ("system", "快捷", &ui.model.actions.system),
    ];
    for (id, title, actions) in categories {
        let flow = gtk::FlowBox::new();
        flow.set_selection_mode(gtk::SelectionMode::None);
        flow.set_max_children_per_line(9);
        flow.set_column_spacing(6);
        flow.set_row_spacing(6);
        flow.set_margin_top(8);
        flow.set_margin_bottom(8);
        flow.set_margin_start(8);
        flow.set_margin_end(8);
        for action in actions {
            let button = gtk::Button::with_label(&action.label);
            button.set_size_request(76, 36);
            button.add_css_class("flat");
            button.set_tooltip_text(Some(&action.label));
            if action.value == current {
                button.add_css_class("suggested-action");
            }
            let value = action.value;
            let weak = Rc::downgrade(ui);
            let dialog_clone = dialog.clone();
            button.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    ui.apply_key_action(matrix, value);
                }
                dialog_clone.close();
            });
            flow.insert(&button, -1);
            all_buttons
                .borrow_mut()
                .push((button, action.label.to_lowercase()));
        }
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&flow)
            .build();
        stack.add_titled(&scroll, Some(id), title);
    }

    let searching = all_buttons.clone();
    search.connect_search_changed(move |entry| {
        let query = entry.text().to_lowercase();
        for (button, label) in searching.borrow().iter() {
            button.set_visible(query.is_empty() || label.contains(&query));
        }
    });

    content.append(&stack);

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.set_halign(gtk::Align::End);
    footer.set_margin_top(8);
    footer.set_margin_bottom(4);
    footer.set_margin_start(4);
    footer.set_margin_end(4);
    let disable = gtk::Button::with_label("设为禁用");
    if current == [0, 0, 0, 0] {
        disable.add_css_class("suggested-action");
    }
    {
        let weak = Rc::downgrade(ui);
        let dialog_clone = dialog.clone();
        disable.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.apply_key_action(matrix, [0, 0, 0, 0]);
            }
            dialog_clone.close();
        });
    }
    let cancel = gtk::Button::with_label("取消");
    {
        let dialog_clone = dialog.clone();
        cancel.connect_clicked(move |_| {
            dialog_clone.close();
        });
    }
    footer.append(&disable);
    footer.append(&cancel);
    content.append(&footer);

    let content = content;
    dialog.set_child(Some(&content));
    dialog.present(Some(&ui.window));
}

#[cfg(test)]
mod tests {
    use gtk::gio;

    #[test]
    fn bundled_icons_are_embedded() {
        let data = include_bytes!(concat!(env!("OUT_DIR"), "/mchose_icons.gresource"));
        let resource =
            gio::Resource::from_data(&gtk::glib::Bytes::from_static(data)).expect("icon resource");
        for path in [
            "/top/ddmt/mchosehub/icons/16x16/actions/mchose-device-symbolic.svg",
            "/top/ddmt/mchosehub/icons/scalable/actions/mchose-copy-symbolic.svg",
            "/top/ddmt/mchosehub/icons/scalable/apps/top.ddmt.mchosehub.svg",
        ] {
            assert!(
                resource
                    .lookup_data(path, gio::ResourceLookupFlags::NONE)
                    .is_ok(),
                "missing bundled icon: {path}"
            );
        }
    }
}
