use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_core::controller::AppController;
use audio_core::router::ChannelMode;
use windows_reactor::*;

use crate::tray::TrayCommand;
use crate::window_utils;

/// 更新状态机，用于设置页面的 UI 展示
#[derive(Clone)]
pub enum UpdateState {
    Idle,
    Checking,
    UpToDate,
    Available {
        version: String,
        download_url: String,
        release_notes: String,
        file_size: u64,
    },
    Downloading {
        downloaded: u64,
        total: u64,
    },
    Ready(std::path::PathBuf),
    Failed(String),
}

impl Default for UpdateState {
    fn default() -> Self {
        Self::Idle
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ThemeChoice {
    FollowSystem,
    Light,
    Dark,
}

/// 设置整个应用的主题。WinUI 3 中 `Microsoft.UI.Xaml.Application` 本身没有
/// `RequestedTheme` 属性(只有 UWP `Windows.UI.Xaml.Application` 有),所以这里
/// 只能设置根 FrameworkElement 的 RequestedTheme(由 windows-reactor 的
/// `set_requested_theme` 内部处理),并由 Pane 背景覆盖模块在 XAML 资源层
/// 提供显式控制。
fn apply_theme(choice: ThemeChoice) {
    let requested = match choice {
        ThemeChoice::FollowSystem => RequestedTheme::Default,
        ThemeChoice::Light => RequestedTheme::Light,
        ThemeChoice::Dark => RequestedTheme::Dark,
    };
    set_requested_theme(requested);
}

fn detect_system_is_dark() -> bool {
    matches!(dark_light::detect(), Ok(dark_light::Mode::Dark))
}

fn open_url_in_browser(url: &str) {
    use std::process::Command;
    #[cfg(target_os = "windows")]
    {
        let _ = Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("open").arg(url).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = Command::new("xdg-open").arg(url).spawn();
    }
}

const NAV_TAG_HOME: &str = "home";
const NAV_TAG_SETTINGS: &str = "settings";
/// `NavViewItem.tag` 选中的内置 SettingsItem（框架固定字符串 "settings"）。
/// 我们在 on_selection_changed 中通过排除法区分：
/// - tag == NAV_TAG_HOME        → 切到首页
/// - tag == NAV_TAG_GITHUB      → 打开仓库 URL（不导航）
/// - 其它（"settings" 或空串） → 切到设置页
const NAV_TAG_GITHUB: &str = "github";
const GITHUB_REPO_URL: &str = "https://github.com/fangfuzha/AudioRouter";

pub struct RootComponent {
    controller: Arc<Mutex<AppController>>,
    tick: Cell<u64>,
    set_tick: RefCell<Option<SetState<u64>>>,
    timer: RefCell<Option<DispatcherTimer>>,
    update_state: Arc<Mutex<UpdateState>>,
}

impl RootComponent {
    pub fn new(controller: Arc<Mutex<AppController>>) -> Self {
        Self {
            controller,
            tick: Cell::new(0),
            set_tick: RefCell::new(None),
            timer: RefCell::new(None),
            update_state: Arc::new(Mutex::new(UpdateState::Idle)),
        }
    }
}

impl Component for RootComponent {
    fn render(&self, _props: &(), cx: &mut RenderCx) -> Element {
        let (tick_val, set_tick) = cx.use_state(self.tick.get());
        self.tick.set(tick_val);
        *self.set_tick.borrow_mut() = Some(set_tick.clone());

        // 一次性副作用:在组件 mount 时向 Application.Resources 注入 Pane 背景
        // 覆盖资源,这样 NavigationView 的 Pane 背景才能在亮/暗主题下都正确。
        cx.use_effect((), || {
            crate::pane_bg_override::install_pane_background_overrides();
        });

        let initial_expanded = {
            let c = self.controller.lock().unwrap();
            c.nav_pane_expanded()
        };
        let (nav_expanded, set_nav_expanded) = cx.use_state(initial_expanded);
        let (nav_selected, set_nav_selected) = cx.use_state(NAV_TAG_HOME.to_string());

        let (theme_choice, set_theme_choice) = cx.use_state(ThemeChoice::FollowSystem);
        cx.use_effect((), || {
            // FollowSystem 不能简单传 RequestedTheme::Default：
            // Microsoft.UI.Xaml.Application 没有 RequestedTheme 属性，
            // ElementTheme::Default 会继承 WinUI 3 框架默认主题，
            // 在系统暗色模式下启动时窗口背景会异常偏暗，
            // 直到触发某些重新渲染才恢复正常。
            // 这里显式检测系统主题并设置 ElementTheme::Light/Dark。
            let choice = if detect_system_is_dark() {
                ThemeChoice::Dark
            } else {
                ThemeChoice::Light
            };
            apply_theme(choice);
        });

        // 启动时的 backdrop 已在 main.rs 中通过 App::backdrop 在窗口创建阶段
        // 直接应用，无需在此处 use_effect 中事后设置。

        // 启动时从配置加载 close_to_tray 设置并安装窗口子类化
        let close_to_tray_initial = {
            let c = self.controller.lock().unwrap();
            c.close_to_tray()
        };
        cx.use_effect(close_to_tray_initial, move || {
            window_utils::set_close_to_tray(close_to_tray_initial);
            window_utils::install_close_to_tray();
        });

        // 启动时后台静默检查更新（受配置控制）
        let auto_update_enabled = {
            let c = self.controller.lock().unwrap();
            let cfg = c.config_manager.handle();
            let enabled = cfg.read().general.auto_update_check;
            enabled
        };
        let update_state_clone = Arc::clone(&self.update_state);
        cx.use_effect(auto_update_enabled, move || {
            if !auto_update_enabled {
                return;
            }
            let state = Arc::clone(&update_state_clone);
            std::thread::spawn(move || {
                // 延迟 2 秒再检查，避免影响启动速度
                std::thread::sleep(std::time::Duration::from_secs(2));
                let result = crate::update::check_for_updates();
                let new_state = match result {
                    crate::update::UpdateCheckResult::UpToDate => {
                        log::info!("Update check: already up to date (v{})", crate::update::current_version());
                        UpdateState::UpToDate
                    }
                    crate::update::UpdateCheckResult::NewVersion {
                        version,
                        download_url,
                        release_notes,
                        file_size,
                    } => {
                        log::info!("Update check: new version {version} available");
                        UpdateState::Available {
                            version,
                            download_url,
                            release_notes,
                            file_size,
                        }
                    }
                    crate::update::UpdateCheckResult::Failed(e) => {
                        log::warn!("Update check failed: {e}");
                        return;
                    }
                };
                *state.lock().unwrap() = new_state;
                // UI 重渲染依赖主循环的 700ms timer 自动触发
            });
        });

        if self.timer.borrow().is_none() {
            let controller = Arc::clone(&self.controller);
            let tick_cell = self.tick.clone();
            let set_tick_cell = self.set_tick.clone();
            match DispatcherTimer::new(Duration::from_millis(700), move || {
                {
                    let mut c = controller.lock().unwrap();
                    c.refresh_devices();
                    c.poll_router_events();
                }

                // 托盘图标左键点击与托盘菜单项点击复用同一个命令处理逻辑。
                // try_recv_tray_event 处理左键点击，try_recv_menu_event 处理菜单项点击。
                let handle_command = |cmd: TrayCommand| match cmd {
                    TrayCommand::ToggleWindow => window_utils::toggle_window(),
                    TrayCommand::ShowWindow => window_utils::show_and_focus_window(),
                    TrayCommand::Quit => std::process::exit(0),
                };
                while let Some(cmd) = crate::tray::try_recv_tray_event() {
                    handle_command(cmd);
                }
                while let Some(cmd) = crate::tray::try_recv_menu_event() {
                    handle_command(cmd);
                }

                let new_tick = tick_cell.get().wrapping_add(1);
                tick_cell.set(new_tick);
                if let Some(ref setter) = *set_tick_cell.borrow() {
                    setter.call(new_tick);
                }
            }) {
                Ok(timer) => {
                    *self.timer.borrow_mut() = Some(timer);
                }
                Err(e) => {
                    log::error!("Failed to create DispatcherTimer: {e}");
                }
            }
        }

        main_app(
            Arc::clone(&self.controller),
            set_tick,
            nav_expanded,
            set_nav_expanded,
            nav_selected,
            set_nav_selected,
            theme_choice,
            set_theme_choice,
            Arc::clone(&self.update_state),
        )
    }
}

fn main_app(
    controller: Arc<Mutex<AppController>>,
    set_tick: SetState<u64>,
    nav_expanded: bool,
    set_nav_expanded: SetState<bool>,
    nav_selected: String,
    set_nav_selected: SetState<String>,
    theme_choice: ThemeChoice,
    set_theme_choice: SetState<ThemeChoice>,
    update_state: Arc<Mutex<UpdateState>>,
) -> Element {
    let c = controller.lock().unwrap();
    let i18n = c.i18n.clone();
    drop(c);

    let shared_tick = Arc::new(AtomicU64::new(0));
    let make_setter = {
        let st = Arc::clone(&shared_tick);
        let setter = set_tick.clone();
        move || {
            let new_tick = st.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
            setter.call(new_tick);
        }
    };

    // nav_items:首页 + 仓库链接。
    // 注:NavViewItem.icon 接受 IconElement,而 windows-reactor 的 bindings
    // 没有 BitmapIcon/FontIcon/PathIcon(只有 SymbolIcon)。所以这里用
    // Symbol::Globe 作为仓库图标(代表"打开外部链接");GitHub 官方 PNG
    // 走的是 pane_footer 之外的方案,需要时再扩展 IconElement 支持。
    let nav_items = [
        NavViewItem::new(i18n.t("AppTitle"))
            .tag(NAV_TAG_HOME)
            .icon(Symbol::Home),
        NavViewItem::new(i18n.t("GitHub"))
            .tag(NAV_TAG_GITHUB)
            .icon(Symbol::Globe),
    ];

    let body: Element = if nav_selected == NAV_TAG_SETTINGS {
        settings_page(
            Arc::clone(&controller),
            i18n.clone(),
            make_setter.clone(),
            set_nav_selected.clone(),
            theme_choice,
            set_theme_choice,
            Arc::clone(&update_state),
        )
    } else {
        home_page(
            Arc::clone(&controller),
            i18n.clone(),
            make_setter.clone(),
        )
    };

    // 使用 LeftCompact 模式：始终显示图标条，不会因窗口变窄而完全隐藏侧栏
    // （Auto 模式在窗口过小时会切换到 Minimal，导致汉堡按钮悬浮在内容上）
    //
    // 关键：windows-reactor 的 diff 机制在属性值不变时跳过 setter 调用
    // （见 reconciler/diff_helpers.rs 的 diff_props 实现）。
    // 因此 Rust 侧 is_pane_open 值不变时，不会覆盖用户点击 PaneToggleButton
    // 后框架内部切换的 IsPaneOpen 状态。框架自行管理展开/收起，Rust 侧
    // 只提供初始值。
    let pane_open = nav_expanded;
    let _ = set_nav_expanded;

    // 内置 SettingsItem 点击时 Tag 为 null，提取链返回空字符串。
    // 用排除法：tag == NAV_TAG_HOME → 首页；tag == NAV_TAG_GITHUB → 打开
    // 仓库 URL 后回弹选中项；其它（含 "settings" 与空串）→ 设置页。
    // GitHub 按钮在 nav_items 中(在 settings 上方),点击后浏览器打开仓库,
    // 然后把 selected_tag 回弹到 home（避免侧栏停留在这个临时态上）。
    let nav_selected_for_handler = nav_selected.clone();
    let set_nav_selected_for_handler = set_nav_selected.clone();
    let controller_for_handler = Arc::clone(&controller);
    Element::from(
        NavigationView::new(nav_items, body)
            .selected_tag(&nav_selected)
            .back_button_visible(false)
            .on_selection_changed(move |tag: String| {
                log::info!("NavigationView selection changed, tag={:?}", tag);
                if tag == NAV_TAG_HOME {
                    let mut c = controller_for_handler.lock().unwrap();
                    c.begin_settings_edit();
                    set_nav_selected_for_handler.call(NAV_TAG_HOME.to_string());
                } else if tag == NAV_TAG_GITHUB {
                    open_url_in_browser(GITHUB_REPO_URL);
                    // 点击 GitHub 项不真正切换页面，弹回之前选中的项
                    set_nav_selected_for_handler.call(nav_selected_for_handler.clone());
                } else {
                    let mut c = controller_for_handler.lock().unwrap();
                    c.begin_settings_edit();
                    set_nav_selected_for_handler.call(NAV_TAG_SETTINGS.to_string());
                }
            })
            .settings_visible(true)
            .pane_title("AudioRouter")
            .pane_display_mode(NavigationViewPaneDisplayMode::LeftCompact)
            .pane_open(pane_open)
            .open_pane_length(240.0),
    )
}

fn home_page(
    controller: Arc<Mutex<AppController>>,
    i18n: app_core::i18n::I18n,
    make_setter: impl Fn() + Clone + 'static,
) -> Element {
    let c = controller.lock().unwrap();
    let source_devices: Vec<_> = c.devices.iter().cloned().collect();
    let output_devices: Vec<_> = c.filtered_target_devices().into_iter().cloned().collect();
    let is_running = c.is_running;
    let status_text = c.status_text.clone();
    let selected_source_id = c.selected_source.clone();
    drop(c);

    let channel_mode_items: Vec<String> = vec![
        i18n.t("channelModes.Stereo").to_string(),
        i18n.t("channelModes.LeftMono").to_string(),
        i18n.t("channelModes.RightMono").to_string(),
        i18n.t("channelModes.Mono").to_string(),
        i18n.t("channelModes.Swap").to_string(),
        i18n.t("channelModes.LeftOnly").to_string(),
        i18n.t("channelModes.RightOnly").to_string(),
    ];

    // 每个声道模式的处理逻辑说明,作为 ComboBox 的悬浮提示。
    // 与 channel_mode_items 一一对应。
    let channel_mode_descriptions: Vec<String> = vec![
        i18n.t("channelModeDesc.Stereo").to_string(),
        i18n.t("channelModeDesc.LeftMono").to_string(),
        i18n.t("channelModeDesc.RightMono").to_string(),
        i18n.t("channelModeDesc.Mono").to_string(),
        i18n.t("channelModeDesc.Swap").to_string(),
        i18n.t("channelModeDesc.LeftOnly").to_string(),
        i18n.t("channelModeDesc.RightOnly").to_string(),
    ];

    // 源设备下拉列表
    let source_device_names: Vec<String> =
        source_devices.iter().map(|d| d.friendly_name.clone()).collect();
    let selected_source_index = source_devices
        .iter()
        .position(|d| Some(&d.id) == selected_source_id.as_ref())
        .map(|i| i as i32)
        .unwrap_or(-1);

    let source_combo = {
        let controller_clone = Arc::clone(&controller);
        let refresh = make_setter.clone();
        let devices = source_devices.clone();
        ComboBox::new(source_device_names)
            .selected_index(selected_source_index)
            .on_selection_changed(move |index| {
                if let Some(device) = devices.get(index as usize) {
                    let mut c = controller_clone.lock().unwrap();
                    c.select_source_device(device.id.clone());
                    refresh();
                }
            })
    };

    let output_items: Vec<Element> = output_devices
        .into_iter()
        .map(|device| {
            let device_id = device.id.clone();

            let (enabled, selected_mode_index) = {
                let c = controller.lock().unwrap();
                let handle = c.config_manager.handle();
                let cfg = handle.read();
                let output = cfg.outputs.iter().find(|o| o.device_id == device_id);
                let enabled = output.map(|o| o.enabled).unwrap_or(false);
                let mode = output
                    .and_then(|o| o.channel_mode.as_deref())
                    .map(|s| ChannelMode::from_config(Some(s)))
                    .unwrap_or(ChannelMode::Stereo);
                let index = mode as i32;
                (enabled, index)
            };

            // 当前选中模式对应的处理逻辑说明,用作 ComboBox 悬浮提示。
            // 渲染时由 make_setter 触发刷新,选择变更后 tooltip 会随重渲染更新。
            let selected_desc = channel_mode_descriptions
                .get(selected_mode_index as usize)
                .cloned()
                .unwrap_or_default();

            // 使用 Grid + 三列 [Auto, Star, Auto] 让 ComboBox 右对齐:
            // hstack 不会拉伸子元素,而 Grid 的 Star 列可占据剩余空间,
            // 把第三列(ComboBox)推到行末。
            Element::from(
                grid((
                    Element::from({
                        let controller_clone = Arc::clone(&controller);
                        let refresh = make_setter.clone();
                        let device_id = device_id.clone();
                        check_box(enabled).on_checked(move |checked| {
                            let mut c = controller_clone.lock().unwrap();
                            c.set_output_enabled(&device_id, checked);
                            refresh();
                        })
                    })
                    .grid_column(0),
                    Element::from(text_block(device.friendly_name.clone())).grid_column(1),
                    Element::from({
                        let controller_clone = Arc::clone(&controller);
                        let refresh = make_setter.clone();
                        let device_id = device_id.clone();
                        ComboBox::new(channel_mode_items.clone())
                            .selected_index(selected_mode_index)
                            .on_selection_changed(move |index| {
                                let mode = match index {
                                    0 => ChannelMode::Stereo,
                                    1 => ChannelMode::LeftMono,
                                    2 => ChannelMode::RightMono,
                                    3 => ChannelMode::Mono,
                                    4 => ChannelMode::Swap,
                                    5 => ChannelMode::LeftOnly,
                                    _ => ChannelMode::RightOnly,
                                };
                                let mut c = controller_clone.lock().unwrap();
                                c.set_output_channel_mode(&device_id, mode);
                                refresh();
                            })
                    })
                    .tooltip(selected_desc)
                    .grid_column(2),
                ))
                .columns([GridLength::Auto, GridLength::STAR, GridLength::Auto])
                .column_spacing(12.0),
            )
        })
        .collect();

    let toggle_controller = Arc::clone(&controller);
    let toggle_refresh = make_setter.clone();
    let toggle_btn = if is_running {
        button(i18n.t("Stop"))
            .accent()
            .on_click(move || {
                let mut c = toggle_controller.lock().unwrap();
                c.stop_routing();
                toggle_refresh();
            })
    } else {
        button(i18n.t("Start"))
            .accent()
            .on_click(move || {
                let mut c = toggle_controller.lock().unwrap();
                c.start_routing();
                toggle_refresh();
            })
    };

    Element::from(
        vstack((
            Element::from(
                hstack((
                    Element::from(title(i18n.t("AppTitle"))),
                    Element::from(
                        text_block(if is_running {
                            i18n.t("Running")
                        } else {
                            i18n.t("StatusReady")
                        })
                        .font_size(13.0),
                    ),
                ))
                .spacing(12.0),
            ),
            Element::from(text_block(i18n.t("SourceDevice")).bold()),
            Element::from(source_combo),
            Element::from(text_block(i18n.t("OutputDevices")).bold()),
            Element::from(vstack(output_items).spacing(4.0)),
            Element::from(
                hstack((
                    Element::from(text_block(status_text)),
                    Element::from(toggle_btn),
                ))
                .spacing(12.0)
                .horizontal_alignment(HorizontalAlignment::Right),
            ),
        ))
        .spacing(8.0)
        .padding(Thickness { left: 16.0, top: 12.0, right: 16.0, bottom: 16.0 }),
    )
}

fn settings_page(
    controller: Arc<Mutex<AppController>>,
    i18n: app_core::i18n::I18n,
    make_setter: impl Fn() + Clone + 'static,
    set_nav_selected: SetState<String>,
    theme_choice: ThemeChoice,
    set_theme_choice: SetState<ThemeChoice>,
    update_state: Arc<Mutex<UpdateState>>,
) -> Element {
    let (start_with_windows, start_minimized, auto_route, close_to_tray, auto_update_check, lang_index, theme_index, backdrop_index) = {
        let c = controller.lock().unwrap();
        let draft = &c.draft_general;
        let lang_idx = match draft.language.as_str() {
            "zh" => 1,
            _ => 0,
        };
        let theme_idx = match theme_choice {
            ThemeChoice::FollowSystem => 0,
            ThemeChoice::Light => 1,
            ThemeChoice::Dark => 2,
        };
        let backdrop_idx = match draft.backdrop {
            config::config::Backdrop::Mica => 0,
            config::config::Backdrop::MicaAlt => 1,
            config::config::Backdrop::Acrylic => 2,
        };
        (
            draft.start_with_windows,
            draft.minimized,
            draft.auto_route,
            draft.close_to_tray,
            draft.auto_update_check,
            lang_idx,
            theme_idx,
            backdrop_idx,
        )
    };

    let lang_items = vec!["English".to_string(), "简体中文".to_string()];
    let theme_items = vec![
        i18n.t("ThemeFollowSystem").to_string(),
        i18n.t("ThemeLight").to_string(),
        i18n.t("ThemeDark").to_string(),
    ];
    let backdrop_items = vec![
        i18n.t("BackdropMica").to_string(),
        i18n.t("BackdropMicaAlt").to_string(),
        i18n.t("BackdropAcrylic").to_string(),
    ];

    let back_ctrl = controller.clone();
    let back_nav = set_nav_selected.clone();
    let cancel_btn = button(i18n.t("Cancel")).on_click(move || {
        let mut c = back_ctrl.lock().unwrap();
        c.begin_settings_edit();
        back_nav.call(NAV_TAG_HOME.to_string());
    });

    let save_controller = Arc::clone(&controller);
    let save_refresh = make_setter.clone();
    let save_nav = set_nav_selected.clone();
    let save_btn = button(i18n.t("Save")).accent().on_click(move || {
        let mut c = save_controller.lock().unwrap();
        let new_close_to_tray = c.draft_general.close_to_tray;
        let lang_changed = c.save_general_config();
        if lang_changed.is_some() {
            crate::tray::update_tray_language(&c.i18n);
        }
        window_utils::set_close_to_tray(new_close_to_tray);
        save_refresh();
        save_nav.call(NAV_TAG_HOME.to_string());
    });

    Element::from(
        vstack((
            Element::from(title(i18n.t("SettingsTitle"))),
            Element::from(
                border(
                    vstack((
                        Element::from(
                            check_box(start_with_windows)
                                .content(i18n.t("StartWithWindows"))
                                .on_checked({
                                    let controller_clone = Arc::clone(&controller);
                                    move |checked| {
                                        let mut c = controller_clone.lock().unwrap();
                                        c.draft_general.start_with_windows = checked;
                                    }
                                }),
                        ),
                        Element::from(
                            check_box(start_minimized)
                                .content(i18n.t("StartMinimized"))
                                .on_checked({
                                    let controller_clone = Arc::clone(&controller);
                                    move |checked| {
                                        let mut c = controller_clone.lock().unwrap();
                                        c.draft_general.minimized = checked;
                                    }
                                }),
                        ),
                        Element::from(
                            check_box(auto_route)
                                .content(i18n.t("AutoRoute"))
                                .on_checked({
                                    let controller_clone = Arc::clone(&controller);
                                    move |checked| {
                                        let mut c = controller_clone.lock().unwrap();
                                        c.draft_general.auto_route = checked;
                                    }
                                }),
                        ),
                        Element::from(
                            check_box(close_to_tray)
                                .content(i18n.t("CloseToTray"))
                                .on_checked({
                                    let controller_clone = Arc::clone(&controller);
                                    move |checked| {
                                        let mut c = controller_clone.lock().unwrap();
                                        c.draft_general.close_to_tray = checked;
                                    }
                                }),
                        ),
                        Element::from(
                            check_box(auto_update_check)
                                .content(i18n.t("AutoUpdateCheck"))
                                .on_checked({
                                    let controller_clone = Arc::clone(&controller);
                                    move |checked| {
                                        let mut c = controller_clone.lock().unwrap();
                                        c.draft_general.auto_update_check = checked;
                                    }
                                }),
                        ),
                        Element::from(
                            hstack((
                                Element::from(text_block(i18n.t("Language"))),
                                Element::from(
                                    ComboBox::new(lang_items)
                                        .selected_index(lang_index)
                                        .on_selection_changed({
                                            let controller_clone = Arc::clone(&controller);
                                            move |index| {
                                                let mut c = controller_clone.lock().unwrap();
                                                c.draft_general.language = match index {
                                                    1 => "zh".to_string(),
                                                    _ => "en".to_string(),
                                                };
                                            }
                                        }),
                                ),
                            ))
                            .spacing(8.0),
                        ),
                        Element::from(
                            hstack((
                                Element::from(text_block(i18n.t("Theme"))),
                                Element::from(
                                    ComboBox::new(theme_items)
                                        .selected_index(theme_index)
                                        .on_selection_changed(move |index| {
                                            let choice = match index {
                                                1 => ThemeChoice::Light,
                                                2 => ThemeChoice::Dark,
                                                _ => ThemeChoice::FollowSystem,
                                            };
                                            apply_theme(choice);
                                            set_theme_choice.call(choice);
                                        }),
                                ),
                            ))
                            .spacing(8.0),
                        ),
                        Element::from(
                            hstack((
                                Element::from(text_block(i18n.t("Backdrop"))),
                                Element::from(
                                    ComboBox::new(backdrop_items)
                                        .selected_index(backdrop_index)
                                        .on_selection_changed({
                                            let controller_clone = Arc::clone(&controller);
                                            move |index| {
                                                let mut c = controller_clone.lock().unwrap();
                                                let bd = match index {
                                                    1 => config::config::Backdrop::MicaAlt,
                                                    2 => config::config::Backdrop::Acrylic,
                                                    _ => config::config::Backdrop::Mica,
                                                };
                                                c.draft_general.backdrop = bd;
                                                // 实时应用 backdrop 以便预览效果
                                                use windows_reactor::Backdrop as WrBackdrop;
                                                let wr_bd = match bd {
                                                    config::config::Backdrop::Mica => WrBackdrop::Mica,
                                                    config::config::Backdrop::MicaAlt => WrBackdrop::MicaAlt,
                                                    config::config::Backdrop::Acrylic => WrBackdrop::Acrylic,
                                                };
                                                windows_reactor::set_backdrop(Some(wr_bd));
                                            }
                                        }),
                                ),
                            ))
                            .spacing(8.0),
                        ),
                    ))
                    .spacing(14.0),
                )
                .padding(Thickness::uniform(16.0))
                .background(ThemeRef::LayerFill)
                .corner_radius(8.0),
            ),
            Element::from(build_update_section(
                Arc::clone(&update_state),
                i18n.clone(),
            )),
            Element::from(
                hstack((Element::from(cancel_btn), Element::from(save_btn)))
                    .spacing(8.0)
                    .horizontal_alignment(HorizontalAlignment::Right),
            ),
        ))
        .spacing(12.0)
        .padding(Thickness { left: 16.0, top: 12.0, right: 16.0, bottom: 16.0 }),
    )
}

/// 构建设置页面中的更新区域，根据 UpdateState 展示不同 UI。
///
/// 后台线程只修改共享的 UpdateState，UI 更新依赖主循环的 700ms timer
/// 触发重渲染。这样避免了把 Rc<SetState> 传到后台线程的问题。
fn build_update_section(
    update_state: Arc<Mutex<UpdateState>>,
    i18n: app_core::i18n::I18n,
) -> Element {
    let state = update_state.lock().unwrap().clone();
    let current_ver = crate::update::current_version();

    let header = hstack((
        Element::from(text_block(i18n.t("CheckForUpdates")).bold()),
        Element::from(text_block(format!("v{current_ver}")).font_size(12.0)),
    ))
    .spacing(8.0);

    let body: Element = match state {
        UpdateState::Idle => {
            let state_clone = Arc::clone(&update_state);
            let btn = button(i18n.t("CheckForUpdates")).on_click(move || {
                *state_clone.lock().unwrap() = UpdateState::Checking;
                let sc = Arc::clone(&state_clone);
                std::thread::spawn(move || {
                    let result = crate::update::check_for_updates();
                    let new_state = match result {
                        crate::update::UpdateCheckResult::UpToDate => UpdateState::UpToDate,
                        crate::update::UpdateCheckResult::NewVersion {
                            version,
                            download_url,
                            release_notes,
                            file_size,
                        } => UpdateState::Available {
                            version,
                            download_url,
                            release_notes,
                            file_size,
                        },
                        crate::update::UpdateCheckResult::Failed(e) => UpdateState::Failed(e),
                    };
                    *sc.lock().unwrap() = new_state;
                });
            });
            Element::from(btn)
        }
        UpdateState::Checking => {
            Element::from(vstack((
                Element::from(ProgressBar::indeterminate()),
                Element::from(text_block(i18n.t("CheckingForUpdates")).font_size(12.0)),
            ))
            .spacing(8.0))
        }
        UpdateState::UpToDate => {
            let state_clone = Arc::clone(&update_state);
            let btn = button(i18n.t("CheckForUpdates")).on_click(move || {
                *state_clone.lock().unwrap() = UpdateState::Checking;
                let sc = Arc::clone(&state_clone);
                std::thread::spawn(move || {
                    let result = crate::update::check_for_updates();
                    let new_state = match result {
                        crate::update::UpdateCheckResult::UpToDate => UpdateState::UpToDate,
                        crate::update::UpdateCheckResult::NewVersion {
                            version,
                            download_url,
                            release_notes,
                            file_size,
                        } => UpdateState::Available {
                            version,
                            download_url,
                            release_notes,
                            file_size,
                        },
                        crate::update::UpdateCheckResult::Failed(e) => UpdateState::Failed(e),
                    };
                    *sc.lock().unwrap() = new_state;
                });
            });
            Element::from(vstack((
                Element::from(text_block(i18n.t("UpToDate"))),
                Element::from(btn),
            ))
            .spacing(8.0))
        }
        UpdateState::Available {
            version,
            download_url,
            release_notes,
            file_size,
        } => {
            let size_str = crate::update::format_size(file_size);
            let state_clone = Arc::clone(&update_state);
            let url = download_url.clone();
            let download_btn = button(i18n.t("DownloadUpdate"))
                .accent()
                .on_click(move || {
                    *state_clone.lock().unwrap() = UpdateState::Downloading {
                        downloaded: 0,
                        total: file_size,
                    };
                    let sc = Arc::clone(&state_clone);
                    let url2 = url.clone();
                    std::thread::spawn(move || {
                        let sc_inner = Arc::clone(&sc);
                        let result = crate::update::download_installer(&url2, move |d, t| {
                            let mut s = sc_inner.lock().unwrap();
                            if let UpdateState::Downloading {
                                ref mut downloaded,
                                ref mut total,
                            } = *s
                            {
                                *downloaded = d;
                                if t > 0 {
                                    *total = t;
                                }
                            }
                        });
                        let new_state = match result {
                            Ok(path) => UpdateState::Ready(path),
                            Err(e) => UpdateState::Failed(e.to_string()),
                        };
                        *sc.lock().unwrap() = new_state;
                    });
                });

            let notes_el = if release_notes.is_empty() {
                Element::from(text_block(""))
            } else {
                let notes = release_notes.lines().take(10).collect::<Vec<_>>().join("\n");
                Element::from(text_block(notes).font_size(12.0).opacity(0.7))
            };

            Element::from(vstack((
                Element::from(text_block(format!(
                    "{}: {}",
                    i18n.t("LatestVersion"),
                    version
                ))),
                Element::from(
                    text_block(format!("{}: {size_str}", i18n.t("UpdateAvailable")))
                        .font_size(12.0),
                ),
                notes_el,
                Element::from(download_btn),
            ))
            .spacing(8.0))
        }
        UpdateState::Downloading { downloaded, total } => {
            let progress = if total > 0 {
                (downloaded as f64 / total as f64) * 100.0
            } else {
                0.0
            };
            let downloaded_str = crate::update::format_size(downloaded);
            let total_str = crate::update::format_size(total);
            let progress_text = i18n
                .t("DownloadProgress")
                .replace("{downloaded}", &downloaded_str)
                .replace("{total}", &total_str);

            Element::from(vstack((
                Element::from(ProgressBar::new(progress).range(0.0, 100.0)),
                Element::from(text_block(progress_text).font_size(12.0)),
            ))
            .spacing(8.0))
        }
        UpdateState::Ready(path) => {
            let install_btn = button(i18n.t("InstallAndRestart"))
                .accent()
                .on_click(move || {
                    crate::update::launch_installer_and_quit(&path);
                });
            Element::from(vstack((
                Element::from(text_block(i18n.t("UpdateReady"))),
                Element::from(install_btn),
            ))
            .spacing(8.0))
        }
        UpdateState::Failed(err) => {
            let state_clone = Arc::clone(&update_state);
            let err_text = i18n.t("UpdateFailed").replace("{error}", &err);
            let btn = button(i18n.t("CheckForUpdates")).on_click(move || {
                *state_clone.lock().unwrap() = UpdateState::Checking;
                let sc = Arc::clone(&state_clone);
                std::thread::spawn(move || {
                    let result = crate::update::check_for_updates();
                    let new_state = match result {
                        crate::update::UpdateCheckResult::UpToDate => UpdateState::UpToDate,
                        crate::update::UpdateCheckResult::NewVersion {
                            version,
                            download_url,
                            release_notes,
                            file_size,
                        } => UpdateState::Available {
                            version,
                            download_url,
                            release_notes,
                            file_size,
                        },
                        crate::update::UpdateCheckResult::Failed(e) => UpdateState::Failed(e),
                    };
                    *sc.lock().unwrap() = new_state;
                });
            });
            Element::from(vstack((
                Element::from(text_block(err_text).font_size(12.0)),
                Element::from(btn),
            ))
            .spacing(8.0))
        }
    };

    Element::from(
        border(
            vstack((Element::from(header), body))
                .spacing(12.0),
        )
        .padding(Thickness::uniform(16.0))
        .background(ThemeRef::LayerFill)
        .corner_radius(8.0),
    )
}
