use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::model::Model;
use crate::protocol::{
    build_write_packets, get_battery_request, get_keys_request, get_light_request,
    get_performance_request, Battery, KeyLayer, LightState, Packet, Performance, ReadAssembler,
    WriteMode, CMD_GET_BATTERY, CMD_GET_KEYS, CMD_GET_LIGHT, CMD_GET_PERFORMANCE, CMD_SET_KEYS,
    CMD_SET_LIGHT, CMD_SET_PERFORMANCE, REPORT_ID,
};

use super::hidraw::{find_receiver, DeviceError, DeviceInfo, HidrawDevice};

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
}

pub struct RealLink {
    dev: HidrawDevice,
}

impl RealLink {
    pub fn open(model: &Model) -> Result<Self, DeviceError> {
        let info = find_receiver(model)?;
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
        for attempt in 0..3 {
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
}
