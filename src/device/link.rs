use std::path::PathBuf;
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::macro_table::{self, MacroDef};
use crate::model::Model;
use crate::protocol::{
    build_write_packets, get_battery_request, get_diy_request, get_keys_request, get_light_request,
    get_macro_request, get_performance_request, wired_battery_request, wired_diy_request,
    wired_diy_write, wired_keys_request, wired_keys_write, wired_light_request, wired_light_write,
    wired_macro_request, wired_macro_writes, wired_performance_request, wired_performance_write,
    wired_response, Battery, DiyLight, KeyLayer, LightState, Packet, Performance, ReadAssembler,
    WriteMode, CMD_GET_BATTERY, CMD_GET_DIY, CMD_GET_KEYS, CMD_GET_LIGHT, CMD_GET_MACRO,
    CMD_GET_PERFORMANCE, CMD_SET_DIY, CMD_SET_KEYS, CMD_SET_LIGHT, CMD_SET_MACRO,
    CMD_SET_PERFORMANCE, DIY_LEN, LIGHT_LEN, MACRO_LEN, PERF_LEN, REPORT_ID, WIRED_CMD_GET_BATTERY,
    WIRED_CMD_GET_DIY, WIRED_CMD_GET_KEYS, WIRED_CMD_GET_LIGHT, WIRED_CMD_GET_MACRO,
    WIRED_CMD_GET_PERFORMANCE, WIRED_KEYS_READ_LEN, WIRED_REPORT_LEN,
};

use super::hidraw::{find_receiver, find_wired, DeviceError, DeviceInfo, HidrawDevice};

pub trait KeyboardLink: Send {
    fn info(&self) -> &DeviceInfo;
    fn is_present(&self) -> bool;
    fn read_performance(&mut self) -> Result<Performance, DeviceError>;
    fn write_performance(&mut self, performance: &Performance) -> Result<(), DeviceError>;
    fn read_keys(&mut self, layer: u8) -> Result<KeyLayer, DeviceError>;
    fn write_keys(&mut self, layer: u8, keys: &KeyLayer) -> Result<(), DeviceError>;
    fn read_light(&mut self) -> Result<LightState, DeviceError>;
    fn write_light(&mut self, light: &LightState) -> Result<(), DeviceError>;
    fn read_battery(&mut self) -> Result<Battery, DeviceError>;
    /// 便宜的存活探测：键盘休眠时用它一次判定，避免整轮读取各堵满超时。
    fn probe(&mut self) -> bool;
    fn read_diy(&mut self) -> Result<DiyLight, DeviceError>;
    fn write_diy(&mut self, diy: &DiyLight) -> Result<(), DeviceError>;
    fn read_macros(&mut self) -> Result<Vec<MacroDef>, DeviceError>;
    fn write_macros(&mut self, macros: &[MacroDef]) -> Result<(), DeviceError>;
}

pub struct RealLink {
    dev: HidrawDevice,
}

impl RealLink {
    pub fn open_info(info: DeviceInfo) -> Result<Self, DeviceError> {
        log::info!("using {} ({})", info.product_name, info.hidraw);
        let dev = HidrawDevice::open(info)?;
        Ok(Self { dev })
    }

    fn read_response(
        &mut self,
        request: &[u8; 20],
        expected: u8,
        timeout: Duration,
    ) -> Result<Vec<u8>, DeviceError> {
        self.read_response_with(request, expected, timeout, 3)
    }

    fn read_response_with(
        &mut self,
        request: &[u8; 20],
        expected: u8,
        timeout: Duration,
        attempts: u32,
    ) -> Result<Vec<u8>, DeviceError> {
        for attempt in 0..attempts {
            self.dev.drain();
            log::debug!("tx {:02x?} attempt={attempt}", request);
            self.dev.write_report(request)?;
            let mut assembler = ReadAssembler::new(expected);
            let deadline = Instant::now() + timeout;
            while Instant::now() < deadline {
                let remaining = deadline.saturating_duration_since(Instant::now());
                let wait = remaining.min(Duration::from_millis(200));
                match self.dev.read_report(wait)? {
                    Some(buf) => {
                        log::debug!("rx {:02x?}", buf);
                        if buf.len() < 20 || buf[0] != REPORT_ID {
                            continue;
                        }
                        let Ok(packet) = Packet::decode(&buf[1..20]) else {
                            log::debug!("packet decode failed");
                            continue;
                        };
                        if packet.command != expected {
                            continue;
                        }
                        if assembler.push(&packet) {
                            return Ok(assembler.data());
                        }
                    }
                    None => continue,
                }
            }
            log::warn!("read command {expected:#04x} timed out (attempt {attempt})");
            sleep(Duration::from_millis(80));
        }
        Err(DeviceError::Timeout)
    }

    fn write_packets(
        &mut self,
        command: u8,
        data: &[u8],
        mode: WriteMode,
    ) -> Result<(), DeviceError> {
        self.dev.drain();
        let packets = build_write_packets(command, data, mode);
        for (index, packet) in packets.iter().enumerate() {
            let mut acked = false;
            for attempt in 0..3 {
                self.dev.write_report(packet)?;
                if self.wait_for_ack(command, index as u8, Duration::from_millis(1500))? {
                    acked = true;
                    break;
                }
                sleep(Duration::from_millis(100 * (attempt + 1)));
            }
            if !acked {
                log::warn!("no ack for command {command:#04x} packet {index}");
                return Err(DeviceError::Timeout);
            }
        }
        sleep(Duration::from_millis(60));
        Ok(())
    }

    fn wait_for_ack(
        &mut self,
        command: u8,
        index: u8,
        timeout: Duration,
    ) -> Result<bool, DeviceError> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self
                .dev
                .read_report(remaining.min(Duration::from_millis(200)))?
            {
                Some(buf) => {
                    if buf.len() < 20 || buf[0] != REPORT_ID {
                        continue;
                    }
                    let Ok(packet) = Packet::decode(&buf[1..20]) else {
                        continue;
                    };
                    if packet.command == command && packet.index == index {
                        return Ok(true);
                    }
                }
                None => continue,
            }
        }
        Ok(false)
    }
}

impl KeyboardLink for RealLink {
    fn info(&self) -> &DeviceInfo {
        &self.dev.info
    }

    fn is_present(&self) -> bool {
        self.dev.is_present()
    }

    fn read_performance(&mut self) -> Result<Performance, DeviceError> {
        let data = self.read_response(
            &get_performance_request(),
            CMD_GET_PERFORMANCE,
            Duration::from_secs(4),
        )?;
        Ok(Performance::from_bytes(&data)?)
    }

    fn write_performance(&mut self, performance: &Performance) -> Result<(), DeviceError> {
        self.write_packets(CMD_SET_PERFORMANCE, performance.raw(), WriteMode::ExactLast)?;
        let readback = self.read_performance()?;
        if readback.raw() != performance.raw() {
            return Err(DeviceError::Verify("性能设置写回校验失败"));
        }
        Ok(())
    }

    fn read_keys(&mut self, layer: u8) -> Result<KeyLayer, DeviceError> {
        let data = self.read_response(
            &get_keys_request(layer),
            CMD_GET_KEYS,
            Duration::from_secs(5),
        )?;
        Ok(KeyLayer::from_read(&data)?)
    }

    fn write_keys(&mut self, layer: u8, keys: &KeyLayer) -> Result<(), DeviceError> {
        let bytes = keys.to_write_bytes();
        self.write_packets(CMD_SET_KEYS, &bytes, WriteMode::Layer(layer))?;
        let readback = self.read_keys(layer)?;
        if readback.keys != keys.keys {
            return Err(DeviceError::Verify("按键写回校验失败"));
        }
        Ok(())
    }

    fn read_light(&mut self) -> Result<LightState, DeviceError> {
        let data =
            self.read_response(&get_light_request(), CMD_GET_LIGHT, Duration::from_secs(5))?;
        Ok(LightState::from_bytes(&data)?)
    }

    fn write_light(&mut self, light: &LightState) -> Result<(), DeviceError> {
        let bytes = light.to_write_bytes();
        self.write_packets(CMD_SET_LIGHT, &bytes, WriteMode::TrimLast)?;
        let readback = self.read_light()?;
        if readback.raw() != light.raw() {
            return Err(DeviceError::Verify("灯效写回校验失败"));
        }
        Ok(())
    }

    fn read_battery(&mut self) -> Result<Battery, DeviceError> {
        let data = self.read_response(
            &get_battery_request(),
            CMD_GET_BATTERY,
            Duration::from_secs(3),
        )?;
        Ok(Battery::from_bytes(&data)?)
    }

    fn probe(&mut self) -> bool {
        self.read_response_with(&get_battery_request(), CMD_GET_BATTERY, Duration::from_millis(700), 1)
            .is_ok()
    }

    fn read_diy(&mut self) -> Result<DiyLight, DeviceError> {
        let data = self.read_response(&get_diy_request(), CMD_GET_DIY, Duration::from_secs(5))?;
        Ok(DiyLight::from_bytes(&data)?)
    }

    fn write_diy(&mut self, diy: &DiyLight) -> Result<(), DeviceError> {
        self.write_packets(CMD_SET_DIY, diy.raw(), WriteMode::ExactLast)?;
        let readback = self.read_diy()?;
        if readback.raw() != diy.raw() {
            return Err(DeviceError::Verify("逐键灯效写回校验失败"));
        }
        Ok(())
    }

    fn read_macros(&mut self) -> Result<Vec<MacroDef>, DeviceError> {
        let data = self.read_response(&get_macro_request(), CMD_GET_MACRO, Duration::from_secs(6))?;
        decode_macros(&data)
    }

    /// 键盘不会回读这张表（官方同样只写不校验），所以写完即结束。
    fn write_macros(&mut self, macros: &[MacroDef]) -> Result<(), DeviceError> {
        let table = macro_table::encode(macros)?;
        self.write_packets(CMD_SET_MACRO, &table, WriteMode::ExactLast)
    }
}

pub struct WiredLink {
    dev: HidrawDevice,
    report_id: u8,
}

impl WiredLink {
    pub fn open_info(info: DeviceInfo, report_id: u8) -> Result<Self, DeviceError> {
        log::info!("using {} ({}) over USB", info.product_name, info.hidraw);
        let dev = HidrawDevice::open(info)?;
        Ok(Self { dev, report_id })
    }

    fn read_data(
        &mut self,
        request: &[u8; WIRED_REPORT_LEN],
        command: u8,
        length: usize,
    ) -> Result<Vec<u8>, DeviceError> {
        for attempt in 0..3 {
            self.send_feature(request)?;
            sleep(Duration::from_millis(60));
            let deadline = Instant::now() + Duration::from_millis(600);
            loop {
                match self.dev.get_feature(self.report_id) {
                    Ok(buffer) => match wired_response(&buffer, command) {
                        Ok(data) if data.len() >= length => {
                            return Ok(data[..length].to_vec());
                        }
                        Ok(data) => log::debug!(
                            "wired command {command:#04x}: short response ({} bytes)",
                            data.len()
                        ),
                        Err(e) => log::debug!("wired command {command:#04x}: {e}"),
                    },
                    Err(e) => log::debug!("wired get_feature failed: {e}"),
                }
                if Instant::now() >= deadline {
                    break;
                }
                sleep(Duration::from_millis(40));
            }
            log::warn!("wired command {command:#04x} timed out (attempt {attempt})");
        }
        Err(DeviceError::Timeout)
    }

    fn send_feature(&mut self, report: &[u8; WIRED_REPORT_LEN]) -> Result<(), DeviceError> {
        for attempt in 0..3 {
            match self.dev.set_feature(self.report_id, report) {
                Ok(()) => return Ok(()),
                Err(e) if attempt < 2 => {
                    log::warn!("wired feature write failed ({e}); retrying");
                    sleep(Duration::from_millis(120));
                }
                Err(e) => return Err(e),
            }
        }
        unreachable!()
    }

    fn send_report(&mut self, report: &[u8; WIRED_REPORT_LEN]) -> Result<(), DeviceError> {
        self.send_feature(report)?;
        sleep(Duration::from_millis(200));
        Ok(())
    }
}

impl KeyboardLink for WiredLink {
    fn info(&self) -> &DeviceInfo {
        &self.dev.info
    }

    fn is_present(&self) -> bool {
        self.dev.is_present()
    }

    fn read_performance(&mut self) -> Result<Performance, DeviceError> {
        let data = self.read_data(
            &wired_performance_request(),
            WIRED_CMD_GET_PERFORMANCE,
            PERF_LEN,
        )?;
        Ok(Performance::from_bytes(&data)?)
    }

    fn write_performance(&mut self, performance: &Performance) -> Result<(), DeviceError> {
        self.send_report(&wired_performance_write(performance))?;
        let readback = self.read_performance()?;
        if readback.raw() != performance.raw() {
            return Err(DeviceError::Verify("性能设置写回校验失败"));
        }
        Ok(())
    }

    fn read_keys(&mut self, layer: u8) -> Result<KeyLayer, DeviceError> {
        let data = self.read_data(
            &wired_keys_request(layer),
            WIRED_CMD_GET_KEYS,
            WIRED_KEYS_READ_LEN,
        )?;
        Ok(KeyLayer::from_read(&data)?)
    }

    fn write_keys(&mut self, layer: u8, keys: &KeyLayer) -> Result<(), DeviceError> {
        self.send_report(&wired_keys_write(layer, keys))?;
        let readback = self.read_keys(layer)?;
        if readback.keys != keys.keys {
            return Err(DeviceError::Verify("按键写回校验失败"));
        }
        Ok(())
    }

    fn read_light(&mut self) -> Result<LightState, DeviceError> {
        let data = self.read_data(&wired_light_request(), WIRED_CMD_GET_LIGHT, LIGHT_LEN)?;
        Ok(LightState::from_bytes(&data)?)
    }

    fn write_light(&mut self, light: &LightState) -> Result<(), DeviceError> {
        self.send_report(&wired_light_write(light))?;
        let readback = self.read_light()?;
        if readback.raw() != light.raw() {
            return Err(DeviceError::Verify("灯效写回校验失败"));
        }
        Ok(())
    }

    fn read_battery(&mut self) -> Result<Battery, DeviceError> {
        let data = self.read_data(&wired_battery_request(), WIRED_CMD_GET_BATTERY, 2)?;
        Ok(Battery::from_bytes(&data)?)
    }

    fn probe(&mut self) -> bool {
        self.read_battery().is_ok()
    }

    fn read_diy(&mut self) -> Result<DiyLight, DeviceError> {
        let data = self.read_data(&wired_diy_request(), WIRED_CMD_GET_DIY, DIY_LEN)?;
        Ok(DiyLight::from_bytes(&data)?)
    }

    fn write_diy(&mut self, diy: &DiyLight) -> Result<(), DeviceError> {
        self.send_report(&wired_diy_write(diy))?;
        let readback = self.read_diy()?;
        if readback.raw() != diy.raw() {
            return Err(DeviceError::Verify("逐键灯效写回校验失败"));
        }
        Ok(())
    }

    fn read_macros(&mut self) -> Result<Vec<MacroDef>, DeviceError> {
        let data = self.read_data(&wired_macro_request(), WIRED_CMD_GET_MACRO, MACRO_LEN)?;
        decode_macros(&data)
    }

    fn write_macros(&mut self, macros: &[MacroDef]) -> Result<(), DeviceError> {
        let table = macro_table::encode(macros)?;
        for report in wired_macro_writes(&table)? {
            self.send_report(&report)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Presence {
    #[default]
    Absent,
    Denied(PathBuf),
    Available(DeviceInfo),
}

impl Presence {
    pub fn info(&self) -> Option<&DeviceInfo> {
        match self {
            Self::Available(info) => Some(info),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Discovery {
    pub wired: Presence,
    pub wireless: Presence,
}

fn presence(result: Result<DeviceInfo, DeviceError>) -> Presence {
    match result {
        Ok(info) => Presence::Available(info),
        Err(DeviceError::Permission { path }) => Presence::Denied(path),
        Err(_) => Presence::Absent,
    }
}

/// An erased flash region is all `0xFF`, an untouched one all `0x00`; both
/// mean no commands rather than a parse failure.
pub fn decode_macros(data: &[u8]) -> Result<Vec<MacroDef>, DeviceError> {
    if data.iter().all(|byte| *byte == 0x00) || data.iter().all(|byte| *byte == 0xff) {
        return Ok(Vec::new());
    }
    macro_table::decode(data).map_err(|error| {
        let head: String = data.iter().take(32).map(|byte| format!("{byte:02x} ")).collect();
        log::warn!("cannot parse macro table ({} bytes): {error}; head={head}", data.len());
        DeviceError::Macro(error)
    })
}

pub fn discover(model: &Model) -> Discovery {
    Discovery {
        wired: presence(find_wired(model)),
        wireless: presence(find_receiver(model)),
    }
}

pub fn open_detected(
    discovery: &Discovery,
    model: &Model,
) -> Result<Box<dyn KeyboardLink>, DeviceError> {
    let mut last_error = None;
    if let Some(info) = discovery.wired.info() {
        match WiredLink::open_info(info.clone(), model.report.wired_id) {
            Ok(link) => return Ok(Box::new(link)),
            Err(e) => {
                log::warn!("cannot open wired interface: {e}");
                last_error = Some(e);
            }
        }
    }
    if let Some(info) = discovery.wireless.info() {
        match RealLink::open_info(info.clone()) {
            Ok(link) => return Ok(Box::new(link)),
            Err(e) => {
                log::warn!("cannot open wireless receiver: {e}");
                last_error = Some(e);
            }
        }
    }
    if let Some(e) = last_error {
        return Err(e);
    }
    for presence in [&discovery.wired, &discovery.wireless] {
        if let Presence::Denied(path) = presence {
            return Err(DeviceError::Permission { path: path.clone() });
        }
    }
    Err(DeviceError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::hidraw::Transport;
    use std::path::PathBuf;

    fn info(transport: Transport, path: &str) -> DeviceInfo {
        DeviceInfo {
            vendor_id: 0x05ac,
            product_id: 0x024f,
            product_name: "test".to_string(),
            hidraw: "test0".to_string(),
            path: PathBuf::from(path),
            transport,
        }
    }

    #[test]
    fn wired_is_preferred_when_both_are_available() {
        let model = Model::load();
        let discovery = Discovery {
            wired: Presence::Available(info(Transport::Wired, "/dev/null")),
            wireless: Presence::Available(info(Transport::Wireless, "/dev/null")),
        };
        let link = open_detected(&discovery, &model).unwrap();
        assert_eq!(link.info().transport, Transport::Wired);
    }

    #[test]
    fn falls_back_to_wireless_when_wired_cannot_open() {
        let model = Model::load();
        let discovery = Discovery {
            wired: Presence::Available(info(Transport::Wired, "/nonexistent/mchose")),
            wireless: Presence::Available(info(Transport::Wireless, "/dev/null")),
        };
        let link = open_detected(&discovery, &model).unwrap();
        assert_eq!(link.info().transport, Transport::Wireless);
    }
}
