//! Slint 前端入口。当前作为迁移中的独立 UI 壳，后续会替换 egui 入口。

use std::cell::RefCell;
use std::rc::Rc;

use audio_core::router::Router;
use config::ConfigManager;
use slint::ComponentHandle;

use crate::controller::AppController;

slint::include_modules!();

pub fn run_slint_app(config_manager: ConfigManager, router: Router) -> anyhow::Result<()> {
    let ui = MainWindow::new()?;
    let controller = Rc::new(RefCell::new(AppController::new(config_manager, router)));

    {
        let mut controller = controller.borrow_mut();
        controller.init();
        update_main_window(&ui, &controller);
    }

    let weak_ui = ui.as_weak();
    let refresh_controller = Rc::clone(&controller);
    ui.on_refresh_devices(move || {
        if let Some(ui) = weak_ui.upgrade() {
            let mut controller = refresh_controller.borrow_mut();
            controller.refresh_devices();
            update_main_window(&ui, &controller);
        }
    });

    let weak_ui = ui.as_weak();
    let routing_controller = Rc::clone(&controller);
    ui.on_toggle_routing(move || {
        if let Some(ui) = weak_ui.upgrade() {
            let mut controller = routing_controller.borrow_mut();
            if controller.is_running {
                controller.stop_routing();
            } else {
                controller.start_routing();
            }
            update_main_window(&ui, &controller);
        }
    });

    ui.run()?;
    Ok(())
}

fn update_main_window(ui: &MainWindow, controller: &AppController) {
    ui.set_device_count(controller.devices.len() as i32);
    ui.set_is_running(controller.is_running);
    ui.set_status_text(controller.status_text.as_str().into());
}
