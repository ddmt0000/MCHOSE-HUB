use gettextrs::gettext;
use serde::Deserialize;

use crate::i18n::tf;
use crate::macro_table::NodeKind;

const MODEL_JSON: &str = include_str!("../data/models/k99_v2.json");

/// Modifier actions store a bit mask in byte 1; macro nodes store the HID usage.
const MODIFIER_USAGES: &[(u8, u8)] = &[
    (1, 224),
    (2, 225),
    (4, 226),
    (8, 227),
    (16, 228),
    (32, 229),
    (64, 230),
    (128, 231),
];

/// The keyboard action byte and the macro node bitmask differ for mouse keys.
const MOUSE_NODE_MASKS: &[(u8, u8)] = &[(1, 1), (2, 2), (3, 4), (4, 8), (5, 16)];

/// Convert a key-table action into the node form used inside a macro.
pub fn action_to_node(value: [u8; 4]) -> Option<NodeKind> {
    let [family, second, third, usage] = value;
    match family {
        0 if third == 0 => {
            if second == 0 && usage > 0 {
                Some(NodeKind::Key(usage))
            } else {
                MODIFIER_USAGES
                    .iter()
                    .find(|(mask, _)| *mask == second)
                    .map(|(_, usage)| NodeKind::Key(*usage))
            }
        }
        1 if third == 1 => MOUSE_NODE_MASKS
            .iter()
            .find(|(action, _)| *action == second)
            .map(|(_, mask)| NodeKind::Mouse(*mask)),
        _ => None,
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Receiver {
    pub vendor_id: u16,
    pub product_id: u16,
    pub product_name: String,
    pub usage_pages: Vec<u16>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReportInfo {
    pub wireless_id: u8,
    pub wired_id: u8,
    pub chunk: usize,
    pub header: usize,
    pub payload: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PhysicalKey {
    pub name: String,
    pub label: String,
    pub matrix: usize,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    #[serde(rename = "default")]
    pub default: [u8; 4],
    #[serde(rename = "fn")]
    pub fn_layer: [u8; 4],
    pub fn2: [u8; 4],
}

impl PhysicalKey {
    pub fn label_parts(&self) -> (&str, &str) {
        match self.label.split_once(' ') {
            Some((a, b)) if !a.is_empty() && !b.is_empty() => (a, b),
            _ => (self.label.as_str(), ""),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ActionItem {
    pub label: String,
    pub value: [u8; 4],
}

#[derive(Debug, Clone, Deserialize)]
pub struct Actions {
    pub basic: Vec<ActionItem>,
    pub mouse: Vec<ActionItem>,
    pub media: Vec<ActionItem>,
    pub system: Vec<ActionItem>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Effect {
    pub mode: u8,
    pub id: String,
    pub name: String,
    pub brightness: bool,
    pub speed: bool,
    pub color: bool,
    pub multi: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Defaults {
    pub performance_raw: Vec<u8>,
    pub light_raw: Vec<u8>,
    pub key_layers: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Bounds {
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub receiver: Receiver,
    pub report: ReportInfo,
    pub matrix: Vec<String>,
    pub keys: Vec<PhysicalKey>,
    pub actions: Actions,
    pub defaults: Defaults,
    pub effects: Vec<Effect>,
    pub polling_rates: Vec<u16>,
    pub bounds: Bounds,
}

impl Model {
    pub fn load() -> Self {
        let model: Model =
            serde_json::from_str(MODEL_JSON).expect("embedded model data is invalid");
        model.validate();
        model
    }

    fn validate(&self) {
        assert_eq!(self.keys.len(), 99, "K99 V2 layout must have 99 keys");
        assert_eq!(self.matrix.len(), 113, "key matrix must have 113 slots");
        assert_eq!(self.defaults.performance_raw.len(), 128);
        assert_eq!(self.defaults.light_raw.len(), 490);
        assert_eq!(self.defaults.key_layers.len(), 3);
        for layer in &self.defaults.key_layers {
            assert_eq!(layer.len(), 113 * 4);
        }
        for key in &self.keys {
            assert!(key.matrix < self.matrix.len(), "matrix index out of range");
        }
    }

    pub fn key_for_matrix(&self, matrix: usize) -> Option<&PhysicalKey> {
        self.keys.iter().find(|k| k.matrix == matrix)
    }

    /// Detect the software Mac layout (Left_Win/Left_Alt swapped) for the
    /// default layer. Returns None when the keys were customised.
    pub fn mac_layout_state(&self, keys: &[[u8; 4]]) -> Option<bool> {
        let win = self.key_for_matrix(11)?.default;
        let alt = self.key_for_matrix(17)?.default;
        let current_win = *keys.get(11)?;
        let current_alt = *keys.get(17)?;
        if current_win == win && current_alt == alt {
            Some(false)
        } else if current_win == alt && current_alt == win {
            Some(true)
        } else {
            None
        }
    }

    pub fn effect(&self, mode: u8) -> Option<&Effect> {
        self.effects.iter().find(|e| e.mode == mode)
    }

    pub fn action_label(&self, value: [u8; 4]) -> String {
        if value == [0, 0, 0, 0] {
            return gettext("禁用");
        }
        for group in [
            &self.actions.basic,
            &self.actions.mouse,
            &self.actions.media,
            &self.actions.system,
        ] {
            if let Some(item) = group.iter().find(|a| a.value == value) {
                return gettext(&item.label);
            }
        }
        match value[0] {
            13 => {
                if value[1] == 0 {
                    gettext("Fn 层")
                } else {
                    gettext("Fn2 层")
                }
            }
            3 => gettext("宏"),
            7 => gettext("Fn 功能"),
            8 => gettext("系统功能"),
            _ => format!(
                "{} {:02X} {:02X} {:02X} {:02X}",
                gettext("自定义"),
                value[0],
                value[1],
                value[2],
                value[3]
            ),
        }
    }

    fn node_label_in(&self, kind: NodeKind) -> Option<&str> {
        let groups: &[&Vec<ActionItem>] = match kind {
            NodeKind::Key(_) => &[&self.actions.basic],
            NodeKind::Mouse(_) => &[&self.actions.mouse],
        };
        groups
            .iter()
            .flat_map(|group| group.iter())
            .find(|action| action_to_node(action.value) == Some(kind))
            .map(|action| action.label.as_str())
    }

    pub fn node_label(&self, kind: NodeKind) -> String {
        self.node_label_in(kind)
            .map(gettext)
            .unwrap_or_else(|| match kind {
                NodeKind::Key(usage) => tf("按键 {}", &[&usage]),
                NodeKind::Mouse(mask) => tf("鼠标键 {}", &[&mask]),
            })
    }

    /// Key-table actions usable as macro nodes.
    pub fn macro_choices(&self, mouse: bool) -> Vec<(String, NodeKind)> {
        let group = if mouse {
            &self.actions.mouse
        } else {
            &self.actions.basic
        };
        group
            .iter()
            .filter_map(|action| Some((action.label.clone(), action_to_node(action.value)?)))
            .collect()
    }

    pub fn default_layer(&self, layer: usize) -> Vec<[u8; 4]> {
        let raw = &self.defaults.key_layers[layer];
        raw.as_chunks::<4>()
            .0
            .iter()
            .map(|c| [c[0], c[1], c[2], c[3]])
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_is_consistent() {
        let model = Model::load();
        assert_eq!(model.name, "K99 V2");
        assert_eq!(model.keys.len(), 99);
        assert!(model.key_for_matrix(0).is_some());
        assert_eq!(model.default_layer(0).len(), 113);
        assert_eq!(model.default_layer(1).len(), 113);
        assert_eq!(model.default_layer(2).len(), 113);
    }

    #[test]
    fn label_parts() {
        let model = Model::load();
        let esc = model.keys.iter().find(|k| k.name == "Esc").unwrap();
        assert_eq!(esc.label_parts(), ("Esc", ""));
        let one = model.keys.iter().find(|k| k.name == "one").unwrap();
        assert_eq!(one.label_parts(), ("1", "!"));
    }

    #[test]
    fn mac_layout_detection() {
        let model = Model::load();
        let mut keys = model.default_layer(0);
        assert_eq!(model.mac_layout_state(&keys), Some(false));
        keys.swap(11, 17);
        assert_eq!(model.mac_layout_state(&keys), Some(true));
        keys[11] = [9, 9, 9, 9];
        assert_eq!(model.mac_layout_state(&keys), None);
    }

    #[test]
    fn action_labels() {
        let model = Model::load();
        assert_eq!(model.action_label([0, 0, 0, 0]), "禁用");
        assert_eq!(model.action_label([0, 0, 0, 41]), "Esc");
        assert_eq!(model.action_label([1, 1, 1, 0]), "左键");
        assert_eq!(model.action_label([2, 0, 0, 205]), "播放/暂停");
        assert_eq!(model.action_label([13, 1, 0, 0]), "Fn2 层");
    }

    #[test]
    fn macro_nodes_reuse_the_action_list() {
        assert_eq!(action_to_node([0, 0, 0, 4]), Some(NodeKind::Key(4)));
        assert_eq!(action_to_node([0, 0, 0, 101]), Some(NodeKind::Key(101)));
        assert_eq!(action_to_node([0, 2, 0, 0]), Some(NodeKind::Key(225)));
        assert_eq!(action_to_node([0, 16, 0, 0]), Some(NodeKind::Key(228)));
        assert_eq!(action_to_node([1, 1, 1, 0]), Some(NodeKind::Mouse(1)));
        assert_eq!(action_to_node([1, 5, 1, 0]), Some(NodeKind::Mouse(16)));
        assert_eq!(action_to_node([1, 2, 1, 0]), Some(NodeKind::Mouse(2)));
        // 滚轮与组合快捷键不能作为宏节点
        assert_eq!(action_to_node([1, 8, 1, 0]), None);
        assert_eq!(action_to_node([0, 1, 45, 0]), None);
        assert_eq!(action_to_node([2, 0, 0, 205]), None);

        let model = Model::load();
        assert_eq!(model.node_label(NodeKind::Key(4)), "A");
        assert_eq!(model.node_label(NodeKind::Key(225)), "Left Shift");
        assert_eq!(model.node_label(NodeKind::Mouse(4)), "中键");
        assert_eq!(model.node_label(NodeKind::Key(200)), "按键 200");
        assert_eq!(model.macro_choices(false).len(), 104);
        assert_eq!(model.macro_choices(true).len(), 5);
    }
}
