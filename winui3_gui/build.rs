fn main() {
    windows_reactor_setup::as_self_contained();

    // 把项目根目录的 assets/icon.ico 嵌入到 exe 资源段。
    // 嵌入后 exe 本身携带图标资源，Windows 资源管理器、任务栏、开始菜单、
    // 桌面快捷方式都会显示该图标（无需依赖外部 ico 文件）。
    // Cargo.toml 中 manifest_dir 指向 winui3_gui/，所以 ../assets 指向项目根。
    let icon_path = std::path::Path::new("..")
        .join("assets")
        .join("icon.ico");
    if icon_path.exists() {
        let mut res = winres::WindowsResource::new();
        res.set_icon_with_id(&icon_path.to_string_lossy(), "MAINICON");
        // 设置文件元数据（在文件属性面板中显示）
        res.set("FileDescription", "AudioRouter");
        res.set("ProductName", "AudioRouter");
        if let Err(e) = res.compile() {
            println!("cargo:warning=Failed to embed Windows resource: {e}");
            println!("cargo:warning=Icon will fall back to default. Ensure rc.exe is available (Visual Studio Build Tools).");
        }
    } else {
        println!("cargo:warning=Icon not found at {}, skipping resource embedding", icon_path.display());
    }

    println!("cargo:rerun-if-changed={}", icon_path.display());
    println!("cargo:rerun-if-changed=build.rs");
}
