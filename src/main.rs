/* main.rs
 *
 * Copyright 2026 ddmt
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program.  If not, see <https://www.gnu.org/licenses/>.
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

mod application;
mod config;
mod device;
mod macro_store;
mod macro_table;
mod model;
mod probe;
mod protocol;
mod ui;

use self::application::MchoseHubLinuxApplication;

use config::{GETTEXT_PACKAGE, LOCALEDIR};
use gettextrs::{bind_textdomain_codeset, bindtextdomain, textdomain};
use gtk::prelude::*;
use gtk::{gio, glib};

fn main() -> glib::ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "MCHOSE HUB for Linux {}\n\n用法：\n  mchose_hub_linux                 启动图形界面\n  mchose_hub_linux --simulate      使用模拟设备\n  mchose_hub_linux --probe         只读导出真机数据\n  mchose_hub_linux --probe-write   原样写回并校验（幂等）\n  mchose_hub_linux --version       显示版本",
            config::VERSION
        );
        return glib::ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--version") {
        println!("MCHOSE HUB {}", config::VERSION);
        return glib::ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--probe" || a == "--probe-write") {
        let write = args.iter().any(|a| a == "--probe-write");
        return probe::run(write);
    }

    let icon_data = include_bytes!(concat!(env!("OUT_DIR"), "/mchose_icons.gresource"));
    let icons = gio::Resource::from_data(&glib::Bytes::from_static(icon_data))
        .expect("Could not parse icon resources");
    gio::resources_register(&icons);

    // Translation setup is best-effort so the app also runs from a build tree.
    let _ = bindtextdomain(GETTEXT_PACKAGE, LOCALEDIR);
    let _ = bind_textdomain_codeset(GETTEXT_PACKAGE, "UTF-8");
    let _ = textdomain(GETTEXT_PACKAGE);

    // Create a new GtkApplication. The application manages our main loop,
    // application windows, integration with the window manager/compositor, and
    // desktop features such as file opening and single-instance applications.
    let simulate = args.iter().any(|a| a == "--simulate");
    let model = model::Model::load();
    let app = MchoseHubLinuxApplication::new(
        "top.ddmt.mchosehub",
        &gio::ApplicationFlags::empty(),
        model,
        simulate,
    );

    // Run the application. This function will block until the application
    // exits. Upon return, we have our exit code to return to the shell. (This
    // is the code you see when you do `echo $?` after running a command in a
    // terminal.
    let gtk_args: Vec<String> = args
        .iter()
        .filter(|arg| arg.as_str() != "--simulate")
        .cloned()
        .collect();
    app.run_with_args(&gtk_args)
}
