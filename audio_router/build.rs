fn main() {
    slint_build::compile("ui/main.slint").expect("failed to compile Slint UI");

    #[cfg(windows)]
    winresource::WindowsResource::new()
        .set_icon("assets/icon.ico")
        .compile()
        .expect("failed to compile Windows resources");
}
