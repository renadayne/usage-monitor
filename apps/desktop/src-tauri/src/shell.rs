use adapters::codex::Surface;
use collector::{
    export::HistoryExport,
    query::{EventCursor, SessionCursor, SessionDetail},
    Store,
};
use normalized_schema::Phase;
use policy_engine::Policy;
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::Mutex,
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager,
};
use usage_monitor_desktop::{profile::ensure_profile, DesktopService};

struct ApplicationState {
    service: Mutex<DesktopService>,
    server: Mutex<Option<observer_runtime::ObserverServer>>,
    address: String,
    setup_info: serde_json::Value,
    _tray: tauri::tray::TrayIcon,
}

async fn with_service<T: Send + 'static>(
    app: AppHandle,
    action: impl FnOnce(&mut DesktopService) -> Result<T, &'static str> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<ApplicationState>();
        let mut service = state
            .service
            .lock()
            .map_err(|_| "Dữ liệu desktop đang lỗi. Hãy khởi động lại ứng dụng.".to_owned())?;
        action(&mut service).map_err(str::to_owned)
    })
    .await
    .map_err(|_| "Không hoàn tất tác vụ desktop.".to_owned())?
}

#[tauri::command]
async fn dashboard(
    app: AppHandle,
    cursor: Option<SessionCursor>,
) -> Result<serde_json::Value, String> {
    let address = {
        let state = app.state::<ApplicationState>();
        let running = state
            .server
            .lock()
            .map(|server| server.as_ref().is_some_and(|server| server.is_running()))
            .unwrap_or(false);
        running.then(|| state.address.clone())
    };
    with_service(app, move |service| {
        service.dashboard(cursor.as_ref(), address.as_deref())
    })
    .await
}
#[tauri::command]
async fn session_detail(
    app: AppHandle,
    session_id: String,
    cursor: Option<EventCursor>,
) -> Result<SessionDetail, String> {
    with_service(app, move |service| {
        service.session_detail(&session_id, cursor.as_ref())
    })
    .await
}
#[tauri::command]
async fn change_phase(
    app: AppHandle,
    session_id: String,
    expected_epoch: i64,
    phase: Phase,
) -> Result<(), String> {
    with_service(app, move |service| {
        service.change_phase(&session_id, expected_epoch, phase)
    })
    .await
}
#[tauri::command]
async fn list_policies(app: AppHandle) -> Result<Vec<Policy>, String> {
    with_service(app, |service| service.list_policies()).await
}
#[tauri::command]
async fn save_policy(app: AppHandle, policy: Policy) -> Result<(), String> {
    with_service(app, move |service| service.save_policy(&policy)).await
}
#[tauri::command]
async fn export_history(app: AppHandle, days: u8) -> Result<HistoryExport, String> {
    with_service(app, move |service| service.export_history(days)).await
}
#[tauri::command]
async fn preview_setup(
    current_json: String,
    helper_command: String,
) -> Result<usage_monitor_desktop::SetupPreview, String> {
    tauri::async_runtime::spawn_blocking(move || {
        usage_monitor_desktop::preview_setup(&current_json, &helper_command).map_err(str::to_owned)
    })
    .await
    .map_err(|_| "Không tạo được preview.".to_owned())?
}
#[tauri::command]
fn setup_info(app: AppHandle) -> serde_json::Value {
    app.state::<ApplicationState>().setup_info.clone()
}

fn show(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}
fn tray_image() -> tauri::image::Image<'static> {
    let mut pixels = Vec::with_capacity(32 * 32 * 4);
    for y in 0..32 {
        for x in 0..32 {
            let bar = ((8..11).contains(&x) && (15..25).contains(&y))
                || ((14..17).contains(&x) && (8..25).contains(&y))
                || ((20..23).contains(&x) && (18..25).contains(&y));
            pixels.extend_from_slice(if bar {
                &[53, 100, 71, 255]
            } else {
                &[209, 237, 206, 255]
            });
        }
    }
    tauri::image::Image::new_owned(pixels, 32, 32)
}

pub fn run() {
    let app=tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app,_,_|show(app)))
        .setup(|app|{
            let data=app.path().app_data_dir()?;let runtime=data.join("runtime");
            let address=SocketAddr::from((Ipv4Addr::LOCALHOST,4319));
            let vscode=ensure_profile(&runtime,Surface::VsCodeExtension,address).map_err(std::io::Error::other)?;
            let desktop=ensure_profile(&runtime,Surface::StandaloneDesktop,address).map_err(std::io::Error::other)?;
            let db=data.join("usage-monitor.sqlite3");
            let service=DesktopService::open(&db).map_err(std::io::Error::other)?;
            let store=Store::open(&db)?;
            let bindings=vec![vscode.binding().map_err(std::io::Error::other)?,desktop.binding().map_err(std::io::Error::other)?];
            let server=tauri::async_runtime::block_on(observer_runtime::start(store,bindings,4319)).map_err(std::io::Error::other)?;
            let helper=std::env::current_exe()?.with_file_name("aum-observer.exe");
            let info=serde_json::json!({"helperExecutable":helper.to_string_lossy(),"helperExists":helper.is_file(),"profiles":[
                {"surface":"Codex · VS Code","configPath":vscode.config_path().to_string_lossy()},
                {"surface":"Codex · Desktop","configPath":desktop.config_path().to_string_lossy()}]});
            let open=MenuItem::with_id(app,"show","Mở Usage Monitor",true,None::<&str>)?;
            let quit=MenuItem::with_id(app,"quit","Thoát và dừng bộ thu",true,None::<&str>)?;
            let menu=Menu::with_items(app,&[&open,&quit])?;
            let tray=TrayIconBuilder::with_id("usage-monitor").show_menu_on_left_click(false).icon(tray_image()).tooltip("Usage Monitor · Observer")
                .menu(&menu).on_menu_event(|app,event|match event.id.as_ref() {"show"=>show(app),"quit"=>app.exit(0),_=>{}})
                .on_tray_icon_event(|tray,event|if matches!(event,TrayIconEvent::Click{button:MouseButton::Left,button_state:MouseButtonState::Up,..}) {show(tray.app_handle());})
                .build(app)?;
            app.manage(ApplicationState{service:Mutex::new(service),server:Mutex::new(Some(server)),address:address.to_string(),setup_info:info,_tray:tray});
            Ok(())
        })
        .on_window_event(|window,event|if let tauri::WindowEvent::CloseRequested{api,..}=event {let _=window.hide();api.prevent_close();})
        .invoke_handler(tauri::generate_handler![dashboard,session_detail,change_phase,list_policies,save_policy,export_history,preview_setup,setup_info])
        .build(tauri::generate_context!())
        .expect("Không khởi động được Usage Monitor. Kiểm tra app data hoặc cổng 127.0.0.1:4319.");
    app.run(|app, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            if let Some(state) = app.try_state::<ApplicationState>() {
                let server = state
                    .server
                    .lock()
                    .ok()
                    .and_then(|mut server| server.take());
                if let Some(server) = server {
                    let _ = tauri::async_runtime::block_on(server.shutdown());
                }
            }
        }
    });
}
