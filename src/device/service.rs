use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::macro_table::MacroDef;
use crate::model::Model;
use crate::protocol::{Battery, DiyLight, KeyLayer, LightState, Performance};

use super::hidraw::{DeviceError, Transport};
use super::link::{discover, open_detected, Discovery, KeyboardLink, WiredLink};
use super::mock::MockLink;

const SLEEP_HINT: &str = "键盘未响应（2.4G 休眠），按任意键唤醒后点击刷新";

// Messages are short-lived; boxing the large variants would only add noise.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum Event {
    Detected(Discovery),
    Connected { transport: Transport },
    Disconnected,
    Battery(Battery),
    Performance(Performance),
    Light(LightState),
    Keys(u8, KeyLayer),
    Diy(DiyLight),
    Saved(&'static str),
    Healthy,
    Error(String),
    Busy(bool, String),
}

#[allow(clippy::large_enum_variant)]
pub enum Request {
    Refresh,
    /// 只读电量，供界面定时刷新充电状态；失败时静默跳过。
    PollBattery,
    LoadKeys,
    LoadDiy,
    SetPerformance(Performance),
    /// `macros` is written before the key layer so a key never points at a
    /// command slot the keyboard does not have yet.
    SetKeys {
        layer: u8,
        keys: KeyLayer,
    },
    SetLighting(Performance, LightState),
    SetDiy(DiyLight),
    /// Writing the command table and the layers that reference it has to happen
    /// in this order, otherwise a key can point at a slot the keyboard does not
    /// have yet.
    CommitMacros {
        macros: Vec<MacroDef>,
        layers: Vec<(u8, KeyLayer)>,
    },
    ResetPerformance,
    ResetKeys(u8),
    ResetLight,
    Shutdown,
}

pub struct DeviceService {
    tx: Sender<Request>,
    rx: async_channel::Receiver<Event>,
    focus: Arc<AtomicBool>,
    _handle: JoinHandle<()>,
}

impl DeviceService {
    pub fn start(model: Model, simulate: bool) -> Self {
        let (req_tx, req_rx) = std::sync::mpsc::channel();
        let (ev_tx, ev_rx) = async_channel::bounded(128);
        let focus = Arc::new(AtomicBool::new(true));
        let worker_focus = focus.clone();
        let handle = thread::Builder::new()
            .name("mchose-device".to_string())
            .spawn(move || worker(model, simulate, req_rx, ev_tx, worker_focus))
            .expect("failed to spawn device worker");
        Self {
            tx: req_tx,
            rx: ev_rx,
            focus,
            _handle: handle,
        }
    }

    pub fn events(&self) -> async_channel::Receiver<Event> {
        self.rx.clone()
    }

    pub fn send(&self, request: Request) {
        let _ = self.tx.send(request);
    }

    /// 界面失焦时暂停设备探测，不再反复敲休眠的键盘；回到前台立即补一次。
    pub fn set_focus(&self, active: bool) {
        self.focus.store(active, Ordering::Relaxed);
    }
}

impl Drop for DeviceService {
    fn drop(&mut self) {
        let _ = self.tx.send(Request::Shutdown);
    }
}

fn emit(tx: &async_channel::Sender<Event>, event: Event) {
    let _ = tx.send_blocking(event);
}

fn emit_error(tx: &async_channel::Sender<Event>, last: &mut String, message: impl Into<String>) {
    let message = message.into();
    if *last != message {
        log::warn!("{message}");
        *last = message.clone();
        emit(tx, Event::Error(message));
    }
}

fn publish_discovery(
    tx: &async_channel::Sender<Event>,
    last: &mut Option<Discovery>,
    discovery: Discovery,
) {
    if last.as_ref() == Some(&discovery) {
        return;
    }
    *last = Some(discovery.clone());
    emit(tx, Event::Detected(discovery));
}

fn read_all(
    link: &mut dyn KeyboardLink,
    tx: &async_channel::Sender<Event>,
    last_error: &mut String,
) -> bool {
    let mut ok = true;
    match link.read_battery() {
        Ok(battery) => emit(tx, Event::Battery(battery)),
        Err(e) => {
            emit_error(tx, last_error, format!("读取电量失败：{e}"));
            ok = false;
        }
    }
    match link.read_performance() {
        Ok(performance) => emit(tx, Event::Performance(performance)),
        Err(e) => {
            emit_error(tx, last_error, format!("读取性能设置失败：{e}"));
            ok = false;
        }
    }
    match link.read_light() {
        Ok(light) => emit(tx, Event::Light(light)),
        Err(e) => {
            emit_error(tx, last_error, format!("读取灯效失败：{e}"));
            ok = false;
        }
    }
    if ok {
        *last_error = String::new();
    }
    ok
}

fn announce_link(
    link: &mut dyn KeyboardLink,
    tx: &async_channel::Sender<Event>,
    last_error: &mut String,
) -> bool {
    let info = link.info().clone();
    log::info!("connected to {} ({})", info.product_name, info.hidraw);
    emit(
        tx,
        Event::Connected {
            transport: info.transport,
        },
    );
    if !link.probe() {
        log::warn!("device silent on {}; assume asleep", info.transport.label());
        emit(tx, Event::Busy(false, String::new()));
        emit(tx, Event::Error(SLEEP_HINT.to_string()));
        return false;
    }
    emit(tx, Event::Busy(true, "正在读取设备…".to_string()));
    if read_all(link, tx, last_error) {
        emit(tx, Event::Healthy);
    }
    emit(tx, Event::Busy(false, String::new()));
    *last_error = String::new();
    true
}

fn try_connect(
    model: &Model,
    simulate: bool,
    tx: &async_channel::Sender<Event>,
    last_error: &mut String,
    last_discovery: &mut Option<Discovery>,
) -> Option<(Box<dyn KeyboardLink>, bool)> {
    log::info!("try_connect simulate={simulate}");
    let mut link: Box<dyn KeyboardLink> = if simulate {
        Box::new(MockLink::new(model))
    } else {
        let discovery = discover(model);
        publish_discovery(tx, last_discovery, discovery.clone());
        match open_detected(&discovery, model) {
            Ok(link) => link,
            Err(DeviceError::NotFound) => return None,
            Err(e) => {
                emit_error(tx, last_error, e.to_string());
                return None;
            }
        }
    };
    let alive = announce_link(link.as_mut(), tx, last_error);
    Some((link, alive))
}

fn handle_request(
    request: Request,
    model: &Model,
    link: &mut Box<dyn KeyboardLink>,
    tx: &async_channel::Sender<Event>,
    last_error: &mut String,
    sleeping: &mut bool,
) -> bool {
    if *sleeping {
        if !link.probe() {
            emit(tx, Event::Error(SLEEP_HINT.to_string()));
            return true;
        }
        *sleeping = false;
    }
    let result = match request {
        Request::Shutdown => return false,
        Request::Refresh => {
            emit(tx, Event::Busy(true, "正在刷新…".to_string()));
            let ok = read_all(link.as_mut(), tx, last_error);
            emit(tx, Event::Busy(false, String::new()));
            if ok {
                emit(tx, Event::Healthy);
            }
            Ok(())
        }
        Request::PollBattery => match link.read_battery() {
            Ok(battery) => {
                emit(tx, Event::Battery(battery));
                Ok(())
            }
            Err(_) => {
                *sleeping = true;
                Ok(())
            }
        },
        Request::LoadKeys => {
            emit(tx, Event::Busy(true, "正在读取按键…".to_string()));
            let mut result = Ok(());
            for layer in 0..3u8 {
                match link.read_keys(layer) {
                    Ok(keys) => emit(tx, Event::Keys(layer, keys)),
                    Err(e) => {
                        result = Err(e);
                        break;
                    }
                }
            }
            emit(tx, Event::Busy(false, String::new()));
            result
        }
        Request::LoadDiy => {
            emit(tx, Event::Busy(true, "正在读取逐键灯效…".to_string()));
            let result = match link.read_diy() {
                Ok(diy) => {
                    emit(tx, Event::Diy(diy));
                    Ok(())
                }
                Err(e) => Err(e),
            };
            emit(tx, Event::Busy(false, String::new()));
            result
        }
        Request::SetDiy(diy) => {
            emit(tx, Event::Busy(true, "正在保存逐键灯效…".to_string()));
            let result = link.write_diy(&diy);
            if result.is_ok() {
                emit(tx, Event::Diy(diy));
                emit(tx, Event::Saved("逐键灯效已保存"));
            }
            emit(tx, Event::Busy(false, String::new()));
            result
        }
        Request::SetPerformance(performance) => {
            emit(tx, Event::Busy(true, "正在保存性能设置…".to_string()));
            let result = link.write_performance(&performance);
            if result.is_ok() {
                emit(tx, Event::Performance(performance));
                emit(tx, Event::Saved("性能设置已保存"));
            }
            emit(tx, Event::Busy(false, String::new()));
            result
        }
        Request::SetKeys { layer, keys } => {
            emit(tx, Event::Busy(true, "正在保存按键…".to_string()));
            let result = link.write_keys(layer, &keys);
            if result.is_ok() {
                emit(tx, Event::Keys(layer, keys));
                emit(tx, Event::Saved("按键已保存"));
            }
            emit(tx, Event::Busy(false, String::new()));
            result
        }
        Request::SetLighting(performance, light) => {
            emit(tx, Event::Busy(true, "正在保存灯效…".to_string()));
            let result = link
                .write_performance(&performance)
                .and_then(|_| link.write_light(&light));
            if result.is_ok() {
                emit(tx, Event::Performance(performance));
                emit(tx, Event::Light(light));
                emit(tx, Event::Saved("灯效已保存"));
            }
            emit(tx, Event::Busy(false, String::new()));
            result
        }
        Request::CommitMacros { macros, layers } => {
            emit(tx, Event::Busy(true, "正在保存快捷指令…".to_string()));
            let mut result = link.write_macros(&macros);
            if result.is_ok() {
                for (layer, keys) in &layers {
                    match link.write_keys(*layer, keys) {
                        Ok(()) => emit(tx, Event::Keys(*layer, keys.clone())),
                        Err(e) => {
                            result = Err(e);
                            break;
                        }
                    }
                }
            }
            if result.is_ok() {
                emit(tx, Event::Saved("快捷指令已保存"));
            }
            emit(tx, Event::Busy(false, String::new()));
            result
        }
        Request::ResetPerformance => {
            let performance = Performance::from_model_default(model);
            emit(tx, Event::Busy(true, "正在恢复默认…".to_string()));
            let result = link.write_performance(&performance);
            if result.is_ok() {
                emit(tx, Event::Performance(performance));
                emit(tx, Event::Saved("性能设置已恢复默认"));
            }
            emit(tx, Event::Busy(false, String::new()));
            result
        }
        Request::ResetKeys(layer) => {
            let keys = KeyLayer::from_defaults(model, layer as usize);
            emit(tx, Event::Busy(true, "正在恢复默认…".to_string()));
            let result = link.write_keys(layer, &keys);
            if result.is_ok() {
                emit(tx, Event::Keys(layer, keys));
                emit(tx, Event::Saved("按键已恢复默认"));
            }
            emit(tx, Event::Busy(false, String::new()));
            result
        }
        Request::ResetLight => {
            let light = LightState::from_model_default(model);
            emit(tx, Event::Busy(true, "正在恢复默认…".to_string()));
            let result = link.write_light(&light);
            if result.is_ok() {
                emit(tx, Event::Light(light));
                emit(tx, Event::Saved("灯效已恢复默认"));
            }
            emit(tx, Event::Busy(false, String::new()));
            result
        }
    };
    if let Err(e) = result {
        emit_error(tx, last_error, format!("操作失败：{e}"));
        return link.is_present();
    }
    *last_error = String::new();
    true
}

/// 探测节奏：醒着时快扫；键盘休眠时放缓（继续盯有线插入，但不再密集敲通道）。
/// 界面失焦时由调用方整体暂停，不在这条规则里。
fn probe_interval(sleeping: bool) -> Duration {
    if sleeping {
        Duration::from_millis(1500)
    } else {
        Duration::from_millis(900)
    }
}

fn worker(
    model: Model,
    simulate: bool,
    requests: Receiver<Request>,
    events: async_channel::Sender<Event>,
    focus: Arc<AtomicBool>,
) {
    let mut link: Option<Box<dyn KeyboardLink>> = None;
    let mut last_error = String::new();
    let mut last_discovery = None;
    let mut next_probe = Instant::now();
    let mut announced_empty = false;
    let mut sleeping = false;
    loop {
        // 失焦期间不推进 next_probe，于是回到前台的第一轮就立刻探测一次。
        if focus.load(Ordering::Relaxed) && Instant::now() >= next_probe {
            log::debug!("probe sleeping={sleeping} link={}", link.is_some());
            if link.is_none() {
                if let Some((new_link, alive)) = try_connect(
                    &model,
                    simulate,
                    &events,
                    &mut last_error,
                    &mut last_discovery,
                ) {
                    sleeping = !alive;
                    link = Some(new_link);
                    announced_empty = false;
                } else if !announced_empty && last_error.is_empty() {
                    emit(&events, Event::Disconnected);
                    announced_empty = true;
                }
            } else if !simulate {
                // 键盘休眠时通道不响应，只能靠盯着插拔来判断有线是否接入。
                let discovery = discover(&model);
                publish_discovery(&events, &mut last_discovery, discovery.clone());
                if let Some(info) = discovery.wired.info() {
                    if link
                        .as_ref()
                        .is_some_and(|current| current.info().transport == Transport::Wireless)
                    {
                        match WiredLink::open_info(info.clone(), model.report.wired_id) {
                            Ok(new_link) => {
                                let mut new_link: Box<dyn KeyboardLink> = Box::new(new_link);
                                sleeping = !announce_link(new_link.as_mut(), &events, &mut last_error);
                                link = Some(new_link);
                                announced_empty = false;
                            }
                            Err(e) => {
                                log::warn!("wired interface detected but cannot open: {e}");
                            }
                        }
                    }
                }
            }
            // 探测过才排下一次；失焦时不排，next_probe 停在已过期的时刻，
            // 于是回到前台的第一轮就立刻探测一次。
            next_probe = Instant::now() + probe_interval(sleeping);
        }

        if let Some(current) = &link {
            if !current.is_present() {
                link = None;
                sleeping = false;
                emit(&events, Event::Disconnected);
                announced_empty = true;
            }
        }

        match requests.recv_timeout(Duration::from_millis(250)) {
            Ok(request) => {
                if matches!(request, Request::Shutdown) {
                    break;
                }
                last_error.clear();
                if link.is_none() {
                    if let Some((new_link, alive)) = try_connect(
                        &model,
                        simulate,
                        &events,
                        &mut last_error,
                        &mut last_discovery,
                    ) {
                        sleeping = !alive;
                        link = Some(new_link);
                    }
                }
                let Some(current) = link.as_mut() else {
                    emit(
                        &events,
                        Event::Error("设备未连接，请连接 USB 线或插入 2.4G 接收器".to_string()),
                    );
                    continue;
                };
                if !handle_request(request, &model, current, &events, &mut last_error, &mut sleeping) {
                    link = None;
                    sleeping = false;
                    emit(&events, Event::Disconnected);
                    announced_empty = true;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn mock_service_emits_connected() {
        let service = DeviceService::start(Model::load(), true);
        let rx = service.events();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match rx.try_recv() {
                Ok(event) => {
                    println!("event: {event:?}");
                    if matches!(event, Event::Connected { .. }) {
                        return;
                    }
                }
                Err(async_channel::TryRecvError::Empty) => {
                    if Instant::now() > deadline {
                        panic!("timed out waiting for Connected");
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(async_channel::TryRecvError::Closed) => panic!("event channel closed"),
            }
        }
    }
}
