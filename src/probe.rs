use gtk::glib;

use crate::device::link::{KeyboardLink, RealLink};
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

fn run_inner(write: bool) -> Result<(), Box<dyn std::error::Error>> {
    let model = Model::load();
    let mut link = RealLink::open(&model)?;
    let info = link.info().clone();
    println!("device: {} ({})", info.product_name, info.hidraw);
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
    }

    Ok(())
}
