use std::collections::BTreeMap;

use thiserror::Error;

use crate::model::Model;

pub const REPORT_ID: u8 = 0x13;
pub const CHUNK: usize = 14;
pub const PACKET_PAYLOAD: usize = 19;
pub const KEY_COUNT: usize = 113;
pub const PERF_LEN: usize = 128;
pub const LIGHT_LEN: usize = 490;

pub const CMD_GET_KEYS: u8 = 0x41;
pub const CMD_GET_PERFORMANCE: u8 = 0x44;
pub const CMD_GET_LIGHT: u8 = 0x49;
pub const CMD_GET_BATTERY: u8 = 0x4a;
pub const CMD_SET_KEYS: u8 = 0x01;
pub const CMD_SET_PERFORMANCE: u8 = 0x04;
pub const CMD_SET_LIGHT: u8 = 0x09;

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("报文长度不足：期望 {expected}，实际 {actual}")]
    ShortPacket { expected: usize, actual: usize },
    #[error("校验和不匹配")]
    BadChecksum,
    #[error("数据长度不对：{0}")]
    BadLength(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub command: u8,
    pub total: u8,
    pub index: u8,
    pub yy: u8,
    pub data: [u8; CHUNK],
}

impl Packet {
    /// Decode a 19-byte payload read from the HID report (without report ID).
    pub fn decode(buf: &[u8]) -> Result<Self, ProtocolError> {
        if buf.len() < PACKET_PAYLOAD {
            return Err(ProtocolError::ShortPacket {
                expected: PACKET_PAYLOAD,
                actual: buf.len(),
            });
        }
        let mut sum = REPORT_ID as u32;
        for byte in &buf[..PACKET_PAYLOAD - 1] {
            sum += *byte as u32;
        }
        if (sum as u8) != buf[PACKET_PAYLOAD - 1] {
            return Err(ProtocolError::BadChecksum);
        }
        let mut data = [0u8; CHUNK];
        data.copy_from_slice(&buf[4..4 + CHUNK]);
        Ok(Self {
            command: buf[0],
            total: buf[1],
            index: buf[2],
            yy: buf[3],
            data,
        })
    }

    pub fn data_len(&self) -> usize {
        ((self.yy & 0x0f) as usize).min(CHUNK)
    }

    pub fn data_slice(&self) -> &[u8] {
        &self.data[..self.data_len()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    /// Last packet carries its literal byte count (performance).
    ExactLast,
    /// Last packet trims trailing zeroes (light).
    TrimLast,
    /// Key layers: YY high nibble is the layer, low nibble is valid length.
    Layer(u8),
}

fn trailing_zeroes(data: &[u8]) -> usize {
    data.iter().rev().take_while(|b| **b == 0).count()
}

pub fn build_write_packets(command: u8, data: &[u8], mode: WriteMode) -> Vec<[u8; 20]> {
    let total = data.len().div_ceil(CHUNK);
    let mut out = Vec::with_capacity(total);
    for index in 0..total {
        let chunk = &data[index * CHUNK..data.len().min((index + 1) * CHUNK)];
        let is_last = index + 1 == total;
        let yy = if is_last {
            match mode {
                WriteMode::ExactLast => chunk.len() as u8,
                WriteMode::TrimLast => (chunk.len() - trailing_zeroes(chunk)) as u8,
                WriteMode::Layer(layer) => {
                    let valid = chunk.len() - trailing_zeroes(chunk);
                    if valid == 0 {
                        0
                    } else {
                        (layer << 4) | valid as u8
                    }
                }
            }
        } else {
            match mode {
                WriteMode::Layer(layer) => (layer << 4) | CHUNK as u8,
                _ => CHUNK as u8,
            }
        };
        let mut payload = [0u8; PACKET_PAYLOAD];
        payload[0] = command;
        payload[1] = total as u8;
        payload[2] = index as u8;
        payload[3] = yy;
        payload[4..4 + chunk.len()].copy_from_slice(chunk);
        let mut sum = REPORT_ID as u32;
        for byte in &payload[..PACKET_PAYLOAD - 1] {
            sum += *byte as u32;
        }
        payload[PACKET_PAYLOAD - 1] = sum as u8;
        let mut report = [0u8; 20];
        report[0] = REPORT_ID;
        report[1..].copy_from_slice(&payload);
        out.push(report);
    }
    out
}

pub fn read_request(command: u8, selector: u8) -> [u8; 20] {
    let mut payload = [0u8; PACKET_PAYLOAD];
    payload[0] = command;
    payload[1] = 0x01;
    payload[3] = selector;
    let mut report = [0u8; 20];
    report[0] = REPORT_ID;
    report[1..].copy_from_slice(&payload);
    report
}

pub fn get_performance_request() -> [u8; 20] {
    read_request(CMD_GET_PERFORMANCE, 0)
}

pub fn get_keys_request(layer: u8) -> [u8; 20] {
    read_request(CMD_GET_KEYS, layer << 4)
}

pub fn get_light_request() -> [u8; 20] {
    read_request(CMD_GET_LIGHT, 0)
}

pub fn get_battery_request() -> [u8; 20] {
    read_request(CMD_GET_BATTERY, 0)
}

#[derive(Debug, Default)]
pub struct ReadAssembler {
    command: u8,
    total: Option<u8>,
    packets: BTreeMap<u8, Vec<u8>>,
}

impl ReadAssembler {
    pub fn new(command: u8) -> Self {
        Self {
            command,
            total: None,
            packets: BTreeMap::new(),
        }
    }

    pub fn push(&mut self, packet: &Packet) -> bool {
        if packet.command != self.command {
            return false;
        }
        self.total.get_or_insert(packet.total);
        if packet.total == 0 {
            return true;
        }
        self.packets
            .entry(packet.index)
            .or_insert_with(|| packet.data_slice().to_vec());
        self.is_complete()
    }

    pub fn is_complete(&self) -> bool {
        match self.total {
            Some(0) => true,
            Some(total) => self.packets.len() >= total as usize,
            None => false,
        }
    }

    pub fn data(&self) -> Vec<u8> {
        self.packets.values().flatten().copied().collect()
    }
}

#[derive(Debug, Clone)]
pub struct Performance {
    raw: [u8; PERF_LEN],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectSettings {
    pub brightness: u8,
    pub speed: u8,
    pub multi_color: bool,
}

pub const EFFECT_OFFSETS: &[(u8, usize)] = &[
    (1, 58),
    (2, 60),
    (3, 62),
    (4, 64),
    (5, 66),
    (7, 70),
    (8, 72),
    (10, 76),
    (11, 78),
    (12, 80),
    (13, 82),
    (15, 84),
    (16, 88),
    (17, 90),
];

impl Performance {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() < PERF_LEN {
            return Err(ProtocolError::BadLength(bytes.len()));
        }
        let mut raw = [0u8; PERF_LEN];
        raw.copy_from_slice(&bytes[..PERF_LEN]);
        Ok(Self { raw })
    }

    pub fn from_model_default(model: &Model) -> Self {
        let mut raw = [0u8; PERF_LEN];
        raw.copy_from_slice(&model.defaults.performance_raw);
        Self { raw }
    }

    pub fn raw(&self) -> &[u8; PERF_LEN] {
        &self.raw
    }

    pub fn rate_index(&self) -> u8 {
        self.raw[1]
    }

    pub fn rate_hz(&self) -> Option<u16> {
        match self.raw[1] {
            1 => Some(250),
            2 => Some(500),
            3 => Some(1000),
            _ => None,
        }
    }

    pub fn set_rate_index(&mut self, index: u8) {
        self.raw[1] = index.clamp(1, 3);
    }

    pub fn latency_on(&self) -> bool {
        self.raw[3] == 0
    }

    pub fn set_latency_on(&mut self, on: bool) {
        self.raw[3] = if on { 0 } else { 2 };
    }

    pub fn sleep_minutes(&self) -> u16 {
        (self.raw[24] as u16) / 2
    }

    pub fn set_sleep_minutes(&mut self, minutes: u16) {
        self.raw[24] = (minutes.saturating_mul(2)).min(255) as u8;
    }

    pub fn win_lock(&self) -> bool {
        self.raw[15] == 1
    }

    pub fn set_win_lock(&mut self, on: bool) {
        self.raw[15] = u8::from(on);
    }

    pub fn mac_mode(&self) -> bool {
        self.raw[27] == 1
    }

    pub fn set_mac_mode(&mut self, on: bool) {
        self.raw[27] = u8::from(on);
    }

    pub fn light_type(&self) -> u8 {
        self.raw[9]
    }

    pub fn light_mode(&self) -> u8 {
        self.raw[10]
    }

    pub fn set_light_mode(&mut self, mode: u8) {
        self.raw[10] = mode;
        self.raw[9] = if matches!(mode, 19 | 21) { 1 } else { 0 };
    }

    pub fn effect_settings(&self, mode: u8) -> Option<EffectSettings> {
        let (_, offset) = EFFECT_OFFSETS.iter().find(|(m, _)| *m == mode)?;
        let brightness = self.raw[*offset];
        let bits = self.raw[*offset + 1];
        Some(EffectSettings {
            brightness,
            speed: bits >> 4,
            multi_color: (bits & 0x0f) == 7,
        })
    }

    pub fn set_effect_settings(&mut self, mode: u8, settings: EffectSettings) {
        if let Some((_, offset)) = EFFECT_OFFSETS.iter().find(|(m, _)| *m == mode) {
            self.raw[*offset] = settings.brightness;
            self.raw[*offset + 1] =
                (settings.speed.clamp(0, 15) << 4) | if settings.multi_color { 7 } else { 0 };
        }
    }
}

#[derive(Debug, Clone)]
pub struct KeyLayer {
    pub keys: Vec<[u8; 4]>,
}

impl KeyLayer {
    pub fn from_read(data: &[u8]) -> Result<Self, ProtocolError> {
        if data.len() < KEY_COUNT * 4 {
            return Err(ProtocolError::BadLength(data.len()));
        }
        let keys = data[..KEY_COUNT * 4]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| [c[0], c[1], c[2], c[3]])
            .collect();
        Ok(Self { keys })
    }

    pub fn from_defaults(model: &Model, layer: usize) -> Self {
        Self {
            keys: model.default_layer(layer),
        }
    }

    /// The device expects the 452-byte key table followed by a 58-byte pad
    /// and the 0x5AA5 end marker.
    pub fn to_write_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(KEY_COUNT * 4 + 60);
        for key in &self.keys {
            out.extend_from_slice(key);
        }
        out.extend(std::iter::repeat_n(0u8, 58));
        out.extend_from_slice(&[90, 165]);
        out
    }
}

#[derive(Debug, Clone)]
pub struct LightState {
    raw: [u8; LIGHT_LEN],
}

impl LightState {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() < LIGHT_LEN {
            return Err(ProtocolError::BadLength(bytes.len()));
        }
        let mut raw = [0u8; LIGHT_LEN];
        raw.copy_from_slice(&bytes[..LIGHT_LEN]);
        Ok(Self { raw })
    }

    pub fn from_model_default(model: &Model) -> Self {
        let mut raw = [0u8; LIGHT_LEN];
        raw.copy_from_slice(&model.defaults.light_raw);
        Self { raw }
    }

    pub fn raw(&self) -> &[u8; LIGHT_LEN] {
        &self.raw
    }

    pub fn color(&self, mode: u8) -> Option<[u8; 3]> {
        let offset = color_offset(mode)?;
        Some([self.raw[offset], self.raw[offset + 1], self.raw[offset + 2]])
    }

    pub fn set_color(&mut self, mode: u8, rgb: [u8; 3]) {
        if let Some(offset) = color_offset(mode) {
            self.raw[offset..offset + 3].copy_from_slice(&rgb);
        }
    }

    pub fn to_write_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(LIGHT_LEN + 28);
        out.extend_from_slice(&self.raw);
        out.extend(std::iter::repeat_n(0u8, 14));
        out.extend_from_slice(&[0, 0, 90, 165, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        out
    }
}

pub fn color_offset(mode: u8) -> Option<usize> {
    Some(match mode {
        1 => 21,
        2 => 42,
        3 => 63,
        4 => 84,
        5 => 105,
        7 => 147,
        8 => 168,
        10 => 210,
        11 => 231,
        12 => 252,
        13 => 273,
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy)]
pub struct Battery {
    pub level: u8,
    pub status: u8,
}

impl Battery {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() < 2 {
            return Err(ProtocolError::BadLength(bytes.len()));
        }
        Ok(Self {
            level: bytes[0].min(100),
            status: bytes[1],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_packets_roundtrip() {
        let data: Vec<u8> = (0..PERF_LEN).map(|i| i as u8).collect();
        let packets = build_write_packets(CMD_SET_PERFORMANCE, &data, WriteMode::ExactLast);
        assert_eq!(packets.len(), 10);
        assert_eq!(packets[0][0], REPORT_ID);
        assert_eq!(packets[0][1], CMD_SET_PERFORMANCE);
        assert_eq!(packets[0][2], 10);
        assert_eq!(packets[0][3], 0);
        assert_eq!(packets[0][4], 14);
        assert_eq!(packets[9][4], 2);

        let mut assembler = ReadAssembler::new(CMD_SET_PERFORMANCE);
        for report in &packets {
            let packet = Packet::decode(&report[1..]).unwrap();
            if assembler.push(&packet) {
                break;
            }
        }
        assert!(assembler.is_complete());
        assert_eq!(assembler.data(), data);
    }

    #[test]
    fn key_layer_has_expected_layout() {
        let model = Model::load();
        let layer = KeyLayer::from_defaults(&model, 0);
        assert_eq!(layer.keys.len(), KEY_COUNT);
        assert_eq!(layer.keys[0], [0, 0, 0, 41]);
        let bytes = layer.to_write_bytes();
        assert_eq!(bytes.len(), 512);
        assert_eq!(&bytes[510..], &[90, 165]);

        let packets = build_write_packets(CMD_SET_KEYS, &bytes, WriteMode::Layer(0));
        assert_eq!(packets.len(), 37);
        assert_eq!(packets[0][4], 14);
        assert_eq!(*packets[36].last().unwrap(), {
            let mut sum = REPORT_ID as u32;
            for b in &packets[36][1..19] {
                sum += *b as u32;
            }
            sum as u8
        });
    }

    #[test]
    fn light_write_has_expected_layout() {
        let model = Model::load();
        let light = LightState::from_model_default(&model);
        assert_eq!(light.to_write_bytes().len(), 518);
        assert_eq!(light.color(1), Some([255, 0, 0]));
        let mut light = light;
        light.set_color(11, [1, 2, 3]);
        assert_eq!(light.color(11), Some([1, 2, 3]));
        assert_eq!(light.color(6), None);
    }

    #[test]
    fn performance_fields() {
        let mut perf = Performance::from_model_default(&Model::load());
        assert_eq!(perf.rate_hz(), Some(1000));
        perf.set_rate_index(2);
        assert_eq!(perf.rate_hz(), Some(500));
        perf.set_latency_on(true);
        assert!(perf.latency_on());
        perf.set_sleep_minutes(10);
        assert_eq!(perf.sleep_minutes(), 10);
        perf.set_win_lock(true);
        assert!(perf.win_lock());
        perf.set_light_mode(21);
        assert_eq!(perf.light_mode(), 21);
        assert_eq!(perf.light_type(), 1);
        let s = perf.effect_settings(11).unwrap();
        assert_eq!(s.speed, 3);
        assert!(s.multi_color);
        perf.set_effect_settings(
            11,
            EffectSettings {
                brightness: 2,
                speed: 4,
                multi_color: false,
            },
        );
        let s = perf.effect_settings(11).unwrap();
        assert_eq!(s.brightness, 2);
        assert_eq!(s.speed, 4);
        assert!(!s.multi_color);
    }

    #[test]
    fn key_layer_selector_is_at_payload_index_three() {
        let default = get_keys_request(0);
        let fn_layer = get_keys_request(1);
        let fn2_layer = get_keys_request(2);
        assert_eq!(default[4], 0x00);
        assert_eq!(fn_layer[4], 0x10);
        assert_eq!(fn2_layer[4], 0x20);
        assert_eq!(fn_layer[1], CMD_GET_KEYS);
    }

    #[test]
    fn key_layer_write_encodes_layer_in_yy() {
        let mut data = vec![0u8; 512];
        *data.last_mut().unwrap() = 1;
        let packets = build_write_packets(CMD_SET_KEYS, &data, WriteMode::Layer(1));
        assert_eq!(packets[0][4], 0x1e);
        let last = packets.last().unwrap();
        assert_eq!(last[4] >> 4, 1);
        assert_eq!(last[4] & 0x0f, 8);
    }

    #[test]
    fn bad_checksum_is_rejected() {
        let mut payload = [0u8; PACKET_PAYLOAD];
        payload[0] = 0x44;
        payload[1] = 1;
        payload[18] = 0;
        assert!(matches!(
            Packet::decode(&payload),
            Err(ProtocolError::BadChecksum)
        ));
    }
}
