use gtk::glib;

use crate::device::link::{discover, open_detected, Presence};
use crate::model::Model;

pub fn run(write: bool) -> glib::ExitCode {
    match run_inner(write) {
        Ok(()) => glib::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("probe failed: {e}");
            glib::ExitCode::FAILURE
        }
    }
}

fn describe(presence: &Presence) -> String {
    match presence {
        Presence::Available(info) => format!("{} ({})", info.product_name, info.hidraw),
        Presence::Denied(path) => format!("no permission ({})", path.display()),
        Presence::Absent => "absent".to_string(),
    }
}

fn run_inner(write: bool) -> Result<(), Box<dyn std::error::Error>> {    let model = Model::load();
    let mut discovery = discover(&model);
    // 诊断用：强制只走某一条链路，比较两种传输的指令表读取差异。
    match std::env::var("MCHOSE_HUB_TRANSPORT").as_deref() {
        Ok("wireless") => discovery.wired = crate::device::link::Presence::Absent,
        Ok("wired") => discovery.wireless = crate::device::link::Presence::Absent,
        _ => {}
    }
    println!("detected wired: {}", describe(&discovery.wired));
    println!("detected wireless: {}", describe(&discovery.wireless));
    let mut link = open_detected(&discovery, &model)?;
    let info = link.info().clone();
    println!("device: {} ({})", info.product_name, info.hidraw);
    println!("transport: {}", info.transport.label());
    println!("id: {:04x}:{:04x}", info.vendor_id, info.product_id);

    let battery = link.read_battery()?;
    println!("battery: {}% status={:#04x}", battery.level, battery.status);

    let performance = link.read_performance()?;
    println!(
        "performance: rate={:?}Hz latency_on={} sleep={}min win_lock={} mac={} light_mode={} light_type={}",
        performance.rate_hz(),
        performance.latency_on(),
        performance.sleep_minutes(),
        performance.win_lock(),
        performance.mac_mode(),
        performance.light_mode(),
        performance.light_type()
    );
    for (mode, _) in crate::protocol::EFFECT_OFFSETS {
        if let Some(s) = performance.effect_settings(*mode) {
            println!(
                "  effect {mode:2}: brightness={} speed={} multi={}",
                s.brightness, s.speed, s.multi_color
            );
        }
    }

    let light = link.read_light()?;
    for mode in [1u8, 2, 3, 4, 5, 7, 8, 10, 11, 12, 13] {
        if let Some(c) = light.color(mode) {
            println!("  color {mode:2}: #{:02X}{:02X}{:02X}", c[0], c[1], c[2]);
        }
    }

    let diy = link.read_diy()?;
    let lit: Vec<(usize, [u8; 3])> = (0..crate::protocol::DIY_PLANES)
        .filter_map(|index| diy.color(index).map(|color| (index, color)))
        .collect();
    println!("diy: {} lit keys", lit.len());
    for (index, color) in &lit {
        let name = model.matrix.get(*index).map_or("?", String::as_str);
        println!(
            "  diy {index:3} {name}: #{:02X}{:02X}{:02X}",
            color[0], color[1], color[2]
        );
    }

    let macros = match link.read_macros() {
        Ok(macros) => macros,
        Err(e) => {
            println!("macros: 读取或解析失败（{e}）");
            Vec::new()
        }
    };
    println!("macros: {} slots", macros.len());
    for (slot, macro_def) in macros.iter().enumerate() {
        println!(
            "  slot {slot}: {} · {} 节点 · 一轮 {}ms",
            macro_def.name,
            macro_def.event_count(),
            macro_def.duration_ms()
        );
    }

    let mut layers = Vec::new();
    println!(
        "perf raw macSwitch[27]={:#04x} winKeySwitch[15]={:#04x}",
        performance.raw()[27],
        performance.raw()[15]
    );
    for layer in 0..3u8 {
        let keys = link.read_keys(layer)?;
        let changed = keys.keys.iter().filter(|k| **k != [0, 0, 0, 0]).count();
        println!(
            "layer {layer}: {} keys, first={:?}, non-empty={changed}",
            keys.keys.len(),
            keys.keys.first()
        );
        if layer == 0 {
            println!(
                "layer0 Left_Win[11]={:?} Left_Alt[17]={:?} Right_Alt[91]={:?}",
                keys.keys.get(11),
                keys.keys.get(17),
                keys.keys.get(91)
            );
        }
        layers.push(keys);
    }

    if write {
        println!("writing settings back unchanged...");
        link.write_performance(&performance)?;
        println!("performance write verified");
        for (layer, keys) in layers.iter().enumerate() {
            link.write_keys(layer as u8, keys)?;
            println!("layer {layer} write verified");
        }
        link.write_light(&light)?;
        println!("light write verified");
        link.write_diy(&diy)?;
        println!("diy write verified");
        if !macros.is_empty() {
            link.write_macros(&macros)?;
            println!("macro write verified");
        }
    }

    Ok(())
}
