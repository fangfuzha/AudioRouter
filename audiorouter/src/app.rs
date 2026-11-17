use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use app_core::controller::{AppController, StartBlock, StatusLevel};
use audio_core::router::ChannelMode;
use windows_reactor::*;

use crate::events::RenderSignal;
use crate::window_utils;

/// 生成一个能正常弹出的悬浮提示（tooltip）。
///
/// `ElementExt::tooltip(text)` 走 WinUI 的 `ToolTipService.SetToolTip`，
/// 但 windows-reactor 传进去的是装箱字符串（`IReference<HSTRING>`）。
/// WinUI 3 的 `ToolTipService::ConvertToToolTip` 只认 `IToolTip` 实例或
/// 可渲染对象，装箱字符串会被原样塞进 `ToolTip.Content`，结果内容为空、
/// 整个提示根本不显示。
///
/// `Tooltip::rich(...)` 走 reactor 的富文本分支：它会创建一个真正的
/// `ToolTip` 控件，并把 `TextBlock` 作为内容挂上去，WinUI 才能正常渲染。
fn hover_hint(text: &str) -> Tooltip {
    Tooltip::rich(text_block(text.to_string()))
}

/// 更新状态机，用于设置页面的 UI 展示
#[derive(Clone, Default)]
pub enum UpdateState {
    #[default]
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
        let _ = Command::new("cmd").args(["/C", "start", "", url]).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("open").args([url]).spawn();
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
/// - 其它（"settings" 或空串）  → 切到设置页
const NAV_TAG_GITHUB: &str = "github";
const GITHUB_REPO_URL: &str = "https://github.com/fangfuzha/AudioRouter";

pub struct RootComponent {
    controller: Arc<Mutex<AppController>>,
    /// 跨线程重绘句柄：常驻服务推送状态后用它请求一次重绘。
    render: RenderSignal,
    /// 更新检查状态（后台线程写入，UI 线程读取）。
    update_state: Arc<Mutex<UpdateState>>,
    /// 上一次渲染时是不是处于「错误」状态，用来在错误刚发生时提示一次。
    error_notified: Arc<AtomicBool>,
    /// 框架首次加载模板时会把 `NavigationView.IsPaneOpen` 强制改回默认值
    /// （展开），我们在挂载阶段声明的「收起」会被丢弃。这个标记表示
    /// 「接下来第一个 IsPaneOpen 事件是框架的重置，而不是用户操作」。
    pane_override_pending: Arc<AtomicBool>,
    /// 是否已经渲染过第一帧（用于在首帧设置上面的标记）。
    pane_first_render: Arc<AtomicBool>,
}

impl RootComponent {
    pub fn new(controller: Arc<Mutex<AppController>>, render: RenderSignal) -> Self {
        Self {
            controller,
            render,
            update_state: Arc::new(Mutex::new(UpdateState::Idle)),
            error_notified: Arc::new(AtomicBool::new(false)),
            pane_override_pending: Arc::new(AtomicBool::new(false)),
            pane_first_render: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Component for RootComponent {
    fn render(&self, _props: &(), cx: &mut RenderCx) -> Element {
        // 跨线程安全的重绘触发器：常驻服务推送状态后调用 request() 即可
        // 请求重绘，不依赖定时器轮询共享状态。
        let (_tick_val, set_tick) = cx.use_async_state(0u64);
        self.render.attach(set_tick.clone());

        // 路由在服务进程里失败时，状态是服务推过来的。窗口若隐藏着
        //（启动时最小化），界面上的提示根本看不到，闪烁任务栏图标提醒用户。
        // 只在「刚刚变成错误状态」时闪一次，避免一直闪。
        let status_is_error = {
            let c = self.controller.lock();
            c.status_level == StatusLevel::Error
        };
        if status_is_error {
            if !self.error_notified.swap(true, Ordering::SeqCst) {
                window_utils::flash_window_if_hidden();
            }
        } else {
            self.error_notified.store(false, Ordering::SeqCst);
        }

        // 窗口钩子与位置恢复：WinUI 的窗口标题在首帧之后才就绪，
        // 这里每帧尝试，直到成功（成功后内部直接返回，开销可忽略）。
        window_utils::install_window_hooks();
        window_utils::restore_window_placement();

        // 一次性副作用:在组件 mount 时向 Application.Resources 注入 Pane 背景
        // 覆盖资源,这样 NavigationView 的 Pane 背景才能在亮/暗主题下都正确。
        cx.use_effect((), || {
            crate::pane_bg_override::install_pane_background_overrides();
        });

        let initial_expanded = {
            let c = self.controller.lock();
            c.nav_pane_expanded()
        };
        // nav_expanded：侧栏的「期望状态」，与配置保持一致。
        // pane_declared：实际声明给框架的值。
        //
        // 两者必须分开：WinUI 在加载 NavigationView 模板时会把 IsPaneOpen
        // 强制改回自己的默认值（展开），挂载阶段声明的「收起」会被丢弃；
        // 而 reactor 的 diff 只在声明值发生变化时才下发 setter。声明值因此
        // 需要先「让位」一次，再由下面的 effect 收敛回期望状态。
        let (nav_expanded, set_nav_expanded) = cx.use_state(initial_expanded);
        let (pane_declared, set_pane_declared) = cx.use_state(initial_expanded);
        let (nav_selected, set_nav_selected) = cx.use_state(NAV_TAG_HOME.to_string());

        // 声明「收起」时，框架加载模板会把 IsPaneOpen 改回「展开」并触发一次
        // 事件。那不是用户的操作，必须与用户点击区分开，否则配置里记住的
        // 状态会被启动时的默认值顶掉。
        if !self.pane_first_render.swap(true, Ordering::SeqCst) {
            self.pane_override_pending
                .store(!nav_expanded, Ordering::SeqCst);
        }

        // 让声明值收敛到期望状态：用户点击时两者同步变化；启动时被框架重置
        // 则会把声明值拉回配置里记住的状态。
        cx.use_effect(pane_declared, {
            let set_pane_declared = set_pane_declared.clone();
            move || {
                if pane_declared != nav_expanded {
                    set_pane_declared.call(nav_expanded);
                }
            }
        });
        // 设置页的内联错误提示（保存失败 / 热键被占用等）。
        let (settings_error, set_settings_error) = cx.use_state(None::<String>);
        // 快捷键录制提示（例如「至少要包含一个修饰键」）。
        let (hotkey_status, set_hotkey_status) = cx.use_state(None::<String>);

        // 快捷键捕获结果：窗口过程（UI 线程）把捕获到的组合键写进来，
        // 这里取走并写进设置草稿。
        // 注意：渲染过程中改 state 不会立刻触发一次重绘，所以这里不能
        // 只依赖 state —— 还要就地算出「本次渲染该显示什么」，否则上一条
        // 错误提示（例如「需要修饰键」）会在捕获成功后仍然残留在界面上。
        let mut effective_hotkey_status = hotkey_status.clone();
        if let Some(result) = window_utils::take_capture_result() {
            match result {
                window_utils::CaptureResult::Captured(hotkey) => {
                    {
                        let mut c = self.controller.lock();
                        c.draft_general.global_hotkey = hotkey;
                    }
                    effective_hotkey_status = None;
                    set_hotkey_status.call(None);
                }
                window_utils::CaptureResult::NeedsModifier => {
                    let text = {
                        let c = self.controller.lock();
                        c.i18n.t("HotkeyNeedsModifier").to_string()
                    };
                    effective_hotkey_status = Some(text.clone());
                    set_hotkey_status.call(Some(text));
                }
            }
        }

        // 捕获到组合键后请求一次重绘（回调发生在 UI 线程的窗口过程里）。
        cx.use_effect((), {
            let render = self.render.clone();
            move || {
                window_utils::set_capture_listener(Arc::new(move || {
                    render.request();
                }));
            }
        });

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

        // 启动时后台静默检查更新（受配置控制）
        let auto_update_enabled = {
            let c = self.controller.lock();
            let cfg = c.config_manager.handle();
            let enabled = cfg.read().general.auto_update_check;
            enabled
        };
        let update_state_clone = Arc::clone(&self.update_state);
        cx.use_effect(auto_update_enabled, {
            let render_request = self.render.clone();
            move || {
                if !auto_update_enabled {
                    return;
                }
                let state = Arc::clone(&update_state_clone);
                let render = render_request.clone();
                std::thread::spawn(move || {
                    // 延迟 2 秒再检查，避免影响启动速度
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    let new_state = match crate::update::check_for_updates() {
                        crate::update::UpdateCheckResult::UpToDate => {
                            log::info!(
                                "Update check: already up to date (v{})",
                                crate::update::current_version()
                            );
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
                            UpdateState::Failed(e)
                        }
                    };
                    *state.lock() = new_state;
                    // 直接请求重绘，不再等待定时器轮询。
                    render.request();
                });
            }
        });

        main_app(
            Arc::clone(&self.controller),
            self.render.clone(),
            set_nav_expanded,
            pane_declared,
            set_pane_declared,
            Arc::clone(&self.pane_override_pending),
            nav_selected,
            set_nav_selected,
            theme_choice,
            set_theme_choice,
            Arc::clone(&self.update_state),
            settings_error,
            set_settings_error,
            effective_hotkey_status,
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn main_app(
    controller: Arc<Mutex<AppController>>,
    render: RenderSignal,
    set_nav_expanded: SetState<bool>,
    pane_declared: bool,
    set_pane_declared: SetState<bool>,
    pane_override_pending: Arc<AtomicBool>,
    nav_selected: String,
    set_nav_selected: SetState<String>,
    theme_choice: ThemeChoice,
    set_theme_choice: SetState<ThemeChoice>,
    update_state: Arc<Mutex<UpdateState>>,
    settings_error: Option<String>,
    set_settings_error: SetState<Option<String>>,
    hotkey_status: Option<String>,
) -> Element {
    let c = controller.lock();
    let i18n = c.i18n.clone();
    drop(c);

    // 刷新句柄：UI 线程和后台线程都可以调用，用来请求一次重绘。
    let make_setter = {
        let render = render.clone();
        move || render.request()
    };

    // GitHub 项使用 SVG 图标（ImageIcon + SvgImageSource），根据当前主题
    // 选择浅色（黑色图标）或深色（白色图标）版本，确保在任意主题背景下可见。
    let is_dark = match theme_choice {
        ThemeChoice::Dark => true,
        ThemeChoice::Light | ThemeChoice::FollowSystem => detect_system_is_dark(),
    };
    let svg_name = if is_dark {
        "assets/github-dark.svg"
    } else {
        "assets/github-light.svg"
    };
    let github_icon_uri = {
        let path = crate::resolve_asset_path(svg_name);
        let path_str = path.to_string_lossy().replace('\\', "/");
        format!("file:///{path_str}")
    };

    let body: Element = if nav_selected == NAV_TAG_SETTINGS {
        settings_page(
            Arc::clone(&controller),
            i18n.clone(),
            make_setter.clone(),
            set_nav_selected.clone(),
            theme_choice,
            set_theme_choice,
            Arc::clone(&update_state),
            settings_error.clone(),
            set_settings_error.clone(),
            hotkey_status.clone(),
        )
    } else {
        home_page(Arc::clone(&controller), i18n.clone(), make_setter.clone())
    };

    // === 侧栏 ===
    //
    // 侧栏使用 NavigationView（与 0.3.12 一致）：三横线按钮由框架自带
    // PaneToggleButton 提供，固定画在侧栏顶部——也就是「主页」按钮的上方，
    // 展开/收起时图标由框架自动切换。
    //
    // 展开状态本身由框架托管：框架的 PaneToggleButton 点击后只改控件内部的
    // IsPaneOpen，reactor 的 diff 机制在属性值不变时跳过 setter 调用，所以
    // Rust 侧不会把它覆盖回去。
    //
    // windows-reactor 从 PR #4782 起提供 `on_pane_open_changed`：框架把
    // 新的 IsPaneOpen 回调出来。这里把它同步回 Rust 状态并写入配置，
    // 于是「收起 / 展开」既保留框架原生的按钮位置与图标，又能跨重启记住。
    let set_nav_expanded_for_pane = set_nav_expanded.clone();
    let set_pane_declared_for_pane = set_pane_declared.clone();
    let controller_for_pane = Arc::clone(&controller);
    let pane_open = pane_declared;

    // 导航项：主页 / GitHub 仓库；设置项用框架内置的 SettingsItem。
    let nav_items = [
        NavViewItem::new(i18n.t("AppTitle"))
            .tag(NAV_TAG_HOME)
            .icon(Symbol::Home),
        NavViewItem::new(i18n.t("GitHub"))
            .tag(NAV_TAG_GITHUB)
            .icon(Icon::image(github_icon_uri)),
    ];

    // 内置 SettingsItem 点击时 Tag 为 null，提取链返回空字符串。
    // 用排除法：tag == NAV_TAG_HOME → 首页；tag == NAV_TAG_GITHUB → 打开
    // 仓库 URL 后回弹选中项；其它（含 "settings" 与空串）→ 设置页。
    let nav_selected_for_handler = nav_selected.clone();
    let set_nav_selected_for_handler = set_nav_selected.clone();
    let controller_for_handler = Arc::clone(&controller);

    // Ctrl+W 关闭窗口：挂在根元素上且不指定 ScopeOwner，
    // 因此在窗口内容范围内全局生效（焦点在文本框等控件里时同样有效）。
    let close_accel = KeyboardAccelerator::new(
        VirtualKey::W,
        VirtualKeyModifiers::Control,
        window_utils::close_window,
    );
    Element::from(
        NavigationView::new(nav_items, body)
            .selected_tag(&nav_selected)
            .back_button_visible(false)
            .on_selection_changed(move |tag: String| {
                log::info!("NavigationView selection changed, tag={tag:?}");
                if tag == NAV_TAG_HOME {
                    window_utils::cancel_hotkey_capture();
                    let mut c = controller_for_handler.lock();
                    c.begin_settings_edit();
                    set_nav_selected_for_handler.call(NAV_TAG_HOME.to_string());
                } else if tag == NAV_TAG_GITHUB {
                    open_url_in_browser(GITHUB_REPO_URL);
                    // 点击 GitHub 项不真正切换页面，弹回之前选中的项
                    set_nav_selected_for_handler.call(nav_selected_for_handler.clone());
                } else {
                    window_utils::cancel_hotkey_capture();
                    let mut c = controller_for_handler.lock();
                    c.begin_settings_edit();
                    set_nav_selected_for_handler.call(NAV_TAG_SETTINGS.to_string());
                }
            })
            .on_pane_open_changed(move |open: bool| {
                // 框架加载模板时把 IsPaneOpen 强制改回默认值（展开）：那是框架
                // 的重置、不是用户的操作，不能写进配置，也不能让它顶掉配置里
                // 记住的状态。这里只把声明值让位成原生值，再由上面的 effect
                // 收敛回期望状态。
                if pane_override_pending.swap(false, Ordering::SeqCst) && open {
                    log::info!("NavigationView pane reset by framework at startup");
                    set_pane_declared_for_pane.call(open);
                    return;
                }
                // 用户点击三横线：采纳并持久化。
                {
                    let mut c = controller_for_pane.lock();
                    c.set_nav_pane_expanded(open);
                }
                set_nav_expanded_for_pane.call(open);
                set_pane_declared_for_pane.call(open);
            })
            .settings_visible(true)
            .pane_title("AudioRouter")
            .pane_display_mode(NavigationViewPaneDisplayMode::Left)
            .pane_open(pane_open)
            .open_pane_length(240.0),
    )
    .keyboard_accelerator(close_accel)
}

fn home_page(
    controller: Arc<Mutex<AppController>>,
    i18n: app_core::i18n::I18n,
    make_setter: impl Fn() + Clone + Send + 'static,
) -> Element {
    let c = controller.lock();
    // 设备数据只克隆一次。原来这里克隆了两遍整份设备列表（源设备一份、
    // 目标设备一份），每帧都要把每个设备的多个 String 复制一遍。
    let all_devices: Vec<_> = c.devices.clone();
    let is_running = c.is_running;
    let status_text = c.status_text.clone();
    let status_level = c.status_level;
    let selected_source_id = c.selected_source.clone();
    let start_block = c.start_block();
    let enabled_output_count = c.enabled_output_count();
    drop(c);

    // 输出设备行（排除源设备本身）。这里借用 all_devices，不再逐个深拷贝。
    let output_devices: Vec<_> = all_devices
        .iter()
        .filter(|d| Some(d.id.as_str()) != selected_source_id.as_deref())
        .collect();

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
    let source_device_names: Vec<String> = all_devices
        .iter()
        .map(|d| d.friendly_name.clone())
        .collect();
    // 下拉框的选择回调是 'static 的，必须自己持有一份数据；
    // 但它只需要设备 id，没必要把整条 DeviceInfo 再克隆一遍。
    let source_device_ids: Vec<String> = all_devices.iter().map(|d| d.id.clone()).collect();
    let selected_source_index = all_devices
        .iter()
        .position(|d| Some(&d.id) == selected_source_id.as_ref())
        .map(|i| i as i32)
        .unwrap_or(-1);

    let source_combo = {
        let controller_clone = Arc::clone(&controller);
        let refresh = make_setter.clone();
        let device_ids = source_device_ids;
        ComboBox::new(source_device_names)
            .selected_index(selected_source_index)
            // 未选择源设备时显示占位提示，而不是一个空白下拉框。
            .placeholder_text(i18n.t("SourcePlaceholder"))
            .on_selection_changed(move |index| {
                if let Some(device_id) = device_ids.get(index as usize) {
                    let mut c = controller_clone.lock();
                    c.select_source_device(device_id.clone());
                    refresh();
                }
            })
    };

    // 每行的勾选状态与声道模式都来自配置：整张表读一次就够，
    // 原来每一行都要重新加锁 + 读一遍配置。
    let output_states: Vec<(bool, i32)> = {
        let c = controller.lock();
        let handle = c.config_manager.handle();
        let cfg = handle.read();
        output_devices
            .iter()
            .map(|device| {
                let output = cfg.outputs.iter().find(|o| o.device_id == device.id);
                let enabled = output.map(|o| o.enabled).unwrap_or(false);
                let mode = output
                    .and_then(|o| o.channel_mode.as_deref())
                    .map(|s| ChannelMode::from_config(Some(s)))
                    .unwrap_or(ChannelMode::Stereo);
                (enabled, mode as i32)
            })
            .collect()
    };

    let output_items: Vec<Element> = output_devices
        .into_iter()
        .zip(output_states)
        .map(|(device, (enabled, selected_mode_index))| {
            let device_id = device.id.clone();

            // 当前选中模式对应的处理逻辑说明,用作 ComboBox 悬浮提示。
            // 渲染时由 make_setter 触发刷新,选择变更后 tooltip 会随重渲染更新。
            let selected_desc = channel_mode_descriptions
                .get(selected_mode_index as usize)
                .cloned()
                .unwrap_or_default();

            // 试听按钮：在该设备上播放一段提示音，用来确认「哪个是哪个音箱」。
            // 播放是阻塞操作，放到后台线程执行，结果回写到状态通道。
            let test_button = {
                let controller_clone = Arc::clone(&controller);
                let refresh = make_setter.clone();
                let device_id = device_id.clone();
                let device_name = device.friendly_name.clone();
                button(i18n.t("TestSound")).subtle().on_click(move || {
                    let controller = Arc::clone(&controller_clone);
                    let refresh = refresh.clone();
                    let device_id = device_id.clone();
                    let device_name = device_name.clone();
                    std::thread::spawn(move || {
                        {
                            let mut c = controller.lock();
                            let text = c
                                .i18n
                                .t("TestSoundPlaying")
                                .replace("{device}", &device_name);
                            c.set_status(StatusLevel::Idle, text);
                        }
                        refresh();

                        let result = audio_core::test_tone::play(&device_id);
                        {
                            let mut c = controller.lock();
                            match result {
                                Ok(()) => {
                                    let text = c.i18n.t("TestSoundDone").to_string();
                                    c.set_status(StatusLevel::Ok, text);
                                }
                                Err(e) => {
                                    let text = c
                                        .i18n
                                        .t("TestSoundFailed")
                                        .replace("{error}", &e.to_string());
                                    c.set_status(StatusLevel::Error, text);
                                }
                            }
                        }
                        refresh();
                    });
                })
            };

            // 设备名这一格：正在输出时在名字后面跟一个标记，
            // 避免「勾了但没声音」时无从判断是哪个设备在工作。
            let mut name_cells: Vec<Element> =
                vec![Element::from(text_block(device.friendly_name.clone()))];
            if is_running && enabled {
                name_cells.push(Element::from(
                    text_block(i18n.t("Outputting")).font_size(12.0),
                ));
            }
            let name_cell = Element::from(hstack(name_cells).spacing(8.0))
                .vertical_alignment(VerticalAlignment::Center);

            // 每行四列：勾选框 | 设备名 | 声道模式 | 试听。
            // 声道模式列固定宽度，所有设备的下拉框才对得齐；
            // 设备名列吃掉剩余空间，试听按钮始终贴在最右边。
            let row = Element::from(
                grid((
                    Element::from({
                        let controller_clone = Arc::clone(&controller);
                        let refresh = make_setter.clone();
                        let device_id = device_id.clone();
                        check_box(enabled).on_checked(move |checked| {
                            let mut c = controller_clone.lock();
                            c.set_output_enabled(&device_id, checked);
                            refresh();
                        })
                    })
                    .vertical_alignment(VerticalAlignment::Center)
                    .grid_column(0),
                    name_cell.grid_column(1),
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
                                let mut c = controller_clone.lock();
                                c.set_output_channel_mode(&device_id, mode);
                                refresh();
                            })
                    })
                    // 声道模式的说明只以悬浮提示出现，不再常驻占一行。
                    .tooltip_with(hover_hint(&selected_desc))
                    .vertical_alignment(VerticalAlignment::Center)
                    .grid_column(2),
                    Element::from(test_button)
                        .vertical_alignment(VerticalAlignment::Center)
                        .grid_column(3),
                ))
                .columns([
                    GridLength::Auto,
                    GridLength::STAR,
                    GridLength::Pixel(150.0),
                    GridLength::Auto,
                ])
                .column_spacing(12.0),
            );

            // 设备列表会随热插拔增减，给每一行一个稳定的 key：
            // 框架对无 key 的兄弟节点按位置复用控件，列表一变长就会
            // 把不同设备的行错配到别的设备上。
            // 行与行之间用一条细分隔线区分，比单纯拉大间距更整齐。
            Element::from(
                border(row)
                    .border_brush(ThemeRef::DividerStroke)
                    .border_thickness(Thickness {
                        left: 0.0,
                        top: 0.0,
                        right: 0.0,
                        bottom: 1.0,
                    })
                    .padding(Thickness {
                        left: 4.0,
                        top: 8.0,
                        right: 4.0,
                        bottom: 8.0,
                    }),
            )
            .with_key(format!("home.output.{device_id}"))
        })
        .collect();

    let toggle_controller = Arc::clone(&controller);
    let toggle_refresh = make_setter.clone();
    let toggle_btn = if is_running {
        button(i18n.t("Stop")).accent().on_click(move || {
            let mut c = toggle_controller.lock();
            c.stop_routing();
            toggle_refresh();
        })
    } else {
        button(i18n.t("Start"))
            .accent()
            .enabled(start_block.is_none())
            .on_click(move || {
                let mut c = toggle_controller.lock();
                c.start_routing();
                toggle_refresh();
            })
    };

    let batch_controller = Arc::clone(&controller);
    let batch_refresh = make_setter.clone();
    let batch_buttons = hstack((
        Element::from(button(i18n.t("SelectAll")).on_click({
            let controller = Arc::clone(&batch_controller);
            let refresh = batch_refresh.clone();
            move || {
                controller.lock().set_all_outputs_enabled(true);
                refresh();
            }
        })),
        Element::from(button(i18n.t("ClearAll")).on_click({
            let controller = Arc::clone(&batch_controller);
            let refresh = batch_refresh.clone();
            move || {
                controller.lock().set_all_outputs_enabled(false);
                refresh();
            }
        })),
    ))
    .spacing(8.0);

    let start_hint = start_block.map(|block| {
        let key = match block {
            StartBlock::NoDevices => "StartHintNoDevices",
            StartBlock::NoSource => "StartHintNoSource",
            StartBlock::NoOutput => "StartHintNoOutput",
        };
        i18n.t(key).to_string()
    });

    // 状态提示条：按语义级别显示成 InfoBar，比一行灰色小字更容易被注意到。
    // Idle 级别（就绪、设备列表已更新等）不打扰用户，用一行小字即可。
    let status_bar: Option<Element> = match status_level {
        StatusLevel::Idle => None,
        StatusLevel::Ok => Some(Element::from(
            InfoBar::new(i18n.t("StatusTitleOk"))
                .message(status_text.clone())
                .severity(InfoBarSeverity::Success)
                .is_closable(false),
        )),
        StatusLevel::Warning => Some(Element::from(
            InfoBar::new(i18n.t("StatusTitleWarning"))
                .message(status_text.clone())
                .severity(InfoBarSeverity::Warning)
                .is_closable(false),
        )),
        StatusLevel::Error => Some(Element::from(
            InfoBar::new(i18n.t("StatusTitleError"))
                .message(status_text.clone())
                .severity(InfoBarSeverity::Error)
                .is_closable(false),
        )),
    };

    // 首页的子节点数量会随「启动前置条件提示」「状态提示条」变化。
    // 框架默认按位置复用控件，位置一变后面所有控件都会错位，
    // 所以这里给每个兄弟节点一个稳定的 key，改走 keyed 差分。
    let mut page: Vec<Element> = Vec::new();
    page.push(
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
        )
        .with_key("home.title"),
    );
    page.push(Element::from(text_block(i18n.t("SourceDevice")).bold()).with_key("home.source"));
    page.push(Element::from(source_combo).with_key("home.source_combo"));
    page.push(
        Element::from(
            hstack((
                Element::from(text_block(i18n.t("OutputDevices")).bold()),
                Element::from(
                    text_block(
                        i18n.t("SelectedOutputs")
                            .replace("{count}", &enabled_output_count.to_string()),
                    )
                    .font_size(12.0)
                    .opacity(0.7),
                ),
                Element::from(batch_buttons),
            ))
            .spacing(12.0),
        )
        .with_key("home.outputs_header"),
    );
    page.push(
        Element::from(
            // 行之间已经有分隔线，列表本身不再额外加间距。
            scroll_viewer(vstack(output_items).spacing(0.0))
                .vertical_scroll_bar_visibility(ScrollBarVisibility::Auto),
        )
        .with_key("home.outputs"),
    );
    if let Some(hint) = start_hint {
        page.push(Element::from(text_block(hint).font_size(13.0)).with_key("home.start_hint"));
    }
    if let Some(bar) = status_bar {
        page.push(bar.with_key("home.status"));
    } else if !status_text.is_empty() {
        page.push(
            Element::from(text_block(status_text.clone()).font_size(12.0).opacity(0.7))
                .with_key("home.status"),
        );
    }
    page.push(
        Element::from(
            hstack((Element::from(toggle_btn),)).horizontal_alignment(HorizontalAlignment::Right),
        )
        .with_key("home.actions"),
    );

    Element::from(vstack(page).spacing(8.0).padding(Thickness {
        left: 16.0,
        top: 12.0,
        right: 16.0,
        bottom: 16.0,
    }))
}

#[allow(clippy::too_many_arguments)]
fn settings_page(
    controller: Arc<Mutex<AppController>>,
    i18n: app_core::i18n::I18n,
    make_setter: impl Fn() + Clone + Send + 'static,
    set_nav_selected: SetState<String>,
    theme_choice: ThemeChoice,
    set_theme_choice: SetState<ThemeChoice>,
    update_state: Arc<Mutex<UpdateState>>,
    settings_error: Option<String>,
    set_settings_error: SetState<Option<String>>,
    hotkey_status: Option<String>,
) -> Element {
    let (
        start_with_windows,
        start_minimized,
        auto_route,
        close_to_tray,
        auto_update_check,
        global_hotkey,
        hotkey_error,
        lang_index,
        theme_index,
        backdrop_index,
    ) = {
        let c = controller.lock();
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
            draft.global_hotkey.clone(),
            c.hotkey_error.clone(),
            lang_idx,
            theme_idx,
            backdrop_idx,
        )
    };

    // 草稿与已保存配置是否一致：不一致说明有未保存的修改。
    let dirty = {
        let c = controller.lock();
        c.settings_dirty()
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

    // 快捷键录制：点「录制」进入捕获状态，之后直接按下组合键即可。
    let capturing = window_utils::hotkey_capture_active();
    let record_click = {
        let refresh = make_setter.clone();
        move || {
            if window_utils::hotkey_capture_active() {
                window_utils::cancel_hotkey_capture();
            } else {
                window_utils::arm_hotkey_capture();
            }
            refresh();
        }
    };
    let record_button = {
        let btn = button(if capturing {
            i18n.t("HotkeyRecording")
        } else {
            i18n.t("HotkeyRecord")
        });
        let btn = if capturing { btn.accent() } else { btn };
        btn.on_click(record_click)
    };
    let clear_click = {
        let controller = Arc::clone(&controller);
        let refresh = make_setter.clone();
        move || {
            controller.lock().draft_general.global_hotkey = String::new();
            refresh();
        }
    };
    let clear_button = button(i18n.t("HotkeyClear")).on_click(clear_click);

    // 快捷键下方的提示：错误优先（刚捕获失败时最需要看到），
    // 其次是录制中的引导，最后是常规说明。
    let hotkey_hint = if let Some(status) = hotkey_status.as_ref() {
        status.clone()
    } else if capturing {
        i18n.t("HotkeyRecordHint").to_string()
    } else {
        i18n.t("HotkeyHint").to_string()
    };

    let back_ctrl = controller.clone();
    let back_nav = set_nav_selected.clone();
    let cancel_btn = button(i18n.t("Cancel")).on_click(move || {
        // 离开设置页时结束快捷键录制，避免继续吞掉键盘输入。
        window_utils::cancel_hotkey_capture();
        let mut c = back_ctrl.lock();
        c.begin_settings_edit();
        back_nav.call(NAV_TAG_HOME.to_string());
    });

    let save_controller = Arc::clone(&controller);
    let save_refresh = make_setter.clone();
    let save_nav = set_nav_selected.clone();
    let set_error = set_settings_error.clone();
    let save_i18n = i18n.clone();
    let save_btn = button(i18n.t("Save")).accent().on_click(move || {
        // 保存时结束录制，避免之后的按键被当成快捷键。
        window_utils::cancel_hotkey_capture();
        let mut c = save_controller.lock();

        // 先单独校验全局热键，给出明确的格式错误提示，
        // 而不是笼统的「保存失败」。
        let hotkey_input = c.draft_general.global_hotkey.trim().to_string();
        if !hotkey_input.is_empty() {
            if let Err(error) = app_core::hotkey::parse(&hotkey_input) {
                let text = save_i18n
                    .t("HotkeyInvalid")
                    .replace("{error}", &error.to_string());
                set_error.call(Some(text));
                return;
            }
        }

        // 保存设置：控制器会把新的全局热键同步给常驻服务（热键由服务进程
        // 注册），并让服务刷新托盘菜单（语言变了托盘文案也要跟着变）。
        if let Err(error) = c.save_general_config() {
            log::warn!("Failed to save general settings: {error}");
            let text = save_i18n.t("SaveFailed").replace("{error}", &error);
            set_error.call(Some(text));
            return;
        }

        set_error.call(None);
        save_refresh();
        save_nav.call(NAV_TAG_HOME.to_string());
    });

    // 设置页同样给每个兄弟节点一个稳定的 key：错误提示和「有未保存的修改」
    // 是按条件插入的，没有 key 时框架会按位置复用控件，导致后面的
    // 选项卡片和按钮被挤到错位的位置。
    let mut page: Vec<Element> = Vec::new();
    page.push(Element::from(title(i18n.t("SettingsTitle"))).with_key("settings.title"));

    // 保存失败 / 热键被占用的内联错误提示，不再静默失败。
    if let Some(error) = settings_error.as_ref() {
        page.push(
            Element::from(
                InfoBar::new(i18n.t("StatusTitleError"))
                    .message(error.clone())
                    .severity(InfoBarSeverity::Error)
                    .is_closable(false),
            )
            .with_key("settings.error"),
        );
    }

    // 热键在服务进程里注册，被别的程序占用时错误由服务推回来，
    // 在这里就地显示出来。
    if let Some(error) = hotkey_error.as_ref() {
        page.push(
            Element::from(
                InfoBar::new(i18n.t("StatusTitleError"))
                    .message(error.clone())
                    .severity(InfoBarSeverity::Error)
                    .is_closable(false),
            )
            .with_key("settings.hotkey_error"),
        );
    }

    // 有未保存的修改时给出提示，避免改了一半切走就丢了。
    if dirty {
        page.push(
            Element::from(
                text_block(i18n.t("UnsavedChanges"))
                    .font_size(12.0)
                    .opacity(0.7),
            )
            .with_key("settings.dirty"),
        );
    }

    page.push(Element::from(
        border(
            vstack((
                Element::from(
                    check_box(start_with_windows)
                        .content(i18n.t("StartWithWindows"))
                        .on_checked({
                            let controller_clone = Arc::clone(&controller);
                            move |checked| {
                                let mut c = controller_clone.lock();
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
                                let mut c = controller_clone.lock();
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
                                let mut c = controller_clone.lock();
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
                                let mut c = controller_clone.lock();
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
                                let mut c = controller_clone.lock();
                                c.draft_general.auto_update_check = checked;
                            }
                        }),
                ),
                // 「打开窗口快捷键」：可以直接输入，也可以点「录制」
                // 后直接按下组合键（窗口过程会把按键翻译成 Ctrl+Alt+M）。
                Element::from(
                    vstack((
                        Element::from(text_block(i18n.t("GlobalHotkey")).bold()),
                        Element::from(
                            grid((
                                Element::from(
                                    text_box(global_hotkey)
                                        .placeholder_text(i18n.t("HotkeyPlaceholder"))
                                        .on_text_changed({
                                            let controller_clone = Arc::clone(&controller);
                                            move |value| {
                                                controller_clone
                                                    .lock()
                                                    .draft_general
                                                    .global_hotkey = value;
                                            }
                                        }),
                                )
                                .grid_column(0),
                                Element::from(record_button).grid_column(1),
                                Element::from(clear_button).grid_column(2),
                            ))
                            .columns([GridLength::STAR, GridLength::Auto, GridLength::Auto])
                            .column_spacing(8.0),
                        ),
                        Element::from(text_block(hotkey_hint).font_size(12.0).opacity(0.7)),
                    ))
                    .spacing(4.0),
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
                                        let mut c = controller_clone.lock();
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
                                        let mut c = controller_clone.lock();
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
                                            config::config::Backdrop::MicaAlt => {
                                                WrBackdrop::MicaAlt
                                            }
                                            config::config::Backdrop::Acrylic => {
                                                WrBackdrop::Acrylic
                                            }
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
        .corner_radius(8.0)
        .with_key("settings.general"),
    ));

    #[allow(clippy::useless_conversion)]
    page.push(
        Element::from(build_update_section(
            Arc::clone(&update_state),
            i18n.clone(),
            make_setter.clone(),
        ))
        .with_key("settings.update"),
    );

    #[allow(clippy::useless_conversion)]
    page.push(Element::from(build_diagnostics_section(&i18n)).with_key("settings.diagnostics"));

    page.push(
        Element::from(
            hstack((Element::from(cancel_btn), Element::from(save_btn)))
                .spacing(8.0)
                .horizontal_alignment(HorizontalAlignment::Right),
        )
        .with_key("settings.actions"),
    );

    // 设置页内容较长（选项 + 更新 + 诊断），整体放进滚动容器，
    // 窗口不够高时也能滚到下面的内容。
    Element::from(
        scroll_viewer(vstack(page).spacing(12.0).padding(Thickness {
            left: 16.0,
            top: 12.0,
            right: 16.0,
            bottom: 16.0,
        }))
        .vertical_scroll_bar_visibility(ScrollBarVisibility::Auto),
    )
}

/// 构建设置页里的诊断区域：打开配置目录 / 日志目录，并显示当前版本号。
///
/// 出问题时用户不用自己去翻应用数据目录。
fn build_diagnostics_section(i18n: &app_core::i18n::I18n) -> Element {
    let config_dir = app_core::paths::app_config_dir();
    let log_dir = config_dir.join("logs");
    let version = crate::update::current_version();

    let open_config = button(i18n.t("OpenConfigFolder")).on_click({
        let path = config_dir.clone();
        move || open_folder_in_explorer(&path)
    });
    let open_logs = button(i18n.t("OpenLogFolder")).on_click({
        let path = log_dir.clone();
        move || open_folder_in_explorer(&path)
    });

    Element::from(
        border(
            vstack((
                Element::from(text_block(i18n.t("DiagnosticsSection")).bold()),
                Element::from(
                    hstack((
                        Element::from(open_config),
                        Element::from(open_logs),
                        Element::from(
                            text_block(format!("{}: v{version}", i18n.t("VersionLabel")))
                                .font_size(12.0)
                                .opacity(0.7),
                        ),
                    ))
                    .spacing(8.0),
                ),
            ))
            .spacing(8.0),
        )
        .padding(Thickness::uniform(16.0))
        .background(ThemeRef::LayerFill)
        .corner_radius(8.0),
    )
}

/// 在文件管理器中打开一个目录。
fn open_folder_in_explorer(path: &std::path::Path) {
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("explorer.exe").arg(path).spawn();
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    }
}

/// 构建设置页面中的更新区域，根据 UpdateState 展示不同 UI。
///
/// 后台线程只修改共享的 UpdateState，完成后调用 refresh 句柄直接请求
/// 重绘，不再依赖主循环定时器。
fn build_update_section(
    update_state: Arc<Mutex<UpdateState>>,
    i18n: app_core::i18n::I18n,
    refresh: impl Fn() + Clone + Send + 'static,
) -> Element {
    let state = update_state.lock().clone();
    let current_ver = crate::update::current_version();

    let header = hstack((
        Element::from(text_block(i18n.t("CheckForUpdates")).bold()),
        Element::from(text_block(format!("v{current_ver}")).font_size(12.0)),
    ))
    .spacing(8.0);

    // 启动一次后台检查：立即切到 Checking 状态，检查结束后请求重绘。
    let start_check = {
        let state = Arc::clone(&update_state);
        let refresh = refresh.clone();
        move || {
            *state.lock() = UpdateState::Checking;
            // 立即重绘，让用户看到“检查中”。
            refresh();

            let sc = Arc::clone(&state);
            let rr = refresh.clone();
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
                *sc.lock() = new_state;
                // 检查完成，直接请求重绘。
                rr();
            });
        }
    };

    let body: Element = match state {
        UpdateState::Idle => {
            let btn = button(i18n.t("CheckForUpdates")).on_click(start_check);
            Element::from(btn)
        }
        UpdateState::Checking => Element::from(
            vstack((
                Element::from(ProgressBar::indeterminate()),
                Element::from(text_block(i18n.t("CheckingForUpdates")).font_size(12.0)),
            ))
            .spacing(8.0),
        ),
        UpdateState::UpToDate => {
            let btn = button(i18n.t("CheckForUpdates")).on_click(start_check);
            Element::from(
                vstack((
                    Element::from(text_block(i18n.t("UpToDate"))),
                    Element::from(btn),
                ))
                .spacing(8.0),
            )
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
            let download_refresh = refresh.clone();
            let download_btn = button(i18n.t("DownloadUpdate")).accent().on_click(move || {
                *state_clone.lock() = UpdateState::Downloading {
                    downloaded: 0,
                    total: file_size,
                };
                download_refresh();

                let sc = Arc::clone(&state_clone);
                let url2 = url.clone();
                let progress_refresh = download_refresh.clone();
                std::thread::spawn(move || {
                    // 进度按百分比变化触发重绘，避免每个数据块都刷新 UI。
                    let last_percent = std::cell::Cell::new(u64::MAX);
                    let sc_inner = Arc::clone(&sc);
                    let progress_cb_refresh = progress_refresh.clone();
                    let result = crate::update::download_installer(&url2, move |d, t| {
                        let percent = d.saturating_mul(100).checked_div(t).unwrap_or(0);
                        if percent == last_percent.get() {
                            return;
                        }
                        last_percent.set(percent);

                        let mut s = sc_inner.lock();
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
                        drop(s);
                        progress_cb_refresh();
                    });
                    let new_state = match result {
                        Ok(path) => UpdateState::Ready(path),
                        Err(e) => UpdateState::Failed(e.to_string()),
                    };
                    *sc.lock() = new_state;
                    // 下载结束（或失败），请求重绘。
                    progress_refresh();
                });
            });

            let notes_el = if release_notes.is_empty() {
                Element::from(text_block(""))
            } else {
                let notes = release_notes
                    .lines()
                    .take(10)
                    .collect::<Vec<_>>()
                    .join("\n");
                Element::from(text_block(notes).font_size(12.0).opacity(0.7))
            };

            Element::from(
                vstack((
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
                .spacing(8.0),
            )
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

            Element::from(
                vstack((
                    Element::from(ProgressBar::new(progress).range(0.0, 100.0)),
                    Element::from(text_block(progress_text).font_size(12.0)),
                ))
                .spacing(8.0),
            )
        }
        UpdateState::Ready(path) => {
            let install_btn = button(i18n.t("InstallAndRestart"))
                .accent()
                .on_click(move || {
                    crate::update::launch_installer_and_quit(&path);
                });
            Element::from(
                vstack((
                    Element::from(text_block(i18n.t("UpdateReady"))),
                    Element::from(install_btn),
                ))
                .spacing(8.0),
            )
        }
        UpdateState::Failed(err) => {
            let err_text = i18n.t("UpdateFailed").replace("{error}", &err);
            let btn = button(i18n.t("CheckForUpdates")).on_click(start_check);
            Element::from(
                vstack((
                    Element::from(text_block(err_text).font_size(12.0)),
                    Element::from(btn),
                ))
                .spacing(8.0),
            )
        }
    };

    Element::from(
        border(vstack((Element::from(header), body)).spacing(12.0))
            .padding(Thickness::uniform(16.0))
            .background(ThemeRef::LayerFill)
            .corner_radius(8.0),
    )
}
