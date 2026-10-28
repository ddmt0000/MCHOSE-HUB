use std::path::PathBuf;

use crate::macro_table::MacroDef;
use crate::model::Model;
use crate::protocol::{Battery, DiyLight, KeyLayer, LightState, Performance};

use super::hidraw::{DeviceError, DeviceInfo, Transport};
use super::link::KeyboardLink;

pub struct MockLink {
    info: DeviceInfo,
    performance: Performance,
    keys: [KeyLayer; 3],
    light: LightState,
    battery: Battery,
    diy: DiyLight,
    macros: Vec<MacroDef>,
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
            diy: DiyLight::empty(),
            macros: demo_macros(),
        }
    }
}

/// Simulated devices start with two commands so the editor and the key picker
/// can be exercised without a keyboard attached.
fn demo_macros() -> Vec<MacroDef> {
    use crate::macro_table::{Chip, MacroEvent, NodeKind};

    let event = |kind, down| Chip::Event(MacroEvent { kind, down });
    vec![
        MacroDef {
            name: "连点 A".to_string(),
            chips: vec![
                event(NodeKind::Key(4), true),
                Chip::Delay(50),
                event(NodeKind::Key(4), false),
                Chip::Delay(50),
            ],
        },
        MacroDef {
            name: "复制粘贴".to_string(),
            chips: vec![
                event(NodeKind::Key(224), true),
                Chip::Delay(50),
                event(NodeKind::Key(6), true),
                Chip::Delay(50),
                event(NodeKind::Key(6), false),
                Chip::Delay(50),
                event(NodeKind::Key(224), false),
            ],
        },
    ]
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

    fn probe(&mut self) -> bool {
        true
    }

    fn read_diy(&mut self) -> Result<DiyLight, DeviceError> {
        Ok(self.diy.clone())
    }

    fn write_diy(&mut self, diy: &DiyLight) -> Result<(), DeviceError> {
        self.diy = diy.clone();
        Ok(())
    }

    fn read_macros(&mut self) -> Result<Vec<MacroDef>, DeviceError> {
        Ok(self.macros.clone())
    }

    fn write_macros(&mut self, macros: &[MacroDef]) -> Result<(), DeviceError> {
        self.macros = macros.to_vec();
        Ok(())
    }
}
