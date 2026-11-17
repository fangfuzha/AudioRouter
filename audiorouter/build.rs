fn main() {
    // 部署模式：
    // - 默认：自包含。WindowsAppRuntime 的运行时 DLL 随安装包发布，同时把
    //   app.manifest 嵌进 exe（清单声明了必须就地加载的那些 DLL）。
    // - framework-dependent 特性：框架依赖。exe 不内嵌清单，运行时用系统上
    //   已安装的 WindowsAppRuntime 包；缺失时由 bootstrapper 弹快速安装。
    //
    // build script 里拿不到 #[cfg(feature)]，只能用 CARGO_FEATURE_* 环境变量。
    //
    // 注意：这两个函数都会往 target 目录拷文件（自包含拷整个运行时，框架依赖
    // 拷 bootstrapper）。这是 build script 的副作用，sccache 命中时不会发生 ——
    // 所以打包脚本不依赖它们，自己从 nupkg/MSIX 准备运行时。
    let framework_dependent = std::env::var_os("CARGO_FEATURE_FRAMEWORK_DEPENDENT").is_some();
    if framework_dependent {
        windows_reactor_setup::as_framework_dependent();
    } else {
        windows_reactor_setup::as_self_contained();
    }

    // 把项目根目录的 assets/icon.ico 嵌入到 exe 资源段。
    // 嵌入后 exe 本身携带图标资源，Windows 资源管理器、任务栏、开始菜单、
    // 桌面快捷方式都会显示该图标（无需依赖外部 ico 文件）。
    // Cargo.toml 中 manifest_dir 指向 audiorouter/，所以 ../assets 指向项目根。
    let icon_path = std::path::Path::new("..").join("assets").join("icon.ico");
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
        println!(
            "cargo:warning=Icon not found at {}, skipping resource embedding",
            icon_path.display()
        );
    }

    println!("cargo:rerun-if-changed={}", icon_path.display());
    println!("cargo:rerun-if-changed=build.rs");
}
