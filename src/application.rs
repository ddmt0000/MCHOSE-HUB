/* application.rs
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

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};

use std::cell::{Cell, RefCell};

use crate::config::VERSION;
use crate::model::Model;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct MchoseHubLinuxApplication {
        pub model: RefCell<Option<Model>>,
        pub simulate: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MchoseHubLinuxApplication {
        const NAME: &'static str = "MchoseHubLinuxApplication";
        type Type = super::MchoseHubLinuxApplication;
        type ParentType = adw::Application;
    }

    impl ObjectImpl for MchoseHubLinuxApplication {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.setup_gactions();
            obj.set_accels_for_action("app.quit", &["<control>q"]);
        }
    }

    impl ApplicationImpl for MchoseHubLinuxApplication {
        fn activate(&self) {
            let application = self.obj();
            if let Some(window) = application.active_window() {
                window.present();
                return;
            }
            let window = crate::ui::build_window(
                application.upcast_ref(),
                application.model(),
                application.simulate(),
            );
            window.present();
        }
    }

    impl GtkApplicationImpl for MchoseHubLinuxApplication {}
    impl AdwApplicationImpl for MchoseHubLinuxApplication {}
}

glib::wrapper! {
    pub struct MchoseHubLinuxApplication(ObjectSubclass<imp::MchoseHubLinuxApplication>)
        @extends gio::Application, gtk::Application, adw::Application,
        @implements gio::ActionGroup, gio::ActionMap;
}

impl MchoseHubLinuxApplication {
    pub fn new(
        application_id: &str,
        flags: &gio::ApplicationFlags,
        model: Model,
        simulate: bool,
    ) -> Self {
        let app: Self = glib::Object::builder()
            .property("application-id", application_id)
            .property("flags", flags)
            .property("resource-base-path", "/top/ddmt/mchosehub")
            .build();
        *app.imp().model.borrow_mut() = Some(model);
        app.imp().simulate.set(simulate);
        app
    }

    fn model(&self) -> Model {
        self.imp()
            .model
            .borrow()
            .clone()
            .expect("model must be set before activate")
    }

    fn simulate(&self) -> bool {
        self.imp().simulate.get()
    }

    fn setup_gactions(&self) {
        let quit_action = gio::ActionEntry::builder("quit")
            .activate(move |app: &Self, _, _| app.quit())
            .build();
        let about_action = gio::ActionEntry::builder("about")
            .activate(move |app: &Self, _, _| app.show_about())
            .build();
        self.add_action_entries([quit_action, about_action]);
    }

    fn show_about(&self) {
        let Some(window) = self.active_window() else {
            return;
        };
        let about = adw::AboutDialog::builder()
            .application_name("MCHOSE HUB")
            .application_icon("top.ddmt.mchosehub")
            .developer_name("ddmt")
            .version(VERSION)
            .developers(vec!["ddmt"])
            .comments(gettext(
                "非官方 Linux 客户端，用于配置 MCHOSE K99 V2 键盘。",
            ))
            .license_type(gtk::License::Gpl30)
            .website(crate::config::REPO_URL)
            // Translators: Replace "translator-credits" with your name/username, and optionally an email or URL.
            .translator_credits(gettext("translator-credits"))
            .copyright("© 2026 ddmt")
            .build();
        about.present(Some(&window));
    }
}
