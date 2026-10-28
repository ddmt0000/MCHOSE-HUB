use std::fmt::Write as _;

use gettextrs::gettext;

/// 登记待翻译文案但不在此处取译文：label() 这类返回值同时给日志和命令行用，
/// 由显示方调 gettext() 取译文。清单工具按 `msgid:1` 关键字收集这些串。
pub const fn msgid(text: &str) -> &str {
    text
}

/// 取运行时文案的译文。`format!` 只接受字面量格式串，翻译结果没法当格式串用，
/// 所以带变量的文案一律走这里：模板里按顺序写 `{}`，译文决定这些 `{}` 的位置。
pub fn tf(template: &str, args: &[&dyn std::fmt::Display]) -> String {
    let text = gettext(template);
    let mut parts = text.split("{}");
    let mut out = parts.next().unwrap_or_default().to_string();
    for (index, part) in parts.enumerate() {
        match args.get(index) {
            Some(value) => {
                let _ = write!(out, "{value}");
            }
            // 译文多了占位符时原样留着，别把后面的内容整体吞掉。
            None => out.push_str("{}"),
        }
        out.push_str(part);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_placeholders_in_order() {
        assert_eq!(tf("第 {} 层", &[&2]), "第 2 层");
        assert_eq!(
            tf("{} · {} 个节点", &[&"按住时循环", &7]),
            "按住时循环 · 7 个节点"
        );
    }

    #[test]
    fn keeps_plain_text_and_extra_braces() {
        assert_eq!(tf("正在读取设备…", &[]), "正在读取设备…");
        assert_eq!(tf("{} 与 {}", &[&"a"]), "a 与 {}");
        assert_eq!(tf("已复制 {} 规则", &[&"udev"]), "已复制 udev 规则");
    }
}
