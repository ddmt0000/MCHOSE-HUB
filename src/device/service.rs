use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::model::Model;
use crate::protocol::{Battery, KeyLayer, LightState, Performance};

use super::hidraw::DeviceError;
use super::link::{KeyboardLink, RealLink};
use super::mock::MockLink;

// Messages are short-lived; boxing the large variants would only add noise.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum Event {
    Connected {
        port: String,
        product_name: String,
        simulated: bool,
    },
    Disconnected,
    Battery(Battery),
    Performance(Performance),
    Light(LightState),
    Keys(u8, KeyLayer),
    Saved(&'static str),
    Healthy,
    Error(String),
    Busy(bool, String),
}

#[allow(clippy::large_enum_variant)]
pub enum Request {
    Refresh,
    LoadKeys,
    SetPerformance(Performance),
    SetKeys(u8, KeyLayer),
    SetLighting(Performance, LightState),
    ResetPerformance,
    ResetKeys(u8),
    ResetLight,
    Shutdown,
}

pub struct DeviceService {
    tx: Sender<Request>,
    rx: async_channel::Receiver<Event>,
    _handle: JoinHandle<()>,
}

impl DeviceService {
    pub fn start(model: Model, simulate: bool) -> Self {
        let (req_tx, req_rx) = std::sync::mpsc::channel();
        let (ev_tx, ev_rx) = async_channel::bounded(128);
        let handle = thread::Builder::new()
            .name("mchose-device".to_string())
            .spawn(move || worker(model, simulate, req_rx, ev_tx))
            .expect("failed to spawn device worker");
        Self {
            tx: req_tx,
            rx: ev_rx,
            _handle: handle,
        }
    }

    pub fn events(&self) -> async_channel::Receiver<Event> {
        self.rx.clone()
    }

    pub fn send(&self, request: Request) {
        let _ = self.tx.send(request);
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

fn try_connect(
    model: &Model,
    simulate: bool,
    tx: &async_channel::Sender<Event>,
    last_error: &mut String,
) -> Option<Box<dyn KeyboardLink>> {
    log::info!("try_connect simulate={simulate}");
    let mut link: Box<dyn KeyboardLink> = if simulate {
        Box::new(MockLink::new(model))
    } else {
        match RealLink::open(model) {
            Ok(link) => Box::new(link),
            Err(DeviceError::NotFound) => return None,
            Err(e) => {
                emit_error(tx, last_error, e.to_string());
                return None;
            }
        }
    };
    let info = link.info().clone();
    log::info!("connected to {}", info.product_name);
    emit(
        tx,
        Event::Connected {
            port: info.hidraw.clone(),
            product_name: info.product_name.clone(),
            simulated: simulate,
        },
    );
    emit(tx, Event::Busy(true, "正在读取设备…".to_string()));
    if read_all(link.as_mut(), tx, last_error) {
        emit(tx, Event::Healthy);
    }
    emit(tx, Event::Busy(false, String::new()));
    *last_error = String::new();
    Some(link)
}

fn handle_request(
    request: Request,
    model: &Model,
    link: &mut Box<dyn KeyboardLink>,
    tx: &async_channel::Sender<Event>,
    last_error: &mut String,
) -> bool {
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
        Request::SetKeys(layer, keys) => {
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

fn worker(
    model: Model,
    simulate: bool,
    requests: Receiver<Request>,
    events: async_channel::Sender<Event>,
) {
    let mut link: Option<Box<dyn KeyboardLink>> = None;
    let mut last_error = String::new();
    let mut last_scan = Instant::now() - Duration::from_secs(1);
    let mut announced_empty = false;
    loop {
        if link.is_none() && last_scan.elapsed() >= Duration::from_millis(900) {
            last_scan = Instant::now();
            if let Some(new_link) = try_connect(&model, simulate, &events, &mut last_error) {
                link = Some(new_link);
                announced_empty = false;
            } else if !announced_empty && last_error.is_empty() {
                emit(&events, Event::Disconnected);
                announced_empty = true;
            }
        }

        if let Some(current) = &link {
            if !current.is_present() {
                link = None;
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
                    if let Some(new_link) = try_connect(&model, simulate, &events, &mut last_error)
                    {
                        link = Some(new_link);
                    }
                }
                let Some(current) = link.as_mut() else {
                    emit(
                        &events,
                        Event::Error("设备未连接，请插入 2.4G 接收器".to_string()),
                    );
                    continue;
                };
                if !handle_request(request, &model, current, &events, &mut last_error) {
                    link = None;
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
