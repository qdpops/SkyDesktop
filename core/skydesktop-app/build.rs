//! Ресурсы Windows: иконка приложения и метаданные версии.
//!
//! Иконка кладётся в ресурсы под id `1`, чтобы окно могло поднять её
//! через `tao::platform::windows::IconExtWindows::from_resource(1, ..)`.

fn main() {
    // Адрес каталога нод вшивается в сборку — пересобираем при его смене.
    println!("cargo:rerun-if-env-changed=SKYDESKTOP_DIRECTORY");

    #[cfg(windows)]
    {
        let icon = std::path::Path::new("../../icons_bg.ico");
        println!("cargo:rerun-if-changed=../../icons_bg.ico");
        if icon.exists() {
            let mut res = winresource::WindowsResource::new();
            res.set_icon_with_id(icon.to_str().unwrap(), "1");
            res.set("ProductName", "SkyDesktop");
            res.set("FileDescription", "SkyDesktop — удалённый рабочий стол");
            res.set("CompanyName", "SkyDesktop");
            if let Err(err) = res.compile() {
                println!("cargo:warning=не удалось встроить ресурсы Windows: {err}");
            }
        }
    }
}
