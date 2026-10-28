use std::collections::BTreeMap;

use thiserror::Error;

use crate::model::Model;

pub const REPORT_ID: u8 = 0x13;
pub const CHUNK: usize = 14;
pub const PACKET_PAYLOAD: usize = 19;
pub const KEY_COUNT: usize = 113;
pub const PERF_LEN: usize = 128;
pub const LIGHT_LEN: usize = 490;
pub const DIY_LEN: usize = 378;
pub const DIY_PLANES: usize = 126;
pub const MACRO_LEN: usize = 512;

pub const CMD_GET_KEYS: u8 = 0x41;
pub const CMD_GET_DIY: u8 = 0x42;
pub const CMD_GET_MACRO: u8 = 0x43;
pub const CMD_GET_PERFORMANCE: u8 = 0x44;
pub const CMD_GET_LIGHT: u8 = 0x49;
pub const CMD_GET_BATTERY: u8 = 0x4a;
pub const CMD_SET_KEYS: u8 = 0x01;
pub const CMD_SET_DIY: u8 = 0x02;
pub const CMD_SET_MACRO: u8 = 0x03;
pub const CMD_SET_PERFORMANCE: u8 = 0x04;
pub const CMD_SET_LIGHT: u8 = 0x09;

pub const WIRED_REPORT_LEN: usize = 519;
pub const WIRED_BUFFER_LEN: usize = WIRED_REPORT_LEN + 1;
pub const WIRED_HEADER_LEN: usize = 7;
pub const WIRED_KEYS_READ_LEN: usize = 504;
pub const WIRED_CMD_GET_KEYS: u8 = 0x83;
pub const WIRED_CMD_GET_MACRO: u8 = 0x82;
pub const WIRED_CMD_GET_DIY: u8 = 0x86;
pub const WIRED_CMD_GET_PERFORMANCE: u8 = 0x84;
pub const WIRED_CMD_GET_BATTERY: u8 = 0x87;
pub const WIRED_CMD_GET_LIGHT: u8 = 0x8a;
pub const WIRED_CMD_SET_KEYS: u8 = 0x03;
pub const WIRED_CMD_SET_DIY: u8 = 0x06;
pub const WIRED_CMD_SET_MACRO: u8 = 0x05;
pub const WIRED_CMD_SET_PERFORMANCE: u8 = 0x04;
pub const WIRED_CMD_SET_LIGHT: u8 = 0x10;
pub const WIRED_SPACE_PERFORMANCE: [u8; 6] = [0, 0, 1, 0, 0x80, 0];
pub const WIRED_SPACE_LIGHT: [u8; 6] = [0, 0, 1, 0, 0x00, 0x02];
pub const WIRED_SPACE_BATTERY: [u8; 6] = [0, 0, 1, 0, 0x02, 0];
pub const WIRED_SPACE_DIY_READ: [u8; 6] = [0, 0, 1, 0, 0x7a, 0x01];
pub const WIRED_SPACE_DIY_WRITE: [u8; 6] = [0, 0, 1, 0, 0x80, 0x01];
/// Declaring the full 512 bytes still returns the key matrix on real hardware,
/// not a command table, so the command list lives in local storage and this
/// read is only used by `--probe`. Official never calls it either: its parser
/// declares a single 4-byte field.
pub const WIRED_SPACE_MACRO_READ: [u8; 6] = [0, 0, 1, 0, 0x00, 0x02];

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("报文长度不足：期望 {expected}，实际 {actual}")]
    ShortPacket { expected: usize, actual: usize },
    #[error("校验和不匹配")]
    BadChecksum,
    #[error("数据长度不对：{0}")]
    BadLength(usize),
    #[error("设备响应命令不匹配：期望 {expected:#04x}，实际 {actual:#04x}")]
    UnexpectedCommand { expected: u8, actual: u8 },
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

pub fn get_diy_request() -> [u8; 20] {
    read_request(CMD_GET_DIY, 0)
}

/// Unlike the other reads, the HUB's macro read carries a `10 03 02` descriptor
/// in the data area; sending zeros instead gets no response.
pub fn get_macro_request() -> [u8; 20] {
    let mut payload = [0u8; PACKET_PAYLOAD];
    payload[0] = CMD_GET_MACRO;
    payload[1] = 0x01;
    payload[4] = 0x10;
    payload[5] = 0x03;
    payload[6] = 0x02;
    let mut report = [0u8; 20];
    report[0] = REPORT_ID;
    report[1..].copy_from_slice(&payload);
    report
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

/// Per-key colours for lighting mode 21: three 126 byte planes (red, green,
/// blue) indexed by key matrix slot. All zero means the key is unlit.
#[derive(Debug, Clone)]
pub struct DiyLight {
    raw: [u8; DIY_LEN],
}

impl DiyLight {
    pub fn empty() -> Self {
        Self {
            raw: [0u8; DIY_LEN],
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() < DIY_LEN {
            return Err(ProtocolError::BadLength(bytes.len()));
        }
        let mut raw = [0u8; DIY_LEN];
        raw.copy_from_slice(&bytes[..DIY_LEN]);
        Ok(Self { raw })
    }

    pub fn raw(&self) -> &[u8; DIY_LEN] {
        &self.raw
    }

    pub fn color(&self, matrix: usize) -> Option<[u8; 3]> {
        if matrix >= DIY_PLANES {
            return None;
        }
        let rgb = [
            self.raw[matrix],
            self.raw[DIY_PLANES + matrix],
            self.raw[2 * DIY_PLANES + matrix],
        ];
        (rgb != [0, 0, 0]).then_some(rgb)
    }

    pub fn set_color(&mut self, matrix: usize, rgb: [u8; 3]) {
        if matrix >= DIY_PLANES {
            return;
        }
        self.raw[matrix] = rgb[0];
        self.raw[DIY_PLANES + matrix] = rgb[1];
        self.raw[2 * DIY_PLANES + matrix] = rgb[2];
    }

    /// Clicking a key paints it with the palette colour; clicking a key that
    /// already has exactly that colour turns it off, like the HUB.
    pub fn next_color(current: Option<[u8; 3]>, palette: [u8; 3]) -> [u8; 3] {
        if current == Some(palette) {
            [0, 0, 0]
        } else {
            palette
        }
    }
}

pub fn wired_diy_request() -> [u8; WIRED_REPORT_LEN] {
    wired_request(WIRED_CMD_GET_DIY, WIRED_SPACE_DIY_READ)
}

pub fn wired_macro_request() -> [u8; WIRED_REPORT_LEN] {
    wired_request(WIRED_CMD_GET_MACRO, WIRED_SPACE_MACRO_READ)
}

pub fn wired_diy_write(diy: &DiyLight) -> [u8; WIRED_REPORT_LEN] {
    let mut request = wired_request(WIRED_CMD_SET_DIY, WIRED_SPACE_DIY_WRITE);
    request[WIRED_HEADER_LEN..WIRED_HEADER_LEN + DIY_LEN].copy_from_slice(diy.raw());
    request
}

/// The wired macro write splits the table into 512 byte blocks and names each
/// block in `space0`, so a table that does not fit is rejected instead of
/// silently truncated.
pub fn wired_macro_writes(table: &[u8]) -> Result<Vec<[u8; WIRED_REPORT_LEN]>, ProtocolError> {
    let blocks = table.len().div_ceil(MACRO_LEN);
    if blocks > 1 {
        return Err(ProtocolError::BadLength(table.len()));
    }
    let len = u16::try_from(table.len()).map_err(|_| ProtocolError::BadLength(table.len()))?;
    let space0 = [
        0,
        0,
        blocks.max(1) as u8,
        0,
        (len & 0xff) as u8,
        (len >> 8) as u8,
    ];
    let mut request = wired_request(WIRED_CMD_SET_MACRO, space0);
    request[WIRED_HEADER_LEN..WIRED_HEADER_LEN + table.len()].copy_from_slice(table);
    Ok(vec![request])
}

pub fn wired_keys_space(layer: u8) -> [u8; 6] {
    [layer, 0, 1, 0, 0xf8, 0x01]
}

pub fn wired_request(command: u8, space0: [u8; 6]) -> [u8; WIRED_REPORT_LEN] {
    let mut request = [0u8; WIRED_REPORT_LEN];
    request[0] = command;
    request[1..WIRED_HEADER_LEN].copy_from_slice(&space0);
    request
}

pub fn wired_performance_request() -> [u8; WIRED_REPORT_LEN] {
    wired_request(WIRED_CMD_GET_PERFORMANCE, WIRED_SPACE_PERFORMANCE)
}

pub fn wired_keys_request(layer: u8) -> [u8; WIRED_REPORT_LEN] {
    wired_request(WIRED_CMD_GET_KEYS, wired_keys_space(layer))
}

pub fn wired_light_request() -> [u8; WIRED_REPORT_LEN] {
    wired_request(WIRED_CMD_GET_LIGHT, WIRED_SPACE_LIGHT)
}

pub fn wired_battery_request() -> [u8; WIRED_REPORT_LEN] {
    wired_request(WIRED_CMD_GET_BATTERY, WIRED_SPACE_BATTERY)
}

pub fn wired_performance_write(performance: &Performance) -> [u8; WIRED_REPORT_LEN] {
    let mut request = wired_request(WIRED_CMD_SET_PERFORMANCE, WIRED_SPACE_PERFORMANCE);
    request[WIRED_HEADER_LEN..WIRED_HEADER_LEN + PERF_LEN].copy_from_slice(performance.raw());
    request
}

pub fn wired_keys_write(layer: u8, keys: &KeyLayer) -> [u8; WIRED_REPORT_LEN] {
    let mut request = wired_request(WIRED_CMD_SET_KEYS, wired_keys_space(layer));
    let bytes = keys.to_write_bytes();
    request[WIRED_HEADER_LEN..WIRED_HEADER_LEN + bytes.len()].copy_from_slice(&bytes);
    request
}

pub fn wired_light_write(light: &LightState) -> [u8; WIRED_REPORT_LEN] {
    let mut request = wired_request(WIRED_CMD_SET_LIGHT, WIRED_SPACE_LIGHT);
    request[WIRED_HEADER_LEN..WIRED_HEADER_LEN + LIGHT_LEN].copy_from_slice(light.raw());
    let marker = WIRED_HEADER_LEN + LIGHT_LEN + 16;
    request[marker] = 0x5a;
    request[marker + 1] = 0xa5;
    request
}

pub fn wired_response(buffer: &[u8], command: u8) -> Result<&[u8], ProtocolError> {
    if buffer.len() < WIRED_HEADER_LEN {
        return Err(ProtocolError::ShortPacket {
            expected: WIRED_HEADER_LEN,
            actual: buffer.len(),
        });
    }
    if buffer[0] != command {
        return Err(ProtocolError::UnexpectedCommand {
            expected: command,
            actual: buffer[0],
        });
    }
    Ok(&buffer[WIRED_HEADER_LEN..])
}

#[derive(Debug, Clone, Copy)]
pub struct Battery {
    pub level: u8,
    pub status: u8,
}

impl Battery {
    /// 低半字节：官方前端按 `chargeStatus === 1` 判“充电中”。
    pub fn charging(&self) -> bool {
        self.status & 0x0f == 1
    }

    /// 高半字节：`fullStatus`，电池充满后固件停充时置位。
    pub fn fully_charged(&self) -> bool {
        self.status & 0xf0 != 0
    }

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

    #[test]
    fn wired_read_requests_have_expected_header() {
        let perf = wired_performance_request();
        assert_eq!(perf.len(), WIRED_REPORT_LEN);
        assert_eq!(&perf[..7], &[0x84, 0, 0, 1, 0, 0x80, 0]);

        let keys = wired_keys_request(1);
        assert_eq!(&keys[..7], &[0x83, 1, 0, 1, 0, 0xf8, 0x01]);

        let light = wired_light_request();
        assert_eq!(&light[..7], &[0x8a, 0, 0, 1, 0, 0x00, 0x02]);

        let battery = wired_battery_request();
        assert_eq!(&battery[..7], &[0x87, 0, 0, 1, 0, 0x02, 0]);
    }

    #[test]
    fn wired_writes_match_hub_layout() {
        let model = Model::load();

        let performance = Performance::from_model_default(&model);
        let request = wired_performance_write(&performance);
        assert_eq!(request.len(), WIRED_REPORT_LEN);
        assert_eq!(&request[..7], &[0x04, 0, 0, 1, 0, 0x80, 0]);
        assert_eq!(&request[7..7 + PERF_LEN], performance.raw());
        assert!(request[7 + PERF_LEN..].iter().all(|b| *b == 0));

        let layer = KeyLayer::from_defaults(&model, 1);
        let request = wired_keys_write(1, &layer);
        assert_eq!(request.len(), WIRED_REPORT_LEN);
        assert_eq!(&request[..7], &[0x03, 1, 0, 1, 0, 0xf8, 0x01]);
        let bytes = layer.to_write_bytes();
        assert_eq!(&request[7..7 + 452], &bytes[..452]);
        assert_eq!(&request[WIRED_REPORT_LEN - 2..], &[0x5a, 0xa5]);
        assert!(request[7 + 452..WIRED_REPORT_LEN - 2]
            .iter()
            .all(|b| *b == 0));

        let light = LightState::from_model_default(&model);
        let request = wired_light_write(&light);
        assert_eq!(request.len(), WIRED_REPORT_LEN);
        assert_eq!(&request[..7], &[0x10, 0, 0, 1, 0, 0x00, 0x02]);
        assert_eq!(&request[7..7 + LIGHT_LEN], light.raw());
        assert_eq!(&request[513..515], &[0x5a, 0xa5]);
        assert!(request[497..513].iter().all(|b| *b == 0));
        assert!(request[515..].iter().all(|b| *b == 0));
    }

    #[test]
    fn wired_response_checks_command() {
        let mut buffer = vec![0u8; WIRED_REPORT_LEN];
        buffer[0] = WIRED_CMD_GET_BATTERY;
        buffer[1..7].copy_from_slice(&WIRED_SPACE_BATTERY);
        buffer[7] = 64;
        buffer[8] = 0x10;
        let data = wired_response(&buffer, WIRED_CMD_GET_BATTERY).unwrap();
        assert_eq!(&data[..2], &[64, 0x10]);
        assert!(matches!(
            wired_response(&buffer, WIRED_CMD_GET_PERFORMANCE),
            Err(ProtocolError::UnexpectedCommand { .. })
        ));
        assert!(matches!(
            wired_response(&buffer[..6], WIRED_CMD_GET_BATTERY),
            Err(ProtocolError::ShortPacket { .. })
        ));
    }

    #[test]
    fn battery_uses_low_nibble_for_charging() {
        // 真机三种取值：无线 50% = 01、有线 100% = 10、有线 100% = 11
        let charging = Battery::from_bytes(&[50, 0x01]).unwrap();
        assert!(charging.charging() && !charging.fully_charged());
        let topped = Battery::from_bytes(&[100, 0x10]).unwrap();
        assert!(!topped.charging() && topped.fully_charged());
        let plugged = Battery::from_bytes(&[100, 0x11]).unwrap();
        assert!(plugged.charging() && plugged.fully_charged());
        let idle = Battery::from_bytes(&[80, 0x00]).unwrap();
        assert!(!idle.charging() && !idle.fully_charged());
    }

    #[test]
    fn macro_read_request_carries_descriptor() {
        let read = get_macro_request();
        assert_eq!(read[0], REPORT_ID);
        assert_eq!(&read[1..4], &[CMD_GET_MACRO, 0x01, 0x00]);
        assert_eq!(&read[5..8], &[0x10, 0x03, 0x02]);
        assert_eq!(get_diy_request()[1], CMD_GET_DIY);
    }

    #[test]
    fn diy_table_uses_three_planes() {
        let mut diy = DiyLight::empty();
        assert_eq!(diy.color(0), None);
        diy.set_color(0, [255, 0, 0]);
        diy.set_color(95, [0, 0, 255]);
        assert_eq!(diy.color(0), Some([255, 0, 0]));
        assert_eq!(diy.raw()[0], 255);
        assert_eq!(diy.raw()[DIY_PLANES], 0);
        assert_eq!(diy.raw()[2 * DIY_PLANES + 95], 255);
        assert_eq!(DiyLight::from_bytes(diy.raw()).unwrap().color(95), Some([0, 0, 255]));
        assert_eq!(diy.color(DIY_PLANES), None);
    }

    #[test]
    fn diy_click_paints_then_clears() {
        let red = [255, 0, 0];
        assert_eq!(DiyLight::next_color(None, red), red);
        assert_eq!(DiyLight::next_color(Some(red), red), [0, 0, 0]);
        // 与官方一致：已点亮成别的颜色时再点改成当前调色板色，而不是熄灭。
        assert_eq!(DiyLight::next_color(Some([0, 0, 255]), red), red);
    }

    #[test]
    fn diy_write_is_27_full_packets() {
        let packets = build_write_packets(CMD_SET_DIY, DiyLight::empty().raw(), WriteMode::ExactLast);
        assert_eq!(packets.len(), 27);
        assert!(packets.iter().all(|p| p[2] == 27));
        assert!(packets.iter().all(|p| p[4] == 14));
    }

    #[test]
    fn wired_diy_and_macro_frames_match_hub() {
        let read = wired_diy_request();
        assert_eq!(&read[..7], &[0x86, 0, 0, 1, 0, 0x7a, 0x01]);

        let mut diy = DiyLight::empty();
        diy.set_color(0, [1, 2, 3]);
        let write = wired_diy_write(&diy);
        assert_eq!(&write[..7], &[0x06, 0, 0, 1, 0, 0x80, 0x01]);
        assert_eq!(write[7], 1);
        assert_eq!(&write[7 + DIY_LEN..], &[0u8; WIRED_REPORT_LEN - 7 - DIY_LEN]);

        let macro_write = wired_macro_writes(&[7u8; 40]).unwrap();
        assert_eq!(macro_write.len(), 1);
        assert_eq!(&macro_write[0][..7], &[0x05, 0, 0, 1, 0, 40, 0]);
        assert_eq!(macro_write[0][7], 7);
        assert!(wired_macro_writes(&[0u8; MACRO_LEN + 1]).is_err());
    }
}
