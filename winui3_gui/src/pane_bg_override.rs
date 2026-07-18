//! 覆盖 NavigationView Pane 背景的 XAML 资源。
//!
//! WinUI 3 的 `XamlControlsResources` 提供的 `NavigationViewPaneBackground` brush
//! 引用系统级颜色(`{ThemeResource SystemColorWindowColor}`),不响应
//! `FrameworkElement.RequestedTheme`。要让 Pane 背景跟随应用主题,我们在应用启动
//! 早期向 `Application.Resources.MergedDictionaries` 插入一个自定义
//! ResourceDictionary(用 `XamlReader::Load` 从 XAML 解析),覆盖该 brush 为
//! 响应元素级主题的 ThemeDictionaries 形式。

use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::Once;

use windows_core::{GUID, HRESULT};

#[link(name = "combase", kind = "raw-dylib")]
extern "system" {
    fn RoGetActivationFactory(
        activatable_class_id: *mut u16,
        iid: *const GUID,
        factory: *mut *mut c_void,
    ) -> HRESULT;
    fn WindowsCreateString(
        source: *const u16,
        length: u32,
        string: *mut *mut u16,
    ) -> HRESULT;
    fn WindowsDeleteString(string: *mut u16) -> HRESULT;
}

// Microsoft.UI.Xaml.Markup.IXamlReaderStatics
const IID_IXAML_READER_STATICS: GUID = GUID::from_u128(0x82a4cd9e_435e_5aeb_8c4f_300cece45cae);
// Microsoft.UI.Xaml.IResourceDictionary
const IID_IRESOURCE_DICTIONARY: GUID = GUID::from_u128(0x1b690975_a710_5783_a6e1_15836f6186c2);
// Microsoft.UI.Xaml.IApplicationStatics
const IID_IAPPLICATION_STATICS: GUID = GUID::from_u128(0x4e0d09f5_4358_512c_a987_503b52848e95);

/// Pane 背景覆盖 XAML:用 ThemeDictionaries 同时为 Light/Dark 提供 brush。
const PANE_BG_XAML: &str = r##"<ResourceDictionary
    xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
    xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml">
    <ResourceDictionary.ThemeDictionaries>
        <ResourceDictionary x:Key="Light">
            <SolidColorBrush x:Key="NavigationViewPaneBackground" Color="#F3F3F3"/>
            <SolidColorBrush x:Key="NavigationViewDefaultPaneBackground" Color="#F3F3F3"/>
            <SolidColorBrush x:Key="NavigationViewExpandedPaneBackground" Color="#F3F3F3"/>
        </ResourceDictionary>
        <ResourceDictionary x:Key="Dark">
            <SolidColorBrush x:Key="NavigationViewPaneBackground" Color="#1F1F1F"/>
            <SolidColorBrush x:Key="NavigationViewDefaultPaneBackground" Color="#1F1F1F"/>
            <SolidColorBrush x:Key="NavigationViewExpandedPaneBackground" Color="#1F1F1F"/>
        </ResourceDictionary>
    </ResourceDictionary.ThemeDictionaries>
</ResourceDictionary>"##;

static INIT_ONCE: Once = Once::new();

/// 在应用启动早期调用一次,注入 Pane 背景覆盖资源。多次调用是幂等的。
pub fn install_pane_background_overrides() {
    INIT_ONCE.call_once(|| {
        if let Err(e) = do_install() {
            log::warn!("Failed to install NavigationView pane background overrides: {e:?}");
        }
    });
}

fn do_install() -> windows_core::Result<()> {
    unsafe {
        // 1. 用 XamlReader::Load 加载自定义 ResourceDictionary
        let rd_obj = load_xaml(PANE_BG_XAML)?;
        log::info!("Loaded pane background ResourceDictionary via XamlReader");

        // 2. 拿到 Application.Current().Resources
        let app_resources = get_application_resources()?;
        log::info!("Got Application.Resources");

        // 3. QI 到 IResourceDictionary,再拿 MergedDictionaries
        let rd_iface = query_interface(app_resources, &IID_IRESOURCE_DICTIONARY)?;
        let merged = get_merged_dictionaries(rd_iface)?;
        log::info!("Got MergedDictionaries");

        // 4. InsertAt(0, rd_obj),让我们的资源优先于 XamlControlsResources
        insert_at(merged, 0, rd_obj)?;
        log::info!("Inserted pane background override at index 0");

        // 5. 释放临时引用
        release(rd_obj);
        release(rd_iface);
        release(merged);
        // 注意: app_resources 不释放,因为它属于 Application
        Ok(())
    }
}

/// 通过 `RoGetActivationFactory` 获取指定 WinRT 类的静态工厂接口。
unsafe fn get_activation_factory(iid: &GUID, class_name: &str) -> windows_core::Result<*mut c_void> {
    let hs = create_hstring(class_name)?;
    let mut factory: *mut c_void = null_mut();
    let hr = RoGetActivationFactory(hs, iid, &mut factory);
    windows_delete_string(hs);
    if hr.is_err() {
        return Err(windows_core::Error::from(hr));
    }
    if factory.is_null() {
        return Err(windows_core::Error::empty());
    }
    Ok(factory)
}

/// `XamlReader.Load(xaml)` → 返回 IInspectable* (ResourceDictionary)。
/// IXamlReaderStatics Vtable: IInspectable(6) + Load(6)。
unsafe fn load_xaml(xaml: &str) -> windows_core::Result<*mut c_void> {
    let factory = get_activation_factory(&IID_IXAML_READER_STATICS, "Microsoft.UI.Xaml.Markup.XamlReader")?;

    let vtable = *(factory as *const *const usize);
    type LoadFn = unsafe extern "system" fn(*mut c_void, *mut u16, *mut *mut c_void) -> HRESULT;
    let load_fn: LoadFn = std::mem::transmute(*vtable.add(6));

    let hstring = create_hstring(xaml)?;
    let mut result: *mut c_void = null_mut();
    let hr = load_fn(factory, hstring, &mut result);
    windows_delete_string(hstring);
    release(factory);
    if hr.is_err() {
        return Err(windows_core::Error::from(hr));
    }
    if result.is_null() {
        return Err(windows_core::Error::empty());
    }
    Ok(result)
}

/// `Application.Current().Resources()` → 返回 IInspectable* (ResourceDictionary)。
/// IApplicationStatics Vtable: IInspectable(6) + get_Current(6)。
/// IApplication Vtable: IInspectable(6) + get_Resources(6)。
unsafe fn get_application_resources() -> windows_core::Result<*mut c_void> {
    let statics = get_activation_factory(&IID_IAPPLICATION_STATICS, "Microsoft.UI.Xaml.Application")?;

    // IApplicationStatics::get_Current(this, *mut IInspectable) -> HRESULT
    let statics_vtable = *(statics as *const *const usize);
    type GetFn = unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT;
    let current_fn: GetFn = std::mem::transmute(*statics_vtable.add(6));

    let mut app_obj: *mut c_void = null_mut();
    let hr = current_fn(statics, &mut app_obj);
    release(statics);
    if hr.is_err() {
        return Err(windows_core::Error::from(hr));
    }
    if app_obj.is_null() {
        return Err(windows_core::Error::empty());
    }

    // IApplication::get_Resources(this, *mut IInspectable) -> HRESULT
    let app_vtable = *(app_obj as *const *const usize);
    let resources_fn: GetFn = std::mem::transmute(*app_vtable.add(6));

    let mut result: *mut c_void = null_mut();
    let hr = resources_fn(app_obj, &mut result);
    release(app_obj);
    if hr.is_err() {
        return Err(windows_core::Error::from(hr));
    }
    if result.is_null() {
        return Err(windows_core::Error::empty());
    }
    Ok(result)
}

unsafe fn query_interface(obj: *mut c_void, iid: &GUID) -> windows_core::Result<*mut c_void> {
    let vtable = *(obj as *const *const usize);
    type QiFn = unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT;
    let qi: QiFn = std::mem::transmute(*vtable.add(0));
    let mut result: *mut c_void = null_mut();
    qi(obj, iid, &mut result).ok()?;
    Ok(result)
}

/// IResourceDictionary Vtable: IInspectable(6) + Source(6) + SetSource(7) + MergedDictionaries(8)。
unsafe fn get_merged_dictionaries(rd: *mut c_void) -> windows_core::Result<*mut c_void> {
    let vtable = *(rd as *const *const usize);
    type GetFn = unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT;
    let get_merged: GetFn = std::mem::transmute(*vtable.add(8));
    let mut result: *mut c_void = null_mut();
    get_merged(rd, &mut result).ok()?;
    Ok(result)
}

/// IVector<T> Vtable: IInspectable(6) + GetAt(6) + Size(7) + GetView(8) + IndexOf(9)
/// + SetAt(10) + InsertAt(11) + RemoveAt(12) + Append(13) + RemoveAtEnd(14) + Clear(15)
/// + GetMany(16) + ReplaceAll(17)。InsertAt 在槽位 11。
unsafe fn insert_at(vec: *mut c_void, index: u32, item: *mut c_void) -> windows_core::Result<()> {
    let vtable = *(vec as *const *const usize);
    type InsertAtFn = unsafe extern "system" fn(*mut c_void, u32, *mut c_void) -> HRESULT;
    let insert_at_fn: InsertAtFn = std::mem::transmute(*vtable.add(11));
    insert_at_fn(vec, index, item).ok()
}

unsafe fn release(obj: *mut c_void) {
    if obj.is_null() {
        return;
    }
    let vtable = *(obj as *const *const usize);
    type ReleaseFn = unsafe extern "system" fn(*mut c_void) -> u32;
    let release_fn: ReleaseFn = std::mem::transmute(*vtable.add(2));
    release_fn(obj);
}

/// 用 WindowsCreateString 创建真正的 HSTRING。
/// HSTRING 是引用计数的不可变字符串对象,不能直接用 *const u16 代替。
fn create_hstring(s: &str) -> windows_core::Result<*mut u16> {
    let wide: Vec<u16> = s.encode_utf16().collect();
    let mut hstring: *mut u16 = null_mut();
    unsafe { WindowsCreateString(wide.as_ptr(), wide.len() as u32, &mut hstring).ok()? }
    if hstring.is_null() {
        return Err(windows_core::Error::empty());
    }
    Ok(hstring)
}

unsafe fn windows_delete_string(s: *mut u16) {
    if !s.is_null() {
        let _ = WindowsDeleteString(s);
    }
}
