use std::fs;
use std::io;
use std::path::PathBuf;

use gtk::glib::user_config_dir;

use crate::macro_table::{MacroDef, MacroType};

/// 指令的名称与节点存在本机，键盘里只存表本身。
///
/// 有线读 `0x82` 返回的是键表内容而不是指令表（真机实测），官方前端同样从不读回这张表，
/// 而是把名称记在自己的本地存储里、按键上只留槽位号。这里沿用同一套语义。
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct Saved {
    commands: Vec<SavedCommand>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct SavedCommand {
    name: String,
    #[serde(default)]
    kind: Option<MacroType>,
    chips: Vec<crate::macro_table::Chip>,
}

fn path() -> PathBuf {
    user_config_dir().join("mchose_hub_linux").join("commands.json")
}

pub fn load() -> (Vec<MacroDef>, Vec<MacroType>) {
    let Ok(text) = fs::read_to_string(path()) else {
        return (Vec::new(), Vec::new());
    };
    let Ok(saved) = serde_json::from_str::<Saved>(&text) else {
        return (Vec::new(), Vec::new());
    };
    let mut macros = Vec::new();
    let mut kinds = Vec::new();
    for item in saved.commands {
        kinds.push(item.kind.unwrap_or(MacroType::HoldLoop));
        macros.push(MacroDef {
            name: item.name,
            chips: item.chips,
        });
    }
    (macros, kinds)
}

pub fn save(macros: &[MacroDef], kinds: &[MacroType]) -> io::Result<()> {
    let target = path();
    if let Some(dir) = target.parent() {
        fs::create_dir_all(dir)?;
    }
    let saved = Saved {
        commands: macros
            .iter()
            .enumerate()
            .map(|(index, macro_def)| SavedCommand {
                name: macro_def.name.clone(),
                kind: kinds.get(index).copied(),
                chips: macro_def.chips.clone(),
            })
            .collect(),
    };
    fs::write(target, serde_json::to_string_pretty(&saved).unwrap_or_default())
}
