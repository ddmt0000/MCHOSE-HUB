fn main() {
    glib_build_tools::compile_resources(
        &["data/icons/mchose"],
        "data/icons/mchose/icons.gresource.xml",
        "mchose_icons.gresource",
    );
    println!("cargo:rerun-if-changed=data/icons/mchose/icons.gresource.xml");
    println!("cargo:rerun-if-changed=data/icons/mchose");
}
