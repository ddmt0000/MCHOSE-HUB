use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::protocol::MACRO_LEN;

pub const MIN_NAME_CHARS: usize = 2;
pub const MAX_NAME_CHARS: usize = 15;
pub const DEFAULT_DELAY_MS: u32 = 50;
pub const MAX_DELAY_MS: u32 = 60000;
const ENTRY_LEN: usize = 4;

#[derive(Debug, Error)]
pub enum MacroError {
    #[error("名称需要 2–15 个字符")]
    BadName,
    #[error("指令总长 {0} 字节，超出键盘可保存的 512 字节")]
    TableFull(usize),
    #[error("键盘中的指令数据无法解析：{0}")]
    Corrupted(&'static str),
}

/// What a node acts on. `Key` holds a HID usage, `Mouse` a button bitmask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    Key(u8),
    Mouse(u8),
}

impl NodeKind {
    pub fn code(self) -> u8 {
        match self {
            Self::Key(usage) => usage,
            Self::Mouse(mask) => mask,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacroEvent {
    pub kind: NodeKind,
    pub down: bool,
}

/// A step of the recorded chain. A `Delay` chip is not stored on its own: like
/// the HUB it is written into the preceding event's gap field, so a leading
/// delay and a delay of zero have no effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Chip {
    Event(MacroEvent),
    Delay(u32),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacroDef {
    pub name: String,
    pub chips: Vec<Chip>,
}

impl MacroDef {
    pub fn event_count(&self) -> usize {
        self.chips
            .iter()
            .filter(|chip| matches!(chip, Chip::Event(_)))
            .count()
    }

    pub fn duration_ms(&self) -> u32 {
        self.chips
            .iter()
            .map(|chip| match chip {
                Chip::Delay(ms) => *ms,
                Chip::Event(_) => 0,
            })
            .sum()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MacroType {
    HoldLoop,
    UntilAnyKey,
    Once,
}

impl MacroType {
    pub const ALL: [Self; 3] = [Self::HoldLoop, Self::UntilAnyKey, Self::Once];

    pub fn device_byte(self) -> u8 {
        match self {
            Self::HoldLoop => 4,
            Self::UntilAnyKey => 2,
            Self::Once => 1,
        }
    }

    pub fn from_device_byte(byte: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.device_byte() == byte)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::HoldLoop => "按住时循环",
            Self::UntilAnyKey => "循环至任意键按下",
            Self::Once => "执行一次",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::HoldLoop => "按住该键期间反复执行",
            Self::UntilAnyKey => "循环执行直到按下其它键",
            Self::Once => "按下时执行一次",
        }
    }
}

pub fn macro_action(slot: usize, kind: MacroType) -> [u8; 4] {
    [3, kind.device_byte(), 1, slot.min(255) as u8]
}

pub fn macro_ref(value: [u8; 4]) -> Option<(usize, MacroType)> {
    if value[0] != 3 {
        return None;
    }
    MacroType::from_device_byte(value[1]).map(|kind| (value[3] as usize, kind))
}

/// Removing a command disables the keys that used it and shifts the higher slot
/// numbers down, so remaining references keep pointing at the right command.
pub fn remap_after_delete(values: &mut [[u8; 4]], slot: usize) -> bool {
    let mut dirty = false;
    for value in values.iter_mut() {
        if let Some((found, kind)) = macro_ref(*value) {
            if found == slot {
                *value = [0, 0, 0, 0];
                dirty = true;
            } else if found > slot {
                *value = macro_action(found - 1, kind);
                dirty = true;
            }
        }
    }
    dirty
}

fn event_bytes(event: MacroEvent, delay_ms: u32) -> [u8; ENTRY_LEN] {
    let delay = delay_ms.min(0x0007_ffff);
    let extended = match event.kind {
        NodeKind::Key(usage) => u8::from(usage >= 100),
        NodeKind::Mouse(_) => 2,
    };
    let up = u8::from(!event.down);
    [
        (up << 7) | (extended << 4) | ((delay >> 16) & 0x07) as u8,
        ((delay >> 8) & 0xff) as u8,
        (delay & 0xff) as u8,
        event.kind.code(),
    ]
}

/// Each event carries the gap of the delay chip that follows it.
fn nodes(chips: &[Chip]) -> Vec<u8> {
    let mut out = Vec::with_capacity(chips.len() * ENTRY_LEN);
    for index in 0..chips.len() {
        let Chip::Event(event) = chips[index] else {
            continue;
        };
        let delay = match chips.get(index + 1) {
            Some(Chip::Delay(ms)) => *ms,
            _ => 0,
        };
        out.extend_from_slice(&event_bytes(event, delay));
    }
    out
}

fn body(macro_def: &MacroDef) -> Result<Vec<u8>, MacroError> {
    let chars = macro_def.name.chars().count();
    if !(MIN_NAME_CHARS..=MAX_NAME_CHARS).contains(&chars) {
        return Err(MacroError::BadName);
    }
    let name = macro_def.name.as_bytes();
    let mut out = Vec::with_capacity(1 + name.len() + macro_def.chips.len() * ENTRY_LEN);
    out.push(name.len() as u8);
    out.extend_from_slice(name);
    out.extend_from_slice(&nodes(&macro_def.chips));
    Ok(out)
}

/// Directory of `(address, length)` pairs followed by each macro body, the
/// layout the HUB writes when assigning a command to a key.
pub fn encode(macros: &[MacroDef]) -> Result<Vec<u8>, MacroError> {
    let mut bodies = Vec::with_capacity(macros.len());
    for macro_def in macros {
        bodies.push(body(macro_def)?);
    }
    let dir_len = bodies.len() * ENTRY_LEN;
    let total: usize = dir_len + bodies.iter().map(Vec::len).sum::<usize>();
    if total > MACRO_LEN {
        return Err(MacroError::TableFull(total));
    }

    let mut out = vec![0u8; dir_len];
    let mut address = dir_len;
    for (entry, body) in bodies.iter().enumerate() {
        let base = entry * ENTRY_LEN;
        out[base..base + 2].copy_from_slice(&(address as u16).to_le_bytes());
        out[base + 2..base + 4].copy_from_slice(&(body.len() as u16).to_le_bytes());
        address += body.len();
    }
    for body in &bodies {
        out.extend_from_slice(body);
    }
    Ok(out)
}

pub fn decode(raw: &[u8]) -> Result<Vec<MacroDef>, MacroError> {
    if raw.len() < ENTRY_LEN || raw[..2] == [0, 0] {
        return Ok(Vec::new());
    }
    let dir_len = u16::from_le_bytes([raw[0], raw[1]]) as usize;
    if !dir_len.is_multiple_of(ENTRY_LEN) || dir_len > raw.len() {
        return Err(MacroError::Corrupted("目录起点无效"));
    }
    let count = dir_len / ENTRY_LEN;
    let mut out = Vec::with_capacity(count);
    let mut expected = dir_len;
    for entry in 0..count {
        let base = entry * ENTRY_LEN;
        let address = u16::from_le_bytes([raw[base], raw[base + 1]]) as usize;
        let len = u16::from_le_bytes([raw[base + 2], raw[base + 3]]) as usize;
        if address != expected || len < 1 || address + len > raw.len() {
            return Err(MacroError::Corrupted("目录项不连续或超出数据区"));
        }
        let name_len = raw[address] as usize;
        let Some(nodes_len) = len.checked_sub(1 + name_len) else {
            return Err(MacroError::Corrupted("名称长度超出条目"));
        };
        if !nodes_len.is_multiple_of(ENTRY_LEN) {
            return Err(MacroError::Corrupted("节点长度无效"));
        }
        let name_start = address + 1;
        let name_end = name_start + name_len;
        let name = String::from_utf8_lossy(&raw[name_start..name_end]).into_owned();
        let mut chips = Vec::with_capacity(nodes_len / ENTRY_LEN);
        let (records, _) = raw[name_end..address + len].as_chunks::<ENTRY_LEN>();
        for chunk in records {
            let delay = ((chunk[0] as u32 & 0x07) << 16) | ((chunk[1] as u32) << 8) | chunk[2] as u32;
            let kind = match (chunk[0] >> 4) & 0x07 {
                0 | 1 => NodeKind::Key(chunk[3]),
                2 => NodeKind::Mouse(chunk[3]),
                _ => return Err(MacroError::Corrupted("未知的节点类型")),
            };
            chips.push(Chip::Event(MacroEvent {
                kind,
                down: chunk[0] & 0x80 == 0,
            }));
            if delay > 0 {
                chips.push(Chip::Delay(delay));
            }
        }
        out.push(MacroDef { name, chips });
        expected = address + len;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: NodeKind, down: bool) -> Chip {
        Chip::Event(MacroEvent { kind, down })
    }

    fn sample() -> MacroDef {
        MacroDef {
            name: "Hi".to_string(),
            chips: vec![
                event(NodeKind::Key(4), true),
                Chip::Delay(50),
                event(NodeKind::Key(224), false),
                event(NodeKind::Mouse(1), true),
                Chip::Delay(1200),
            ],
        }
    }

    #[test]
    fn encodes_directory_and_nodes_like_hub() {
        let table = encode(&[sample()]).unwrap();
        // entry: address 4, length 1 + 2 + 3 * 4 = 15
        assert_eq!(&table[..4], &[4, 0, 15, 0]);
        assert_eq!(table[4], 2);
        assert_eq!(&table[5..7], b"Hi");
        assert_eq!(&table[7..11], &[0, 0, 50, 4]);
        // 抬起(bit7) + 扩展键(bit4-2 = 1) + usage 224, no following delay
        assert_eq!(&table[11..15], &[0x90, 0, 0, 224]);
        // mouse node a = 2, trailing delay 1200 = 0x4b0
        assert_eq!(&table[15..19], &[0x20, 0x04, 0xb0, 1]);
        assert_eq!(table.len(), 19);
    }

    #[test]
    fn round_trips_multiple_macros() {
        let mut second = sample();
        second.name = "长按 A".to_string();
        second.chips.truncate(3);
        let macros = vec![sample(), second];
        let table = encode(&macros).unwrap();
        assert_eq!(&table[0..4], &[8, 0, 15, 0]);
        assert_eq!(&table[4..8], &[23, 0, 17, 0]);
        assert_eq!(decode(&table).unwrap(), macros);
    }

    #[test]
    fn chips_survive_json_round_trip() {
        let text = serde_json::to_string(&sample()).unwrap();
        let back: MacroDef = serde_json::from_str(&text).unwrap();
        assert_eq!(back, sample());
    }

    #[test]
    fn empty_table_has_no_entries() {
        assert!(decode(&[0u8; MACRO_LEN]).unwrap().is_empty());
        assert!(encode(&[]).unwrap().is_empty());
    }

    #[test]
    fn leading_and_zero_delays_are_not_stored() {
        let macro_def = MacroDef {
            name: "示例".to_string(),
            chips: vec![
                Chip::Delay(500),
                event(NodeKind::Key(4), true),
                Chip::Delay(0),
                event(NodeKind::Key(4), false),
            ],
        };
        let decoded = decode(&encode(&[macro_def]).unwrap()).unwrap();
        assert_eq!(decoded[0].chips.len(), 2);
        assert_eq!(decoded[0].event_count(), 2);
    }

    #[test]
    fn rejects_oversized_and_short_names() {
        let mut big = sample();
        big.name = "名称".repeat(200);
        assert!(matches!(encode(&[big]).unwrap_err(), MacroError::BadName));

        let mut many: Vec<MacroDef> = (0..40)
            .map(|index| {
                let mut macro_def = sample();
                macro_def.name = format!("指令{index}");
                macro_def
            })
            .collect();
        assert!(matches!(
            encode(&many).unwrap_err(),
            MacroError::TableFull(_)
        ));
        many.truncate(20);
        assert!(encode(&many).is_ok());

        let mut tiny = sample();
        tiny.name = "A".to_string();
        assert!(matches!(encode(&[tiny]).unwrap_err(), MacroError::BadName));
    }

    #[test]
    fn rejects_inconsistent_directory() {
        let table = encode(&[sample()]).unwrap();
        let mut broken = table.clone();
        broken[0] = 12;
        assert!(matches!(
            decode(&broken).unwrap_err(),
            MacroError::Corrupted(_)
        ));
        let mut truncated = table;
        truncated.truncate(12);
        assert!(matches!(
            decode(&truncated).unwrap_err(),
            MacroError::Corrupted(_)
        ));
    }

    #[test]
    fn action_encodes_slot_and_type() {
        assert_eq!(macro_action(0, MacroType::HoldLoop), [3, 4, 1, 0]);
        assert_eq!(macro_action(2, MacroType::Once), [3, 1, 1, 2]);
        assert_eq!(macro_ref([3, 2, 1, 5]), Some((5, MacroType::UntilAnyKey)));
        assert_eq!(macro_ref([0, 0, 0, 41]), None);
        assert_eq!(macro_ref([3, 9, 1, 0]), None);
    }

    #[test]
    fn deleting_a_slot_disables_its_keys_and_shifts_higher_slots() {
        let mut values = vec![
            [0, 0, 0, 41],
            macro_action(0, MacroType::HoldLoop),
            macro_action(2, MacroType::Once),
            macro_action(1, MacroType::UntilAnyKey),
        ];
        assert!(remap_after_delete(&mut values, 0));
        assert_eq!(values[0], [0, 0, 0, 41]);
        assert_eq!(values[1], [0, 0, 0, 0]);
        assert_eq!(values[2], macro_action(1, MacroType::Once));
        assert_eq!(values[3], macro_action(0, MacroType::UntilAnyKey));

        let mut shifted = vec![macro_action(2, MacroType::Once)];
        assert!(remap_after_delete(&mut shifted, 0));
        assert_eq!(shifted[0], macro_action(1, MacroType::Once));

        let mut untouched = vec![[0, 0, 0, 41], macro_action(3, MacroType::Once)];
        assert!(!remap_after_delete(&mut untouched, 5));
        assert_eq!(untouched[1], macro_action(3, MacroType::Once));
    }

    #[test]
    fn reports_chain_length() {
        let macro_def = sample();
        assert_eq!(macro_def.event_count(), 3);
        assert_eq!(macro_def.duration_ms(), 1250);
    }
}
