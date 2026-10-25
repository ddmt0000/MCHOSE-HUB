use std::path::PathBuf;

use crate::model::Model;
use crate::protocol::{Battery, KeyLayer, LightState, Performance};

use super::hidraw::{DeviceError, DeviceInfo, Transport};
use super::link::KeyboardLink;

pub struct MockLink {
    info: DeviceInfo,
    performance: Performance,
    keys: [KeyLayer; 3],
    light: LightState,
    battery: Battery,
}

impl MockLink {
    pub fn new(model: &Model) -> Self {
        Self {
            info: DeviceInfo {
                vendor_id: model.receiver.vendor_id,
                product_id: model.receiver.product_id,
                product_name: "K99 V2（模拟）".to_string(),
                hidraw: "mock0".to_string(),
                path: PathBuf::from("/dev/null"),
                transport: Transport::Simulated,
            },
            performance: Performance::from_model_default(model),
            keys: [
                KeyLayer::from_defaults(model, 0),
                KeyLayer::from_defaults(model, 1),
                KeyLayer::from_defaults(model, 2),
            ],
            light: LightState::from_model_default(model),
            battery: Battery {
                level: 68,
                status: 1,
            },
        }
    }
}

impl KeyboardLink for MockLink {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn is_present(&self) -> bool {
        true
    }

    fn read_performance(&mut self) -> Result<Performance, DeviceError> {
        Ok(self.performance.clone())
    }

    fn write_performance(&mut self, performance: &Performance) -> Result<(), DeviceError> {
        self.performance = performance.clone();
        Ok(())
    }

    fn read_keys(&mut self, layer: u8) -> Result<KeyLayer, DeviceError> {
        self.keys
            .get(layer as usize)
            .cloned()
            .ok_or(DeviceError::Verify("无效图层"))
    }

    fn write_keys(&mut self, layer: u8, keys: &KeyLayer) -> Result<(), DeviceError> {
        *self
            .keys
            .get_mut(layer as usize)
            .ok_or(DeviceError::Verify("无效图层"))? = keys.clone();
        Ok(())
    }

    fn read_light(&mut self) -> Result<LightState, DeviceError> {
        Ok(self.light.clone())
    }

    fn write_light(&mut self, light: &LightState) -> Result<(), DeviceError> {
        self.light = light.clone();
        Ok(())
    }

    fn read_battery(&mut self) -> Result<Battery, DeviceError> {
        Ok(self.battery)
    }
}
