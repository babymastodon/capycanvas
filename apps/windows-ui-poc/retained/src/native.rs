use crate::{
    render::{self, Event, Transaction},
    scene::*,
    text::{Shaped, TextSystem},
};
use accesskit::{
    Action, ActionData, ActionHandler, ActionRequest, ActivationHandler, Node, NodeId, Role,
    TreeId, TreeInfo, TreeUpdate,
};
use accesskit_windows::{Adapter, QueuedEvents};
use arc_swap::ArcSwap;
use crossbeam_channel::{Receiver, Select, Sender, unbounded};
use poc_canvas::Canvas;
use poc_common::{
    AppOptions, PenSample,
    model::{Model, SLIDERS},
};
use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::{
            ClientToScreen, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO,
            MonitorFromWindow, ScreenToClient, UpdateWindow,
        },
        System::{
            Com::{COINIT_MULTITHREADED, CoInitializeEx},
            DataExchange::*,
            LibraryLoader::GetModuleHandleW,
            Memory::*,
            Performance::{QueryPerformanceCounter, QueryPerformanceFrequency},
            Threading::{GetCurrentThread, GetThreadTimes},
        },
        UI::{
            HiDpi::*,
            Input::{Ime::*, KeyboardAndMouse::*, Pointer::*},
            WindowsAndMessaging::*,
        },
    },
    core::{PCWSTR, w},
};

pub enum UiEvent {
    Ready,
    Size(u32, u32, f32),
    Pointer(Target, f32, f32, u8, i64),
    Character(char),
    Key(u32, bool, bool),
    Paste(String),
    Ime(String, bool),
    Access(ActionRequest),
    NeedRows(f32),
    ActivateAccess,
    DrawerSettled(f32),
    ScrollSettled(f32),
    WheelProperties(f32),
    ClosePopup,
    Shutdown,
}
struct Actions(Sender<UiEvent>);
impl ActionHandler for Actions {
    fn do_action(&mut self, request: ActionRequest) {
        let _ = self.0.try_send(UiEvent::Access(request));
    }
}
#[derive(Default)]
struct A11yTree {
    nodes: HashMap<NodeId, Node>,
    focus: Option<NodeId>,
}
struct Activation {
    tree: Arc<Mutex<A11yTree>>,
    ui: Sender<UiEvent>,
}
impl ActivationHandler for Activation {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        let tree = self.tree.lock().unwrap();
        if tree.nodes.is_empty() {
            let _ = self.ui.try_send(UiEvent::ActivateAccess);
            None
        } else {
            Some(TreeUpdate {
                nodes: tree
                    .nodes
                    .iter()
                    .map(|(id, node)| (*id, node.clone()))
                    .collect(),
                tree: Some(TreeInfo::new(NodeId(1))),
                tree_id: TreeId::ROOT,
                focus: tree.focus.unwrap_or(NodeId(1)),
            })
        }
    }
}
struct WindowState {
    ui: Sender<UiEvent>,
    comp: Sender<Event>,
    hits: Arc<ArcSwap<HitMap>>,
    canvas: Arc<OnceLock<Canvas>>,
    adapter: Arc<Mutex<Adapter>>,
    activation: Activation,
    accessibility_events: Receiver<QueuedEvents>,
    contacts: HashMap<u32, Target>,
    scale: f32,
    last_cpu: Instant,
    characters:Utf16Input,
    caption: CaptionPress,
    closing: bool,
}
const COPY_TEXT: u32 = WM_APP + 41;
const CLOSE_WINDOW: u32 = WM_APP + 42;
const CARET_CHANGED: u32 = WM_APP + 43;
const ACCESSIBILITY_EVENTS: u32 = WM_APP + 44;
pub fn qpc() -> i64 {
    let mut value = 0;
    unsafe { QueryPerformanceCounter(&mut value).expect("QPC") };
    value
}
fn frequency() -> i64 {
    let mut value = 0;
    unsafe { QueryPerformanceFrequency(&mut value).expect("QPC frequency") };
    value
}
pub fn cpu_ms() -> f64 {
    let (mut a, mut b, mut k, mut u) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    if unsafe { GetThreadTimes(GetCurrentThread(), &mut a, &mut b, &mut k, &mut u) }.is_err() {
        return 0.0;
    }
    let ticks = |t: FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
    (ticks(k) + ticks(u)) as f64 / 10000.0
}
pub fn client_size(hwnd: HWND) -> [u32; 2] {
    let mut r = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut r);
    }
    [
        ((r.right - r.left).max(1)) as u32,
        ((r.bottom - r.top).max(1)) as u32,
    ]
}
fn trace(canvas: &OnceLock<Canvas>, kind: &str, mut fields: serde_json::Value) {
    if let Some(canvas) = canvas.get() {
        fields["kind"] = kind.into();
        fields["qpc"] = qpc().into();
        let _ = canvas.trace(&fields);
    }
}
fn composition(hwnd: HWND, kind: IME_COMPOSITION_STRING) -> String {
    unsafe {
        let context = ImmGetContext(hwnd);
        if context.0.is_null() {
            return String::new();
        }
        let length = ImmGetCompositionStringW(context, kind, None, 0);
        let mut buffer = vec![0u16; length.max(0) as usize / 2];
        if length > 0 {
            ImmGetCompositionStringW(
                context,
                kind,
                Some(buffer.as_mut_ptr().cast()),
                length as u32,
            );
        }
        let _ = ImmReleaseContext(hwnd, context);
        String::from_utf16_lossy(&buffer)
    }
}
fn clipboard_read(hwnd: HWND) -> Option<String> {
    unsafe {
        OpenClipboard(Some(hwnd)).ok()?;
        let result = (|| {
            let handle = GetClipboardData(13).ok()?;
            let ptr = GlobalLock(HGLOBAL(handle.0));
            if ptr.is_null() {
                return None;
            }
            let size = GlobalSize(HGLOBAL(handle.0)) / 2;
            let data = std::slice::from_raw_parts(ptr.cast::<u16>(), size);
            let end = data.iter().position(|c| *c == 0).unwrap_or(size);
            let value = String::from_utf16_lossy(&data[..end]);
            let _ = GlobalUnlock(HGLOBAL(handle.0));
            Some(value)
        })();
        let _ = CloseClipboard();
        result
    }
}
fn clipboard_write(hwnd: HWND, text: &str) {
    unsafe {
        if OpenClipboard(Some(hwnd)).is_err() {
            return;
        }
        let value: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        if let Ok(memory) = GlobalAlloc(GMEM_MOVEABLE, value.len() * 2) {
            let ptr = GlobalLock(memory);
            if !ptr.is_null() {
                std::ptr::copy_nonoverlapping(value.as_ptr(), ptr.cast(), value.len());
                let _ = GlobalUnlock(memory);
                let _ = EmptyClipboard();
                if SetClipboardData(13, Some(HANDLE(memory.0))).is_err() {
                    let _ = GlobalFree(Some(memory));
                }
            }
        }
        let _ = CloseClipboard();
    }
}
fn update_ime(hwnd: HWND, hits: &HitMap) {
    let r = hits.caret;
    unsafe {
        let context = ImmGetContext(hwnd);
        if context.0.is_null() {
            return;
        }
        let point = POINT {
            x: r.x.round() as i32,
            y: (r.y + r.h).round() as i32,
        };
        let _ = ImmSetCompositionWindow(
            context,
            &COMPOSITIONFORM {
                dwStyle: CFS_POINT,
                ptCurrentPos: point,
                rcArea: RECT::default(),
            },
        );
        let _ = ImmSetCandidateWindow(
            context,
            &CANDIDATEFORM {
                dwIndex: 0,
                dwStyle: CFS_EXCLUDE,
                ptCurrentPos: point,
                rcArea: RECT {
                    left: r.x as i32,
                    top: r.y as i32,
                    right: (r.x + r.w).ceil() as i32,
                    bottom: (r.y + r.h).ceil() as i32,
                },
            },
        );
        let _ = ImmReleaseContext(hwnd, context);
    }
}
fn caption_button(hit: usize) -> Option<Target> {
    match hit as u32 {
        HTMINBUTTON => Some(Target::Minimize),
        HTMAXBUTTON => Some(Target::Maximize),
        HTCLOSE => Some(Target::Close),
        _ => None,
    }
}
fn pointer_phase(message: u32) -> u8 {
    match message {
        WM_POINTERDOWN => 0,
        WM_POINTERUP => 2,
        WM_POINTERCAPTURECHANGED => 3,
        _ => 1,
    }
}
fn pointer(hwnd: HWND, message: u32, wparam: WPARAM, state: &mut WindowState) -> bool {
    let receipt = qpc();
    let id = (wparam.0 & 0xffff) as u32;
    let phase = pointer_phase(message);
    if phase == 3 {
        let Some(target) = state.contacts.remove(&id) else {
            return false;
        };
        trace(
            &state.canvas,
            "native_cancel",
            json!({"receipt_qpc":receipt,"pointer_id":id,"target":format!("{target:?}")}),
        );
        if target == Target::List {
            let _ = state.comp.try_send(Event::CancelTouch(id));
        } else if target != Target::Canvas {
            let _ = state
                .ui
                .try_send(UiEvent::Pointer(target, 0.0, 0.0, 3, receipt));
        }
        return true;
    }
    let mut info = POINTER_INFO::default();
    if unsafe { GetPointerInfo(id, &mut info) }.is_err() {
        return false;
    }
    let mut point = info.ptPixelLocation;
    unsafe {
        let _ = ScreenToClient(hwnd, &mut point);
    }
    let x = point.x as f32 / state.scale;
    let y = point.y as f32 / state.scale;
    let mut kind = PT_POINTER;
    unsafe {
        let _ = GetPointerType(id, &mut kind);
    }
    if phase == 0 {
        let hit=state.hits.load();let target=if kind==PT_TOUCH {hit.touch_target(x,y)} else {hit.at(x,y)};if target!=Target::Menu&&hit.popup.load(std::sync::atomic::Ordering::Acquire){let _=state.ui.try_send(UiEvent::ClosePopup);}
        state.contacts.insert(id, target);
        if kind == PT_MOUSE && matches!(target, Target::Divider | Target::Slider(_)) {
            unsafe {
                SetCapture(hwnd);
            }
        }
    }
    let Some(target) = state.contacts.get(&id).copied() else {
        return false;
    };
    if target == Target::Canvas && kind == PT_PEN {
        let mut count = 0;
        unsafe {
            let _ = GetPointerPenInfoHistory(id, &mut count, None);
        }
        let mut history = vec![POINTER_PEN_INFO::default(); count as usize];
        if count > 0
            && unsafe { GetPointerPenInfoHistory(id, &mut count, Some(history.as_mut_ptr())) }
                .is_ok()
        {
            history.truncate(count as usize);
            let mut origin = POINT::default();
            unsafe {
                let _ = ClientToScreen(hwnd, &mut origin);
            }
            let mut samples: Vec<_> = history
                .iter()
                .map(|p| PenSample {
                    x: (p.pointerInfo.ptPixelLocation.x - origin.x) as f32,
                    y: (p.pointerInfo.ptPixelLocation.y - origin.y) as f32,
                    pressure: p.pressure as f32 / 1024.0,
                    tilt_x: p.tiltX as f32,
                    tilt_y: p.tiltY as f32,
                    twist: p.rotation as f32,
                    timestamp: p.pointerInfo.PerformanceCount,
                    flags: p.pointerInfo.pointerFlags.0,
                })
                .collect();
            poc_common::pen_history_oldest_first(&mut samples);
            trace(
                &state.canvas,
                "host_pen_message",
                json!({"history_count":samples.len(),"samples":samples.len(),"receipt_qpc":receipt,"timestamp_basis":"native_PerformanceCount_matched_ordered_history","native_history_matched":true,"native_history_count":samples.len(),"pointer_id":id}),
            );
            if let Some(canvas) = state.canvas.get()
                && let Err(error) = canvas.pen(&samples, receipt)
            {
                trace(&state.canvas, "host_error", json!({"error":error}));
                unsafe {
                    let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            }
        }
    } else if target == Target::List && kind == PT_TOUCH {
        let _ = state.comp.try_send(Event::Touch(id, y, phase, receipt));
    } else {
        trace(
            &state.canvas,
            "native_input",
            json!({"receipt_qpc":receipt,"target":format!("{target:?}"),"phase":phase,"x":x,"y":y}),
        );
        let _ = state
            .ui
            .try_send(UiEvent::Pointer(target, x, y, phase, receipt));
    }
    if phase == 2 {
        state.contacts.remove(&id);
        if kind == PT_MOUSE {
            unsafe {
                let _ = ReleaseCapture();
            }
        }
    }
    true
}
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
    }
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
    if ptr.is_null() {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }
    let state = unsafe { &mut *ptr };
    if state.closing && matches!(message, WM_POINTERDOWN | WM_POINTERUPDATE | WM_POINTERUP) {
        return LRESULT(0);
    }
    match message {
        WM_NCCALCSIZE => {
            if wparam.0 != 0 && !unsafe { IsZoomed(hwnd) }.as_bool() {
                let params = unsafe { &mut *(lparam.0 as *mut NCCALCSIZE_PARAMS) };
                params.rgrc[0].top += 1;
            }
            return LRESULT(0);
        }
        WM_GETMINMAXINFO => {
            if lparam.0 != 0 {
                let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
                let mut info = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
                    let sizing = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
                    sizing.ptMaxPosition = POINT {
                        x: info.rcWork.left - info.rcMonitor.left,
                        y: info.rcWork.top - info.rcMonitor.top,
                    };
                    sizing.ptMaxSize = POINT {
                        x: info.rcWork.right - info.rcWork.left,
                        y: info.rcWork.bottom - info.rcWork.top,
                    };
                    return LRESULT(0);
                }
            }
        }
        WM_NCHITTEST => {
            let mut p = POINT {
                x: lparam.0 as i16 as i32,
                y: (lparam.0 >> 16) as i16 as i32,
            };
            unsafe {
                let _ = ScreenToClient(hwnd, &mut p);
            }
            let [w, h] = client_size(hwnd);
            let border = (8.0 * state.scale) as i32;
            let x = p.x;
            let y = p.y;
            let code = if !unsafe { IsZoomed(hwnd) }.as_bool() && x < border && y < border {
                HTTOPLEFT
            } else if !unsafe { IsZoomed(hwnd) }.as_bool() && x >= w as i32 - border && y < border {
                HTTOPRIGHT
            } else if !unsafe { IsZoomed(hwnd) }.as_bool() && x < border && y >= h as i32 - border {
                HTBOTTOMLEFT
            } else if !unsafe { IsZoomed(hwnd) }.as_bool()
                && x >= w as i32 - border
                && y >= h as i32 - border
            {
                HTBOTTOMRIGHT
            } else if !unsafe { IsZoomed(hwnd) }.as_bool() && x < border {
                HTLEFT
            } else if !unsafe { IsZoomed(hwnd) }.as_bool() && x >= w as i32 - border {
                HTRIGHT
            } else if !unsafe { IsZoomed(hwnd) }.as_bool() && y < border {
                HTTOP
            } else if !unsafe { IsZoomed(hwnd) }.as_bool() && y >= h as i32 - border {
                HTBOTTOM
            } else {
                match state
                    .hits
                    .load()
                    .at(x as f32 / state.scale, y as f32 / state.scale)
                {
                    Target::Title => HTCAPTION,
                    Target::Minimize => HTMINBUTTON,
                    Target::Maximize => HTMAXBUTTON,
                    Target::Close => HTCLOSE,
                    _ => HTCLIENT,
                }
            };
            return LRESULT(code as isize);
        }
        WM_GETTITLEBARINFOEX => {
            if lparam.0 != 0 {
                let title = unsafe { &mut *(lparam.0 as *mut TITLEBARINFOEX) };
                let mut origin = POINT::default();
                unsafe {
                    let _ = ClientToScreen(hwnd, &mut origin);
                }
                let [width, _] = client_size(hwnd);
                title.rcTitleBar = RECT {
                    left: origin.x,
                    top: origin.y,
                    right: origin.x + width as i32,
                    bottom: origin.y + (metric("title_height") * state.scale) as i32,
                };
                title.rgrect[0] = title.rcTitleBar;
                title.rgstate[1] = 0x8000;
                title.rgstate[4] = 0x8000;
                for (index, target) in [
                    (2, Target::Minimize),
                    (3, Target::Maximize),
                    (5, Target::Close),
                ] {
                    if let Some((r, _)) =
                        state.hits.load().entries.iter().find(|(_, t)| *t == target)
                    {
                        title.rgrect[index] = RECT {
                            left: origin.x + (r.x * state.scale).round() as i32,
                            top: origin.y + (r.y * state.scale).round() as i32,
                            right: origin.x + ((r.x + r.w) * state.scale).round() as i32,
                            bottom: origin.y + ((r.y + r.h) * state.scale).round() as i32,
                        };
                        title.rgstate[index] = 0;
                    }
                }
                return LRESULT(0);
            }
        }
        WM_GETOBJECT => {
            let response = {
                let mut adapter = state.adapter.lock().unwrap();
                adapter.handle_wm_getobject(wparam, lparam, &mut state.activation)
            };
            if let Some(response) = response {
                return response.into();
            }
        }
        WM_NCLBUTTONDOWN | WM_NCPOINTERDOWN => {
            if state.hits.load().popup.load(std::sync::atomic::Ordering::Acquire) {
                let _ = state.ui.try_send(UiEvent::ClosePopup);
            }
            if message == WM_NCLBUTTONDOWN && state.caption.press(caption_button(wparam.0)) {
                return LRESULT(0);
            }
        }
        WM_NCLBUTTONUP => {
            let button = caption_button(wparam.0);
            let handled = state.caption.is_pressed() || button.is_some();
            if let Some(button) = state.caption.release(button) {
                unsafe {
                    match button {
                        Target::Minimize => { let _ = ShowWindow(hwnd, SW_MINIMIZE); }
                        Target::Maximize => {
                            let command = if IsZoomed(hwnd).as_bool() { SW_RESTORE } else { SW_MAXIMIZE };
                            let _ = ShowWindow(hwnd, command);
                        }
                        Target::Close => { let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)); }
                        _ => unreachable!(),
                    }
                }
            }
            if handled {
                return LRESULT(0);
            }
        }
        WM_CANCELMODE | WM_CAPTURECHANGED => state.caption.cancel(),
        WM_ACTIVATE if wparam.0 & 0xffff == WA_INACTIVE as usize => {
            state.caption.cancel();
            if state.hits.load().popup.load(std::sync::atomic::Ordering::Acquire) {
                let _ = state.ui.try_send(UiEvent::ClosePopup);
            }
        }
        WM_SETFOCUS | WM_KILLFOCUS => {
            if message == WM_KILLFOCUS { state.caption.cancel(); }
            if message==WM_KILLFOCUS&&state.hits.load().popup.load(std::sync::atomic::Ordering::Acquire){let _=state.ui.try_send(UiEvent::ClosePopup);}
            let queued = state
                .adapter
                .lock()
                .unwrap()
                .update_window_focus_state(message == WM_SETFOCUS);
            if let Some(queued) = queued {
                queued.raise();
            }
        }
        WM_DPICHANGED => {
            state.scale = (wparam.0 & 0xffff) as f32 / 96.0;
            let r = unsafe { &*(lparam.0 as *const RECT) };
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            update_ime(hwnd, &state.hits.load());
        }
        WM_SIZE => {
            let [w, h] = client_size(hwnd);
            if w > 1 && h > 1 {
                let _ = state.ui.try_send(UiEvent::Size(w, h, state.scale));
                let _ = state.comp.try_send(Event::Size(w, h, state.scale));
                if let Some(canvas) = state.canvas.get() {
                    let _ = canvas.resize(w, h, state.scale);
                }
            }
        }
        WM_POINTERDOWN | WM_POINTERUPDATE | WM_POINTERUP | WM_POINTERCAPTURECHANGED => {
            if pointer(hwnd, message, wparam, state) {
                return LRESULT(0);
            }
        }
        WM_MOUSEWHEEL | WM_POINTERWHEEL => {
            let mut p = POINT {
                x: lparam.0 as i16 as i32,
                y: (lparam.0 >> 16) as i16 as i32,
            };
            unsafe {
                let _ = ScreenToClient(hwnd, &mut p);
            }
            let hits = state.hits.load();
            let x = p.x as f32 / state.scale;
            let y = p.y as f32 / state.scale;
            let target = hits.at(x, y);
            if hits.list.contains(x, y) {
                let delta = ((wparam.0 >> 16) as i16) as f32;
                let _ = state.comp.try_send(Event::Wheel(-delta / 120.0 * 132.0));
            } else if matches!(
                target,
                Target::Properties
                    | Target::Slider(_)
                    | Target::Number
                    | Target::Text
                    | Target::Menu
            ) {
                let delta = ((wparam.0 >> 16) as i16) as f32;
                let _ = state
                    .ui
                    .try_send(UiEvent::WheelProperties(-delta / 120.0 * 88.0));
            }
            return LRESULT(0);
        }
        WM_CHAR => {
            for ch in state.characters.push(wparam.0 as u16).into_iter().flatten(){if !ch.is_control(){let _=state.ui.try_send(UiEvent::Character(ch));}}
            return LRESULT(0);
        }
        WM_KEYDOWN => {
            let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
            if ctrl && wparam.0 == VK_V.0 as usize {
                if let Some(text) = clipboard_read(hwnd) {
                    let _ = state.ui.try_send(UiEvent::Paste(text));
                }
            } else {
                let _ = state.ui.try_send(UiEvent::Key(
                    wparam.0 as u32,
                    ctrl,
                    unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0,
                ));
            }
            return LRESULT(0);
        }
        WM_IME_STARTCOMPOSITION => {
            update_ime(hwnd, &state.hits.load());
            let _ = state.ui.try_send(UiEvent::Ime(String::new(), false));
            return LRESULT(0);
        }
        WM_IME_COMPOSITION => {
            if lparam.0 as u32 & GCS_RESULTSTR.0 != 0 {
                let text = composition(hwnd, GCS_RESULTSTR);
                let _ = state.ui.try_send(UiEvent::Ime(text, true));
            } else if lparam.0 as u32 & GCS_COMPSTR.0 != 0 {
                let text = composition(hwnd, GCS_COMPSTR);
                let _ = state.ui.try_send(UiEvent::Ime(text, false));
            }
            return LRESULT(0);
        }
        WM_IME_ENDCOMPOSITION => {
            let _ = state.ui.try_send(UiEvent::Ime(String::new(), false));
            return LRESULT(0);
        }
        ACCESSIBILITY_EVENTS => {
            for events in state.accessibility_events.try_iter() {
                events.raise();
            }
            return LRESULT(0);
        }
        CARET_CHANGED => {
            update_ime(hwnd, &state.hits.load());
            return LRESULT(0);
        }
        COPY_TEXT => {
            let text = unsafe { Box::from_raw(lparam.0 as *mut String) };
            clipboard_write(hwnd, &text);
            return LRESULT(0);
        }
        WM_TIMER => {
            if state.last_cpu.elapsed() >= Duration::from_secs(1) {
                trace(
                    &state.canvas,
                    "thread_cpu",
                    json!({"thread":"window","cpu_ms":cpu_ms()}),
                );
                state.last_cpu = Instant::now();
            }
            return LRESULT(0);
        }
        WM_CLOSE => {
            if !state.closing {
                state.closing = true;
                let _ = state.ui.try_send(UiEvent::Shutdown);
                let _ = state.comp.try_send(Event::Shutdown);
            }
            return LRESULT(0);
        }
        CLOSE_WINDOW => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            return LRESULT(0);
        }
        WM_DESTROY => {
            unsafe {
                PostQuitMessage(0);
            }
            return LRESULT(0);
        }
        WM_ERASEBKGND => return LRESULT(1),
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}
struct Ui {
    model: Model,
    geometry: Geometry,
    text: TextSystem,
    hits: Arc<ArcSwap<HitMap>>,
    adapter: Arc<Mutex<Adapter>>,
    accessibility_events: Sender<QueuedEvents>,
    tree: Arc<Mutex<A11yTree>>,
    canvas: Arc<OnceLock<Canvas>>,
    comp: Sender<Event>,
    tx: Sender<Transaction>,
    hwnd: usize,
    state: u16,
    offset: f32,
    range: std::ops::Range<u32>,
    focus: Target,
    text_cursor: FieldCursor,
    number_cursor: FieldCursor,
    number_text: String,
    preedit: String,
    properties_offset: f32,
    text_scroll: f32,
    number_scroll: f32,
    properties_height: f32,
    ready: bool,
    a11y_initial: bool,
    text_logged: HashSet<(String, u32, u32, bool)>,
    popup_open: bool,
    popup_rect: Rect,
    last_caret: Rect,
    nodes: RetainedTree,
    drawer_x: f32,
    wrap_rect: Rect,
}
fn node_id(target: Target) -> NodeId {
    NodeId(match target {
        Target::Tab(i) => 2 + i as u64,
        Target::Drawer => 4,
        Target::Zen => 5,
        Target::Divider => 8,
        Target::GroupTab(i) => 9 + i as u64,
        Target::List => 11,
        Target::Slider(i) => 12 + i as u64,
        Target::Number => 16,
        Target::Text => 17,
        Target::WrapText => 19,
        Target::Menu => 18,
        Target::Title => 21,
        Target::Canvas => 22,
        Target::Properties => 7,
        Target::DrawerContent => 20,
        Target::Visible(i) => 1001 + i as u64 * 2,
        Target::MenuItem(i) => 200_000 + i as u64,
        Target::Minimize => 100,
        Target::Maximize => 101,
        Target::Close => 102,
    })
}
fn target_from_node(id: NodeId) -> Option<Target> {
    match id.0 {
        2..=3 => Some(Target::Tab((id.0 - 2) as u32)),
        4 => Some(Target::Drawer),
        5 => Some(Target::Zen),
        8 => Some(Target::Divider),
        9..=10 => Some(Target::GroupTab((id.0 - 9) as u32)),
        11 => Some(Target::List),
        12..=15 => Some(Target::Slider((id.0 - 12) as u32)),
        16 => Some(Target::Number),
        17 => Some(Target::Text),
        18 => Some(Target::Menu),
        19 => Some(Target::WrapText),
        100 => Some(Target::Minimize),
        101 => Some(Target::Maximize),
        102 => Some(Target::Close),
        200000..=200007 => Some(Target::MenuItem((id.0 - 200000) as u32)),
        _ if id.0 >= 1000 && id.0 < 132_072 => Some(Target::Visible(((id.0 - 1000) / 2) as u32)),
        _ => None,
    }
}
fn color(rgb: [u8; 3]) -> [f32; 4] {
    [
        rgb[0] as f32 / 255.0,
        rgb[1] as f32 / 255.0,
        rgb[2] as f32 / 255.0,
        1.0,
    ]
}
fn rect_prim(rect: Rect, rgb: [u8; 3], radius: f32) -> Prim {
    Prim::Rect {
        rect,
        color: color(rgb),
        radius,
        border: 0.0,
        border_color: [0.0; 4],
    }
}
fn probe(rect: Rect, rgb: [u8; 3]) -> Prim {
    Prim::Probe {
        rect,
        color: color(rgb),
    }
}
impl Ui {
    fn log_text(&mut self, value: &str, width: f32, shaped: &Shaped, wrap: bool) {
        let scale = self.geometry.scale;
        if self
            .text_logged
            .insert((value.to_owned(), width.to_bits(), scale.to_bits(), wrap))
        {
            trace(
                &self.canvas,
                "text_layout",
                json!({"text":value,"wrap":wrap,"missing_glyphs":shaped.missing,"line_breaks":shaped.line_breaks,"width":width,"scale":scale,"height_px":shaped.height,"width_px":shaped.width,"line_height_px":shaped.line_height,"end_caret_px":shaped.caret}),
            );
        }
    }
    fn glyphs(&self, prims: &mut Vec<Prim>, shaped: &Shaped, clip: Rect, scroll: f32) {
        let scale = self.geometry.scale;
        for glyph in &shaped.glyphs {
            let rect = Rect::new(
                clip.x + (glyph.rect[0] - scroll) / scale,
                clip.y + glyph.rect[1] / scale,
                glyph.rect[2] / scale,
                glyph.rect[3] / scale,
            );
            if let Some((rect, uv)) = clip_glyph(rect, glyph.uv, clip) {
                prims.push(Prim::Glyph {
                    rect,
                    uv,
                    color: color(rgb("text")),
                });
            }
        }
    }
    fn text(
        &mut self,
        prims: &mut Vec<Prim>,
        value: &str,
        x: f32,
        y: f32,
        width: f32,
        size: f32,
    ) -> Result<Rect, String> {
        let scale = self.geometry.scale;
        let shaped = self.text.shape_line(value, width, size, scale)?;
        self.log_text(value, width, &shaped, false);
        self.glyphs(
            prims,
            &shaped,
            Rect::new(x, y, width, shaped.height / scale),
            0.0,
        );
        let c = shaped.caret_at(value.len());
        Ok(Rect::new(
            x + c[0] / scale,
            y + c[1] / scale,
            c[2] / scale,
            c[3] / scale,
        ))
    }
    fn text_wrapped(
        &mut self,
        prims: &mut Vec<Prim>,
        value: &str,
        x: f32,
        y: f32,
        width: f32,
        size: f32,
    ) -> Result<Rect, String> {
        let scale = self.geometry.scale;
        let shaped = self.text.shape(value, width, size, scale)?;
        self.log_text(value, width, &shaped, true);
        self.glyphs(
            prims,
            &shaped,
            Rect::new(x, y, width, shaped.height / scale),
            0.0,
        );
        let c = shaped.caret_at(value.len());
        Ok(Rect::new(
            x + c[0] / scale,
            y + c[1] / scale,
            c[2] / scale,
            c[3] / scale,
        ))
    }
    fn field_text(
        &mut self,
        prims: &mut Vec<Prim>,
        value: &str,
        bounds: Rect,
        target: Target,
    ) -> Result<Rect, String> {
        let scale = self.geometry.scale;
        let shaped = self
            .text
            .shape_line(value, bounds.w, metric("ui_font_size"), scale)?;
        let index = if target == Target::Text {
            if self.preedit.is_empty() {
                self.text_cursor.caret
            } else {
                self.text_cursor.selection().start + self.preedit.len()
            }
        } else {
            self.number_cursor.caret
        };
        let c = shaped.caret_at(index);
        let current = if target == Target::Text {
            self.text_scroll
        } else {
            self.number_scroll
        };
        let scroll = line_scroll(
            current,
            Rect::new(c[0], c[1], c[2], c[3]),
            bounds.w * scale,
            shaped.width,
        );
        if target == Target::Text {
            self.text_scroll = scroll;
        } else {
            self.number_scroll = scroll;
        }
        self.log_text(value, bounds.w, &shaped, false);
        let selection = if target == Target::Text {
            self.text_cursor.selection()
        } else {
            self.number_cursor.selection()
        };
        if self.focus == target
            && !selection.is_empty()
            && (target != Target::Text || self.preedit.is_empty())
        {
            let first = shaped.caret_at(selection.start);
            let last = shaped.caret_at(selection.end);
            let rect = Rect::new(
                bounds.x + (first[0].min(last[0]) - scroll) / scale,
                bounds.y + first[1] / scale,
                (first[0] - last[0]).abs() / scale,
                first[3] / scale,
            );
            if let Some((rect, _)) = clip_glyph(rect, [0.0, 0.0, 1.0, 1.0], bounds) {
                prims.push(rect_prim(rect, rgb("selected"), 0.0));
            }
        }
        self.glyphs(prims, &shaped, bounds, scroll);
        let width = (c[2] / scale).max(1.0 / scale).min(bounds.w);
        let x = (bounds.x + (c[0] - scroll) / scale).clamp(bounds.x, bounds.x + bounds.w - width);
        let y = (bounds.y + c[1] / scale).clamp(bounds.y, bounds.y + bounds.h);
        Ok(Rect::new(
            x,
            y,
            width,
            (c[3] / scale).min(bounds.y + bounds.h - y),
        ))
    }
    fn layer(id: LayerId, rect: Rect, clip: Rect, display: Vec<Prim>) -> Layer {
        Layer {
            id,
            rect,
            clip,
            display: Arc::new(display),
            origin: [0.0, 0.0],
        }
    }
    fn build_title(&mut self) -> Result<Layer, String> {
        let g = self.geometry;
        let rect = Rect::new(0.0, 0.0, g.width, metric("title_height"));
        let mut p = vec![rect_prim(rect, rgb("title"), 0.0)];
        self.text(
            &mut p,
            poc_common::TITLE,
            8.0,
            9.0,
            230.0,
            metric("ui_font_size"),
        )?;
        for (x, w, label, target) in [
            (256.0, 42.0, string("tab_a"), Target::Tab(0)),
            (306.0, 42.0, string("tab_b"), Target::Tab(1)),
            (356.0, 78.0, string("drawer"), Target::Drawer),
            (442.0, 64.0, string("zen"), Target::Zen),
            (
                g.width - 138.0,
                46.0,
                string("caption_minimize"),
                Target::Minimize,
            ),
            (
                g.width - 92.0,
                46.0,
                string("caption_maximize"),
                Target::Maximize,
            ),
            (g.width - 46.0, 46.0, string("caption_close"), Target::Close),
        ] {
            let selected = match target {
                Target::Tab(i) => self.model.state().tab == i,
                _ => false,
            };
            p.push(rect_prim(
                Rect::new(x, 3.0, w, 34.0),
                if selected {
                    rgb("selected")
                } else {
                    rgb("button")
                },
                5.0,
            ));
            self.text(
                &mut p,
                label,
                x + 9.0,
                9.0,
                w - 12.0,
                metric("ui_font_size"),
            )?;
        }
        let px = 1.0 / g.scale;
        p.push(probe(
            g.width_probe(),
            [0, 0, 255],
        ));
        p.push(probe(
            Rect::new(200.0 * px, 16.0 * px, 6.0 * px, 6.0 * px),
            [255; 3],
        ));
        Ok(Self::layer(LayerId::Title, rect, rect, p))
    }
    fn build_drawer(&mut self) -> Result<Layer, String> {
        let g = self.geometry;
        let px = 1.0 / g.scale;
        let rect = Rect::new(
            0.0,
            metric("column_top"),
            metric("drawer_width") + 2.0 * px,
            g.height - metric("column_top") - metric("column_bottom"),
        );
        let glass = Rect::new(
            0.0,
            metric("column_top"),
            metric("drawer_width"),
            g.height - metric("column_top") - metric("column_bottom"),
        );
        let mut p = vec![Prim::Glass {
            rect: glass,
            radius: metric("radius"),
        }];
        self.text(
            &mut p,
            string("drawer"),
            16.0,
            72.0,
            248.0,
            metric("ui_font_size"),
        )?;
        p.push(probe(
            Rect::new(
                metric("drawer_width"),
                metric("column_top"),
                2.0 * px,
                glass.h,
            ),
            [255, 255, 0],
        ));
        Ok(Self::layer(
            LayerId::Drawer,
            Rect::new(rect.x, rect.y, rect.w, rect.h + 2.0 * px),
            Rect::new(0.0, 0.0, g.width, g.height),
            p,
        ))
    }
    fn build_layers(&mut self) -> Result<Layer, String> {
        let g = self.geometry;
        let r = g.layers;
        let px = 1.0 / g.scale;
        let mut p = vec![Prim::Glass {
            rect: r,
            radius: metric("radius"),
        }];
        for (i, x, width) in [(0, r.x + 4.0, 98.0), (1, r.x + 104.0, 108.0)] {
            p.push(rect_prim(
                Rect::new(x, r.y + 3.0, width, metric("header_height") - 6.0),
                rgb(if self.model.state().group_tab == i {
                    "selected"
                } else {
                    "button"
                }),
                5.0,
            ));
        }
        self.text(
            &mut p,
            string("layers"),
            r.x + 8.0,
            r.y + 8.0,
            90.0,
            metric("ui_font_size"),
        )?;
        self.text(
            &mut p,
            string("brushes"),
            r.x + 108.0,
            r.y + 8.0,
            100.0,
            metric("ui_font_size"),
        )?;
        p.push(probe(
            Rect::new(r.x, r.y + r.h - 2.0 * px, r.w / 3.0, 2.0 * px),
            [255, 0, 255],
        ));
        p.push(probe(
            Rect::new(r.x + r.w * 2.0 / 3.0, r.y + r.h - px, r.w / 3.0, px),
            [255, 128, 0],
        ));
        Ok(Self::layer(
            LayerId::Layers,
            Rect::new(r.x, r.y, r.w, r.h + 2.0 * px),
            Rect::new(0.0, 0.0, g.width, g.height),
            p,
        ))
    }
    fn build_list(&mut self) -> Result<Layer, String> {
        let g = self.geometry;
        let range = recorded_rows(self.offset, g.list.h, self.model.state().visible_rows);
        self.range = range.clone();
        let rect = Rect::new(
            g.list.x,
            g.list.y + range.start as f32 * poc_common::ROW_HEIGHT,
            g.list.w,
            (range.end - range.start) as f32 * poc_common::ROW_HEIGHT,
        );
        let mut p = Vec::new();
        for ordinal in range {
            let Some(row) = self.model.visible_row(ordinal) else {
                continue;
            };
            let y = g.list.y + ordinal as f32 * poc_common::ROW_HEIGHT;
            let bounds = Rect::new(g.list.x, y, g.list.w, poc_common::ROW_HEIGHT);
            p.push(Prim::Image {
                rect: Rect::new(
                    bounds.x + 8.0,
                    y + 6.0,
                    metric("thumbnail_size"),
                    metric("thumbnail_size"),
                ),
                uv: [0.0, 0.0, 1.0, 1.0],
                atlas: row + 1,
            });
            self.text(
                &mut p,
                &poc_common::label(row as usize),
                bounds.x + 48.0,
                y + 12.0,
                176.0,
                metric("ui_font_size"),
            )?;
            if !self.model.row_visible(row) {
                p.push(rect_prim(
                    Rect::new(bounds.x + bounds.w - 75.0, y + 4.0, 36.0, 36.0),
                    rgb("selected"),
                    4.0,
                ));
            }
            p.push(Prim::Image {
                rect: Rect::new(bounds.x + bounds.w - 67.0, y + 14.0, 16.0, 16.0),
                uv: [0.0, 0.0, 1.0, 1.0],
                atlas: 0,
            });
            self.text(
                &mut p,
                string("percent"),
                bounds.x + bounds.w - 43.0,
                y + 12.0,
                40.0,
                metric("ui_font_size"),
            )?;
            p.push(probe(
                Rect::new(
                    bounds.x + 2.0 / g.scale,
                    y,
                    2.0 / g.scale,
                    poc_common::ROW_HEIGHT,
                ),
                poc_common::ruler_rgb(row as u16),
            ));
        }
        Ok(Self::layer(LayerId::List, rect, g.list, p))
    }
    fn properties_controls(&self) -> Vec<(Rect, Target)> {
        let r = self.geometry.properties;
        let x = r.x + 8.0;
        let y = r.y + 8.0 - self.properties_offset;
        let mut result = vec![(Rect::new(x, y, r.w - 16.0, 32.0), Target::Menu)];
        for i in 0..4 {
            result.push((
                Rect::new(
                    x,
                    y + 44.0 + i as f32 * poc_common::ROW_HEIGHT,
                    r.w - 16.0,
                    38.0,
                ),
                Target::Slider(i),
            ));
        }
        result.push((Rect::new(x, y + 224.0, r.w - 16.0, 32.0), Target::Number));
        result.push((Rect::new(x, y + 264.0, r.w - 16.0, 32.0), Target::Text));
        result
    }
    fn build_properties(&mut self) -> Result<Layer, String> {
        let g = self.geometry;
        let r = g.properties;
        let px = 1.0 / g.scale;
        let mut p = vec![Prim::Glass {
            rect: r,
            radius: metric("radius"),
        }];
        for (bounds, target) in self.properties_controls() {
            match target {
                Target::Menu => {
                    p.push(rect_prim(bounds, rgb("menu"), 5.0));
                    self.text(
                        &mut p,
                        string("menu"),
                        bounds.x + 8.0,
                        bounds.y + 7.0,
                        bounds.w - 16.0,
                        metric("ui_font_size"),
                    )?;
                }
                Target::Slider(i) => {
                    let spec = SLIDERS[i as usize];
                    self.text(
                        &mut p,
                        spec.label,
                        bounds.x,
                        bounds.y,
                        bounds.w - 48.0,
                        metric("ui_font_size"),
                    )?;
                    let value = self.model.slider_text(i);
                    self.text(
                        &mut p,
                        &value,
                        bounds.x + bounds.w - 40.0,
                        bounds.y,
                        40.0,
                        metric("ui_font_size"),
                    )?;
                    let track = Rect::new(bounds.x + 8.0, bounds.y + 27.0, bounds.w - 16.0, 2.0);
                    p.push(rect_prim(track, rgb("track"), 1.0));
                    let fraction = ((self.model.state().sliders[i as usize] - spec.min)
                        / (spec.max - spec.min)) as f32;
                    let thumb = Rect::new(
                        track.x + track.w * fraction - 5.0,
                        track.y - 5.0,
                        10.0,
                        12.0,
                    );
                    p.push(rect_prim(
                        thumb,
                        if i == 0 { [0, 255, 0] } else { rgb("text") },
                        5.0,
                    ));
                }
                Target::Number | Target::Text => {
                    p.push(rect_prim(bounds, rgb("field"), 4.0));
                    let value = if target == Target::Number {
                        self.number_text.clone()
                    } else {
                        self.displayed_text()
                    };
                    let caret = self.field_text(
                        &mut p,
                        &value,
                        Rect::new(
                            bounds.x + 8.0,
                            bounds.y + 6.0,
                            bounds.w - 16.0,
                            bounds.h - 12.0,
                        ),
                        target,
                    )?;
                    if target == self.focus {
                        p.push(probe(
                            Rect::new(caret.x, caret.y, 1.0 / g.scale, caret.h),
                            [255; 3],
                        ));
                        let mut hits = (**self.hits.load()).clone();
                        hits.caret = g.physical(caret);
                        self.hits.store(Arc::new(hits));
                    }
                }
                _ => {}
            }
        }
        let y = r.y + 312.0 - self.properties_offset;
        let end = self.text_wrapped(
            &mut p,
            poc_common::PARAGRAPH,
            r.x + 8.0,
            y,
            metric("wrap_width"),
            metric("ui_font_size"),
        )?;
        self.properties_height = end.y + end.h - r.y + self.properties_offset + 8.0;
        self.wrap_rect = Rect::new(r.x + 8.0, y, metric("wrap_width"), end.y + end.h - y);
        Ok(Self::layer(
            LayerId::Properties,
            Rect::new(r.x, r.y, r.w, r.h + 2.0 * px),
            r,
            p,
        ))
    }
    fn publish(
        &mut self,
        dirty: HashSet<LayerId>,
        rows_changed: bool,
        event_start: i64,
        cause: &str,
        event_qpc: i64,
    ) -> Result<(), String> {
        if !self.ready {
            return Ok(());
        }
        let started = Instant::now();
        let mut hits = self.geometry.base_hits();
        hits.push((
            Rect::new(
                self.geometry.layers.x + 108.0,
                self.geometry.layers.y,
                110.0,
                36.0,
            ),
            Target::GroupTab(1),
        ));
        for (rect, target) in self.properties_controls() {
            if self.geometry.properties.contains(rect.x, rect.y)
                && rect.y + rect.h <= self.geometry.properties.y + self.geometry.properties.h
            {
                hits.push((
                    if matches!(target, Target::Slider(_)) {
                        Rect::new(rect.x, rect.y + 20.0, rect.w, 16.0)
                    } else {
                        rect
                    },
                    target,
                ));
            }
        }
        let previous = self.hits.load();
        let caret = previous.caret;
        let motion = previous.motion.clone();let popup=previous.popup.clone();
        let rows = if rows_changed {
            (0..self.model.state().visible_rows)
                .filter_map(|i| self.model.visible_row(i))
                .collect::<Vec<_>>()
                .into()
        } else {
            previous.rows.clone()
        };
        self.hits.store(Arc::new(HitMap {
            entries: hits,
            caret,
            list: self.geometry.list,
            drawer: Rect::new(
                0.0,
                metric("column_top"),
                metric("drawer_width"),
                self.geometry.height - metric("column_top") - metric("column_bottom"),
            ),
            rows,
            motion,popup,
        }));
        let layout_ms = started.elapsed().as_secs_f64() * 1000.0;
        let recording = Instant::now();
        let mut layers = Vec::new();
        let list_recorded = dirty.contains(&LayerId::List);
        if cause == "divider"
            && !list_recorded
            && let Some(layer) = self.nodes.with_clip(LayerId::List, self.geometry.list)
        {
            layers.push(layer);
        }
        for id in dirty {
            let layer = match id {
                LayerId::Title => self.build_title()?,
                LayerId::Drawer => self.build_drawer()?,
                LayerId::Layers => self.build_layers()?,
                LayerId::List => self.build_list()?,
                LayerId::Properties => self.build_properties()?,
                _ => continue,
            };
            layers.push(self.nodes.replace(layer));
        }
        let total_record_ms = recording.elapsed().as_secs_f64() * 1000.0;
        let (shaping_ms, raster_ms) = self.text.take_metrics();
        let record_ms = (total_record_ms - shaping_ms - raster_ms).max(0.0);
        let uploads = self.text.take_uploads();
        self.tx
            .send(Transaction {
                layers,
                uploads,
                state: self.state,
                geometry: self.geometry,
                rows: self.range.clone(),
                visible_rows: self.model.state().visible_rows,
                motion: self.hits.load().motion.clone(),
            })
            .map_err(|e| e.to_string())?;
        self.update_accessibility(rows_changed);
        let caret = self.hits.load().caret;
        if caret != self.last_caret {
            self.last_caret = caret;
            trace(
                &self.canvas,
                "caret",
                json!({"rect":caret.array(),"line_height":caret.h}),
            );
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(self.hwnd as *mut _)),
                    CARET_CHANGED,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
        trace(
            &self.canvas,
            "ui_transaction",
            json!({"event_start_qpc":event_start,"event_qpc":event_qpc,"cause":cause,"layout_ms":layout_ms,"record_ms":record_ms,"shaping_ms":shaping_ms,"raster_ms":raster_ms,"rows":self.model.state().rows,"recorded_rows":self.range.end-self.range.start,"list_recorded":list_recorded}),
        );
        Ok(())
    }
    fn editor_value(&self) -> &str {
        if self.focus == Target::Number {
            &self.number_text
        } else {
            self.model.text()
        }
    }
    fn editor_cursor(&mut self) -> &mut FieldCursor {
        if self.focus == Target::Number {
            &mut self.number_cursor
        } else {
            &mut self.text_cursor
        }
    }
    fn apply_editor_value(&mut self, value: String) {
        if self.focus == Target::Number {
            self.number_text = value;
            apply_number_draft(&self.number_text, &mut self.model);
        } else {
            self.model.set_text(&value);
            self.state = value.chars().count().min(u16::MAX as usize) as u16;
        }
        self.preedit.clear();
    }
    fn edit(&mut self, insert: &str) {
        let value = if self.focus == Target::Number {
            self.number_cursor.replace(&self.number_text, insert)
        } else {
            self.text_cursor.replace(self.model.text(), insert)
        };
        self.apply_editor_value(value);
    }
    fn erase(&mut self, backward: bool) {
        let text = self.editor_value().to_owned();
        let value = if backward {
            self.editor_cursor().backspace(&text)
        } else {
            self.editor_cursor().delete(&text)
        };
        self.apply_editor_value(value);
    }
    fn commit_number(&mut self) {
        self.number_text = commit_number_draft(&self.number_text, &mut self.model);
        self.number_cursor = FieldCursor::at_end(&self.number_text);
    }
    fn focus_to(&mut self, target: Target) {
        if self.focus == Target::Number && target != Target::Number {
            self.commit_number();
        }
        self.focus = target;
        if target != Target::Text {
            self.preedit.clear();
        }
    }
    fn displayed_text(&self) -> String {
        if self.preedit.is_empty() {
            self.model.text().into()
        } else {
            replace_selection(
                self.model.text(),
                self.text_cursor.selection(),
                &self.preedit,
            )
            .0
        }
    }

    fn set_popup_open(&mut self,open:bool){self.popup_open=open;self.hits.load().popup.store(open,std::sync::atomic::Ordering::Release);}
    fn popup(&mut self) -> Result<(), String> {
        let Some((button, _)) = self
            .properties_controls()
            .into_iter()
            .find(|(_, t)| *t == Target::Menu)
        else {
            return Ok(());
        };
        let rect = Rect::new(button.x + button.w - 8.0, button.y, 230.0, spec()["menu_items"].as_array().unwrap().len()as f32*metric("header_height")+metric("divider"));
        self.set_popup_open(true);
        self.popup_rect = rect;
        let mut p = vec![rect_prim(rect, rgb("button"), 8.0)];
        for i in 0..8 {
            self.text(
                &mut p,
                spec()["menu_items"][i as usize]
                    .as_str()
                    .expect("menu item"),
                rect.x + 12.0,
                rect.y+metric("divider")+i as f32*metric("header_height"),
                210.0,
                metric("ui_font_size"),
            )?;
        }
        self.comp
            .send(Event::Popup(Some(render::Popup {
                rect,
                layer: Self::layer(LayerId::Popup, rect, rect, p),
                uploads: self.text.take_uploads(),
            })))
            .map_err(|e| e.to_string())
    }
    fn activate(&mut self, target: Target) -> Result<(), String> {
        match target {
            Target::Tab(i) => {
                self.model.activate(0, i);
            }
            Target::GroupTab(i) => {
                self.model.activate(1, i);
            }
            Target::Drawer => {
                self.model.activate(2, 0);
                let _ = self
                    .comp
                    .send(Event::Drawer(self.model.state().drawer_open));
            }
            Target::Zen => {
                self.model.activate(3, 0);
                let _ = self.comp.send(Event::Zen(self.model.state().zen_visible));
            }
            Target::Visible(i) => {
                self.model.activate(4, i);
            }
            Target::Menu => self.popup()?,
            Target::MenuItem(i) => {
                self.model.activate(5, i);
                self.set_popup_open(false);
                let _ = self.comp.send(Event::Popup(None));
            }
            _ => {}
        }
        Ok(())
    }
    fn update_accessibility(&mut self, rows_changed: bool) {
        let g = self.geometry;
        let state = self.model.state();
        let full = !self.a11y_initial;
        let mut changed = Vec::new();
        let mut roots = Vec::new();
        for (index, id) in poc_common::IDS.iter().enumerate() {
            let node_id = NodeId(index as u64 + 2);
            roots.push(node_id);
            let mut node = Node::new(match index {
                0 | 1 | 7 | 8 => Role::Tab,
                2 | 3 | 16 => Role::Button,
                9 => Role::List,
                10..=13 => Role::Slider,
                14 | 15 => Role::TextInput,
                4 | 5 | 18 => Role::Group,
                6 => Role::Splitter,
                17 => Role::Paragraph,
                19 => Role::TitleBar,
                20 => Role::Canvas,
                _ => Role::GenericContainer,
            });
            node.set_author_id(*id);
            node.set_label(match index {
                0 => string("tab_a"),
                1 => string("tab_b"),
                2 | 18 => string("drawer"),
                3 => string("zen"),
                4 | 7 | 9 => string("layers"),
                5 => string("properties"),
                6 => string("divider"),
                8 => string("brushes"),
                10..=13 => SLIDERS[index - 10].label,
                14 => string("number"),
                15 => string("text"),
                16 => string("menu"),
                17 => poc_common::PARAGRAPH,
                19 => string("title_bar"),
                20 => string("canvas"),
                _ => unreachable!(),
            });
            let rect = match index {
                4 => g.layers,
                5 => g.properties,
                6 => g.divider,
                9 => g.list,
                17 => self.wrap_rect,
                18 => Rect::new(
                    self.drawer_x,
                    metric("column_top"),
                    metric("drawer_width"),
                    g.height - metric("column_top") - metric("column_bottom"),
                ),
                19 => Rect::new(0.0, 0.0, g.width, metric("title_height")),
                20 => Rect::new(0.0, 0.0, g.width, g.height),
                _ => self
                    .hits
                    .load()
                    .entries
                    .iter()
                    .find(|(_, t)| node_id == crate::native::node_id(*t))
                    .map_or(Rect::default(), |(r, _)| *r),
            };
            let rect = g.physical(rect);
            node.set_bounds(accesskit::Rect::new(
                rect.x as f64,
                rect.y as f64,
                (rect.x + rect.w) as f64,
                (rect.y + rect.h) as f64,
            ));
            match index {
                0..=1 => {
                    node.add_action(Action::Click);
                    node.set_selected(state.tab == index as u32);
                }
                7 | 8 => {
                    node.add_action(Action::Click);
                    node.set_selected(state.group_tab == (index - 7) as u32);
                }
                2|3=>node.add_action(Action::Click),
                16=>{node.add_action(Action::Click);node.add_action(Action::ScrollIntoView);},
                10..=13 => {
                    node.add_action(Action::ScrollIntoView);
                    node.add_action(Action::Focus);
                    let i = index - 10;
                    node.set_numeric_value(state.sliders[i]);
                    node.set_min_numeric_value(SLIDERS[i].min);
                    node.set_max_numeric_value(SLIDERS[i].max);
                    node.set_numeric_value_step(SLIDERS[i].step);
                    node.add_action(Action::SetValue);
                    node.add_action(Action::Increment);
                    node.add_action(Action::Decrement);
                }
                17 => {
                    node.add_action(Action::ScrollIntoView);
                }
                14 => {
                    node.add_action(Action::ScrollIntoView);
                    node.add_action(Action::Focus);
                    node.set_value(self.number_text.clone());
                    node.add_action(Action::SetValue);
                }
                15 => {
                    node.add_action(Action::ScrollIntoView);
                    node.add_action(Action::Focus);
                    node.set_value(self.displayed_text());
                    node.add_action(Action::SetValue);
                }
                9 => {
                    node.set_children([NodeId(500)]);
                }
                _ => {}
            }
            changed.push((node_id, node));
        }
        let mut content = Node::new(Role::GenericContainer);
        content.set_transform(accesskit::Affine::new([
            g.scale as f64,
            0.0,
            0.0,
            g.scale as f64,
            (g.list.x * g.scale) as f64,
            ((g.list.y
                - f32::from_bits(
                    self.hits
                        .load()
                        .motion
                        .scroll
                        .load(std::sync::atomic::Ordering::Acquire),
                ))
                * g.scale) as f64,
        ]));
        content.set_children(if state.rows <= 1000 {
            (0..state.visible_rows)
                .filter_map(|i| self.model.visible_row(i))
                .map(|row| NodeId(1000 + row as u64 * 2))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        });
        changed.push((NodeId(500), content));
        if (full || rows_changed) && state.rows <= 1000 {
            for ordinal in 0..state.visible_rows {
                let row = self.model.visible_row(ordinal).unwrap();
                let id = NodeId(1000 + row as u64 * 2);
                let visible = NodeId(id.0 + 1);
                let r = Rect::new(
                    0.0,
                    ordinal as f32 * poc_common::ROW_HEIGHT,
                    g.list.w,
                    poc_common::ROW_HEIGHT,
                );
                let mut node = Node::new(Role::ListItem);
                node.set_author_id(poc_common::row_id(row as usize));
                node.set_label(poc_common::label(row as usize));
                node.set_bounds(accesskit::Rect::new(
                    r.x as f64,
                    r.y as f64,
                    (r.x + r.w) as f64,
                    (r.y + r.h) as f64,
                ));
                node.add_action(Action::ScrollIntoView);
                node.set_children([visible]);
                let mut toggle = Node::new(Role::CheckBox);
                toggle.set_author_id(poc_common::visibility_id(row as usize));
                toggle.set_label(poc_common::label(row as usize));
                toggle.set_toggled(if self.model.row_visible(row) {
                    accesskit::Toggled::True
                } else {
                    accesskit::Toggled::False
                });
                toggle.add_action(Action::Click);
                toggle.set_bounds(accesskit::Rect::new(
                    (r.x + r.w - 72.0) as f64,
                    r.y as f64,
                    (r.x + r.w - 36.0) as f64,
                    (r.y + r.h) as f64,
                ));
                changed.push((id, node));
                changed.push((visible, toggle));
            }
        }
        if self.popup_open {
            for i in 0..8 {
                let id = NodeId(200000 + i as u64);
                roots.push(id);
                let mut node = Node::new(Role::MenuItem);
                node.set_author_id(spec()["menu_ids"][i].as_str().unwrap());
                node.set_label(spec()["menu_items"][i].as_str().unwrap());
                node.add_action(Action::Click);
                let r = g.physical(Rect::new(
                    self.popup_rect.x,
                    self.popup_rect.y+metric("divider")*0.5+i as f32*metric("header_height"),
                    self.popup_rect.w,
                    metric("header_height"),
                ));
                node.set_bounds(accesskit::Rect::new(
                    r.x as f64,
                    r.y as f64,
                    (r.x + r.w) as f64,
                    (r.y + r.h) as f64,
                ));
                changed.push((id, node));
            }
        }
        let mut root = Node::new(Role::Window);
        root.set_label(poc_common::TITLE);
        root.set_children(roots);
        changed.push((NodeId(1), root));
        let focus = node_id(self.focus);
        let focus = if changed.iter().any(|(id, _)| *id == focus) {
            focus
        } else {
            NodeId(1)
        };
        {
            let mut tree = self.tree.lock().unwrap();
            if !self.popup_open {
                tree.nodes.retain(|id, _| id.0 < 200000);
            }
            if rows_changed {
                let visible: HashSet<u32> = (0..state.visible_rows)
                    .filter_map(|i| self.model.visible_row(i))
                    .collect();
                tree.nodes.retain(|id, _| {
                    id.0 < 1000 || id.0 >= 132072 || visible.contains(&(((id.0 - 1000) / 2) as u32))
                });
            }
            for (id, node) in &changed {
                tree.nodes.insert(*id, node.clone());
            }
            tree.focus = Some(focus);
        }
        let update = TreeUpdate {
            nodes: changed,
            tree: Some(TreeInfo::new(NodeId(1))),
            tree_id: TreeId::ROOT,
            focus,
        };
        let queued = self.adapter.lock().unwrap().update_if_active(|| update);
        if let Some(queued) = queued
            && self.accessibility_events.send(queued).is_ok()
        {
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(self.hwnd as *mut _)),
                    ACCESSIBILITY_EVENTS,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
        self.a11y_initial = true;
    }
    fn action(&mut self, request: ActionRequest) -> Result<(), String> {
        let id = request.target_node.0;
        if request.action == Action::ScrollIntoView && (1000..132072).contains(&id) {
            let row = ((id - 1000) / 2) as u32;
            if let Some(ordinal) = (0..self.model.state().visible_rows)
                .find(|i| self.model.visible_row(*i) == Some(row))
            {
                let offset = f32::from_bits(
                    self.hits.load().motion.scroll.load(std::sync::atomic::Ordering::Acquire),
                );
                let next = scroll_into_view(
                    offset,
                    self.geometry.list.h,
                    ordinal as f32 * poc_common::ROW_HEIGHT,
                    poc_common::ROW_HEIGHT,
                );
                if next != offset {
                    let _ = self.comp.send(Event::ScrollTo(next));
                }
            }
            return Ok(());
        }
        let Some(target) = target_from_node(request.target_node) else {
            return Ok(());
        };
        if request.action == Action::ScrollIntoView {
            if let Some((r, _)) = self
                .properties_controls()
                .into_iter()
                .chain(std::iter::once((self.wrap_rect, Target::WrapText)))
                .find(|(_, t)| *t == target)
            {
                let g = self.geometry.properties;
                self.properties_offset = scroll_into_view(
                    self.properties_offset, g.h, r.y - g.y + self.properties_offset, r.h,
                );
            }
            return Ok(());
        }
        match request.action {
            Action::Click => self.activate(target)?,
            Action::Focus => {
                self.focus_to(target);
                if matches!(target, Target::Text | Target::Number)
                    && let Some((r, _)) = self
                        .properties_controls()
                        .into_iter()
                        .find(|(_, t)| *t == target)
                {
                    let g = self.geometry.properties;
                    self.properties_offset = scroll_into_view(
                        self.properties_offset, g.h, r.y - g.y + self.properties_offset, r.h,
                    );
                }
            }
            Action::SetValue => match (target, request.data) {
                (Target::Slider(i), Some(ActionData::NumericValue(value))) => {
                    self.model.set_slider(i, value);
                    self.state = (self.model.state().sliders[i as usize] * 10.0).round() as u16;
                }
                (Target::Text, Some(ActionData::Value(value))) => {
                    self.model.set_text(&value);
                    self.text_cursor = FieldCursor::at_end(&value);
                    self.state = value.chars().count() as u16;
                    self.offset = 0.0;
                }
                (Target::Number, Some(ActionData::Value(value))) => {
                    self.number_text = commit_number_draft(&value, &mut self.model);
                    self.number_cursor = FieldCursor::at_end(&self.number_text);
                }
                _ => {}
            },
            Action::Increment | Action::Decrement => {
                if let Target::Slider(i) = target {
                    let sign = if request.action == Action::Increment {
                        1.0
                    } else {
                        -1.0
                    };
                    self.model.set_slider(
                        i,
                        self.model.state().sliders[i as usize] + SLIDERS[i as usize].step * sign,
                    );
                }
            }
            _ => {}
        }
        Ok(())
    }
}
struct UiChannels {
    events: Receiver<UiEvent>,
    tx: Sender<Transaction>,
    comp: Sender<Event>,
    hits: Arc<ArcSwap<HitMap>>,
    adapter: Arc<Mutex<Adapter>>,
    accessibility_events: Sender<QueuedEvents>,
    tree: Arc<Mutex<A11yTree>>,
    canvas: Arc<OnceLock<Canvas>>,
}
fn run_ui(hwnd: usize, options: AppOptions, channels: UiChannels) -> Result<(), String> {
    let UiChannels {
        events,
        tx,
        comp,
        hits,
        adapter,
        accessibility_events,
        tree,
        canvas,
    } = channels;
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .ok()
            .map_err(|e| e.to_string())?;
    }
    let hwnd_value = HWND(hwnd as *mut _);
    let [w, h] = client_size(hwnd_value);
    let scale = unsafe { GetDpiForWindow(hwnd_value) } as f32 / 96.0;
    let model = Model::new(options.rows as u32)?;
    let number_text = model.state().number.to_string();
    let number_cursor = FieldCursor::at_end(&number_text);
    let geometry=Geometry::new(w,h,scale,model.state().divider_fraction);
    let mut ui = Ui {
        model,
        geometry,
        text: TextSystem::new()?,
        hits,
        adapter,
        accessibility_events,
        tree,
        canvas,
        comp,
        tx,
        hwnd,
        state: 0,
        offset: 0.0,
        range: 0..0,
        focus: Target::Canvas,
        text_cursor: FieldCursor::default(),
        number_cursor,
        number_text,
        preedit: String::new(),
        properties_offset: 0.0,
        text_scroll: 0.0,
        number_scroll: 0.0,
        properties_height: 410.0,
        ready: false,
        a11y_initial: false,
        text_logged: HashSet::new(),
        popup_open: false,
        popup_rect: Rect::default(),
        last_caret: Rect::default(),
        nodes: RetainedTree::default(),
        drawer_x: -metric("drawer_width"),
        wrap_rect: Rect::default(),
    };
    let mut cpu_at = Instant::now();
    let mut stall_at = Instant::now();
    let mut divider_grab = None;
    let mut slider_grab = None;
    let mut properties_grab = None;
    loop {
        if options.ui_stall_ms > 0
            && stall_at.elapsed() >= Duration::from_millis(options.ui_stall_every_ms as u64)
        {
            stall_at = Instant::now();
            let started = qpc();
            let until = stall_at + Duration::from_millis(options.ui_stall_ms as u64);
            while Instant::now() < until {
                std::hint::spin_loop();
            }
            trace(
                &ui.canvas,
                "ui_stall",
                json!({"start_qpc":started,"end_qpc":qpc(),"duration_ms":options.ui_stall_ms}),
            );
        }
        if cpu_at.elapsed() >= Duration::from_secs(1) {
            trace(
                &ui.canvas,
                "thread_cpu",
                json!({"thread":"ui","cpu_ms":cpu_ms()}),
            );
            cpu_at = Instant::now();
        }
        let mut deadline = cpu_at + Duration::from_secs(1);
        if options.ui_stall_ms > 0 {
            deadline = deadline.min(stall_at + Duration::from_millis(options.ui_stall_every_ms as u64));
        }
        let mut select = Select::new();
        select.recv(&events);
        let operation = match select.select_deadline(deadline) {
            Ok(operation) => operation,
            Err(_) => continue,
        };
        let event = match operation.recv(&events) {
            Ok(event) => event,
            Err(_) => break,
        };
        let event_start = qpc();
        let (cause, event_qpc) = match &event {
            UiEvent::NeedRows(_) => ("scroll", event_start),
            UiEvent::Size(..) => ("resize", event_start),
            UiEvent::Pointer(Target::Divider, _, _, _, stamp) => ("divider", *stamp),
            UiEvent::Pointer(Target::Slider(_), _, _, _, stamp) => ("slider", *stamp),
            UiEvent::Pointer(_, _, _, _, stamp) => ("pointer", *stamp),
            UiEvent::Character(_) | UiEvent::Ime(..) | UiEvent::Paste(_) => ("text", event_start),
            _ => ("state", event_start),
        };
        let mut dirty = HashSet::new();
        let mut rows_changed = false;
        let mut accessibility_changed = false;
        match event {
            UiEvent::Shutdown => break,
            UiEvent::ScrollSettled(offset) => {
                ui.offset = offset;
                ui.update_accessibility(false);
            }
            UiEvent::DrawerSettled(x) => {
                ui.drawer_x = x;
                ui.update_accessibility(false);
            }
            UiEvent::ActivateAccess => {
                ui.update_accessibility(true);
            }
            UiEvent::Ready => {
                ui.ready = true;
                dirty.extend([
                    LayerId::Title,
                    LayerId::Drawer,
                    LayerId::Layers,
                    LayerId::List,
                    LayerId::Properties,
                ]);
                rows_changed = true;
                trace(
                    &ui.canvas,
                    "startup",
                    json!({"milestone":"ui_ready","frequency":frequency()}),
                );
            }
            UiEvent::Size(w, h, s) => {
                ui.geometry = Geometry::new(w, h, s, ui.model.state().divider_fraction);
                ui.state = w.min(u16::MAX as u32) as u16;
                dirty.extend([
                    LayerId::Title,
                    LayerId::Drawer,
                    LayerId::Layers,
                    LayerId::List,
                    LayerId::Properties,
                ]);
            }
            UiEvent::NeedRows(offset) => {
                ui.offset = offset;
                let next = recorded_rows(offset, ui.geometry.list.h, ui.model.state().visible_rows);
                if next != ui.range {
                    dirty.insert(LayerId::List);
                }
            }
            UiEvent::WheelProperties(delta) => {
                ui.properties_offset = (ui.properties_offset + delta).clamp(
                    0.0,
                    (ui.properties_height - ui.geometry.properties.h).max(0.0),
                );
                dirty.insert(LayerId::Properties);
            }
            UiEvent::ClosePopup => {if ui.popup_open{ui.set_popup_open(false);let _=ui.comp.send(Event::Popup(None));accessibility_changed=true;}}
            UiEvent::Access(request) => {
                let target = target_from_node(request.target_node);
                let action = request.action;
                accessibility_changed = true;
                rows_changed = target == Some(Target::Text)||target.is_some_and(|target|activation_invalidation(target).is_some_and(|change|change.rows));
                ui.action(request)?;
                match target {

                    Some(Target::Menu) if action == Action::ScrollIntoView => {
                        dirty.insert(LayerId::Properties);
                    }
                    Some(Target::Slider(_) | Target::Number | Target::Text | Target::WrapText) => {
                        dirty.insert(LayerId::Properties);
                    }
                    Some(target)=>{if let Some(change)=activation_invalidation(target){dirty.insert(change.layer);}},
                    None=>{},
                }
                if rows_changed {
                    dirty.insert(LayerId::List);
                }
            }
            UiEvent::Pointer(target, x, y, phase, _) => {
                if phase == 3 {
                    match target {
                        Target::Divider => divider_grab = None,
                        Target::Slider(i) if slider_grab == Some(i) => slider_grab = None,
                        Target::Properties => properties_grab = None,
                        _ => {}
                    }
                    continue;
                }

                if phase == 0 {
                    if ui.focus == Target::Number && target != Target::Number {
                        dirty.insert(LayerId::Properties);
                    }
                    ui.focus_to(target);
                    if target == Target::Divider {
                        divider_grab = Some(y - ui.geometry.divider.y);
                    }
                    if let Target::Slider(i) = target {
                        slider_grab = Some(i);
                    }
                    if target == Target::Properties {
                        properties_grab = Some(y);
                    }
                    if matches!(target, Target::Text | Target::Number) {
                        let value = ui.editor_value().to_owned();
                        *ui.editor_cursor() = FieldCursor::at_end(&value);
                        dirty.insert(LayerId::Properties);
                    }

                }
                if let Some(prior) = properties_grab {
                    ui.properties_offset = (ui.properties_offset + prior - y).clamp(
                        0.0,
                        (ui.properties_height - ui.geometry.properties.h).max(0.0),
                    );
                    properties_grab = Some(y);
                    dirty.insert(LayerId::Properties);
                }
                if let Some(grab) = divider_grab
                    && ui
                        .model
                        .set_divider(y - grab - ui.geometry.column.y, ui.geometry.column.h)
                {
                    ui.geometry = Geometry::new(
                        (ui.geometry.width * ui.geometry.scale).round() as u32,
                        (ui.geometry.height * ui.geometry.scale).round() as u32,
                        ui.geometry.scale,
                        ui.model.state().divider_fraction,
                    );
                    ui.state = (ui.geometry.divider.y * ui.geometry.scale).round() as u16;
                    dirty.extend([LayerId::Layers, LayerId::Properties]);
                    if needs_rows(
                        &ui.range,
                        ui.offset,
                        ui.geometry.list.h,
                        ui.model.state().visible_rows,
                    ) {
                        dirty.insert(LayerId::List);
                    }
                }
                if let Some(i) = slider_grab
                    && let Some((r, _)) = ui
                        .properties_controls()
                        .into_iter()
                        .find(|(_, t)| *t == Target::Slider(i))
                {
                    let fraction = ((x - r.x - 8.0) / (r.w - 16.0)).clamp(0.0, 1.0) as f64;
                    let s = SLIDERS[i as usize];
                    if ui.model.set_slider(i, s.min + (s.max - s.min) * fraction) {
                        ui.state = (ui.model.state().sliders[i as usize] * 10.0).round() as u16;
                        dirty.insert(LayerId::Properties);
                    }
                }
                if phase == 2 {
                    if divider_grab.is_none() && slider_grab.is_none() {
                        ui.activate(target)?;
                        accessibility_changed = true;
                        if let Some(change)=activation_invalidation(target){dirty.insert(change.layer);rows_changed|=change.rows;}
                    }
                    divider_grab = None;
                    slider_grab = None;
                    properties_grab = None;
                }
            }
            UiEvent::Character(ch) => {
                if matches!(ui.focus, Target::Text | Target::Number) {
                    ui.edit(&ch.to_string());
                    dirty.insert(LayerId::Properties);
                    if ui.focus == Target::Text {
                        ui.offset = 0.0;
                        dirty.insert(LayerId::List);
                        rows_changed = true;
                    }
                }
            }
            UiEvent::Paste(value) => {
                if matches!(ui.focus, Target::Text | Target::Number) {
                    ui.edit(&value);
                    dirty.insert(LayerId::Properties);
                    if ui.focus == Target::Text {
                        dirty.insert(LayerId::List);
                        rows_changed = true;
                    }
                }
            }
            UiEvent::Ime(value, committed) => {
                if ui.focus == Target::Text {
                    if committed {
                        ui.edit(&value);
                        dirty.insert(LayerId::List);
                        rows_changed = true;
                    } else {
                        ui.preedit = value;
                    }
                    dirty.insert(LayerId::Properties);
                }
            }
            UiEvent::Key(key, ctrl, shift) => {
                if key == VK_PACKET.0 as u32 {
                    continue;
                }
                if key == VK_ESCAPE.0 as u32 && ui.popup_open {
                    ui.set_popup_open(false);
                    let _ = ui.comp.send(Event::Popup(None));
                    accessibility_changed = true;
                }
                if matches!(ui.focus, Target::Text | Target::Number) {
                    let value = ui.editor_value().to_owned();
                    let focus = ui.focus;
                    let cursor = (ui.editor_cursor().anchor, ui.editor_cursor().caret);
                    let preedit_empty = ui.preedit.is_empty();
                    if key == VK_ESCAPE.0 as u32 && ui.focus == Target::Number {
                        ui.number_text = ui.model.state().number.to_string();
                        ui.number_cursor = FieldCursor::at_end(&ui.number_text);
                    } else if ctrl && key == VK_A.0 as u32 {
                        ui.editor_cursor().select_all(&value);
                    } else if ctrl && (key == VK_C.0 as u32 || key == VK_X.0 as u32) {
                        let range = ui.editor_cursor().selection();
                        let text = value[range].to_owned();
                        unsafe {
                            let _ = PostMessageW(
                                Some(hwnd_value),
                                COPY_TEXT,
                                WPARAM(0),
                                LPARAM(Box::into_raw(Box::new(text)) as isize),
                            );
                        }
                        if key == VK_X.0 as u32 {
                            ui.edit("");
                        }
                    } else if key == VK_BACK.0 as u32 {
                        ui.erase(true);
                    } else if key == VK_DELETE.0 as u32 {
                        ui.erase(false);
                    } else if key == VK_LEFT.0 as u32 {
                        ui.editor_cursor().left(&value, shift);
                    } else if key == VK_RIGHT.0 as u32 {
                        ui.editor_cursor().right(&value, shift);
                    } else if key == VK_HOME.0 as u32 {
                        ui.editor_cursor().move_to(0, shift);
                    } else if key == VK_END.0 as u32 {
                        ui.editor_cursor().move_to(value.len(), shift);
                    } else if key == VK_RETURN.0 as u32 && ui.focus == Target::Number {
                        ui.commit_number();
                    } else if key == VK_TAB.0 as u32 {
                        let next = if ui.focus == Target::Number {
                            if shift {
                                Target::Slider(3)
                            } else {
                                Target::Text
                            }
                        } else if shift {
                            Target::Number
                        } else {
                            Target::Menu
                        };
                        ui.focus_to(next);
                    }
                    if focus == Target::Text && value != ui.model.text() {
                        dirty.insert(LayerId::List);
                        rows_changed = true;
                    }
                    if focus != ui.focus || value != ui.editor_value()
                        || cursor != (ui.editor_cursor().anchor, ui.editor_cursor().caret)
                        || preedit_empty != ui.preedit.is_empty()
                    {
                        dirty.insert(LayerId::Properties);
                    }
                }
            }
        }
        if !dirty.is_empty() {
            ui.publish(dirty, rows_changed, event_start, cause, event_qpc)?;
        } else if accessibility_changed {
            ui.update_accessibility(rows_changed);
        }
    }
    Ok(())
}
pub fn run() -> Result<(), String> {
    let options = AppOptions::parse(&std::env::args().skip(1).collect::<Vec<_>>())?;
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        EnableMouseInPointer(true).map_err(|e| e.to_string())?;
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .ok()
            .map_err(|e| e.to_string())?;
    }
    let (ui_tx, ui_rx) = unbounded();
    let (comp_tx, comp_rx) = unbounded();
    let (tx, rx) = unbounded();
    let (accessibility_tx, accessibility_rx) = unbounded();
    let canvas = Arc::new(OnceLock::new());
    let hits = Arc::new(ArcSwap::from_pointee(HitMap::default()));
    let tree = Arc::new(Mutex::new(A11yTree::default()));
    let instance = unsafe { GetModuleHandleW(None) }.map_err(|e| e.to_string())?;
    let class = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance.into(),
        lpszClassName: w!("CapyPocRetained"),
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.map_err(|e| e.to_string())?,
        ..Default::default()
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(windows::core::Error::from_thread().to_string());
    }
    let window_title: Vec<u16> = poc_common::TITLE.encode_utf16().chain(Some(0)).collect();
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP,
            w!("CapyPocRetained"),
            PCWSTR(window_title.as_ptr()),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            1600,
            1000,
            None,
            None,
            Some(instance.into()),
            None,
        )
    }
    .map_err(|e| e.to_string())?;
    let adapter = Arc::new(Mutex::new(Adapter::new(
        hwnd,
        false,
        Actions(ui_tx.clone()),
    )));
    let mut state = Box::new(WindowState {
        ui: ui_tx.clone(),
        comp: comp_tx.clone(),
        hits: hits.clone(),
        canvas: canvas.clone(),
        adapter: adapter.clone(),
        accessibility_events: accessibility_rx,
        activation: Activation {
            tree: tree.clone(),
            ui: ui_tx.clone(),
        },
        contacts: HashMap::new(),
        scale: unsafe { GetDpiForWindow(hwnd) } as f32 / 96.0,
        last_cpu: Instant::now(),
        characters:Utf16Input::default(),
        caption: CaptionPress::default(),
        closing: false,
    });
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, (&mut *state) as *mut _ as isize);
        SetTimer(Some(hwnd), 1, 1000, None);
    }
    let physical = [
        (poc_common::CLIENT[0] as f32 * state.scale).round() as i32,
        (poc_common::CLIENT[1] as f32 * state.scale).round() as i32,
    ];
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            physical[0],
            physical[1] + 1,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
    let raw = hwnd.0 as usize;
    let comp_canvas = canvas.clone();
    let comp_ui = ui_tx.clone();
    let comp_options = options.clone();
    let compositor = std::thread::Builder::new()
        .name("poc-compositor".into())
        .spawn(move || {
            let result = render::run(raw, comp_options, rx, comp_rx, comp_ui, comp_canvas);
            if result.is_err() {
                unsafe {
                    let _ = PostMessageW(
                        Some(HWND(raw as *mut _)),
                        CLOSE_WINDOW,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            }
            result
        })
        .map_err(|e| e.to_string())?;
    let ui_canvas = canvas.clone();
    let ui_comp = comp_tx.clone();
    let ui_worker = std::thread::Builder::new()
        .name("poc-ui".into())
        .spawn(move || {
            let result = run_ui(
                raw,
                options,
                UiChannels {
                    events: ui_rx,
                    tx,
                    comp: ui_comp,
                    hits,
                    adapter,
                    accessibility_events: accessibility_tx,
                    tree,
                    canvas: ui_canvas,
                },
            );
            if result.is_err() {
                unsafe {
                    let _ = PostMessageW(Some(HWND(raw as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            }
            result
        })
        .map_err(|e| e.to_string())?;
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);
    }
    let mut message = MSG::default();
    while unsafe { GetMessageW(&mut message, None, 0, 0) }.as_bool() {
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    let _ = ui_tx.send(UiEvent::Shutdown);
    let _ = comp_tx.send(Event::Shutdown);
    let ui_result = ui_worker.join().map_err(|_| "UI thread panicked")?;
    let comp_result = compositor
        .join()
        .map_err(|_| "compositor thread panicked")?;
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
    }
    drop(state);
    let shared =
        Arc::try_unwrap(canvas).map_err(|_| "canvas still retained after host shutdown")?;
    let canvas = shared.into_inner().ok_or("canvas was never initialized")?;
    let canvas_result = canvas.finish();
    canvas_result?;
    ui_result?;
    comp_result?;
    Ok(())
}

#[cfg(test)]
mod input_tests {
    use super::*;
    #[test]
    fn capture_loss_cancels_a_widget_contact_instead_of_invoking_it() {
        let mut model = Model::new(1).unwrap();
        let phase = pointer_phase(WM_POINTERCAPTURECHANGED);
        if phase == 2 {
            model.activate(2, 0);
        }
        assert!(!model.state().drawer_open);
        assert_eq!(phase, 3);
        if pointer_phase(WM_POINTERUP) == 2 {
            model.activate(2, 0);
        }
        assert!(model.state().drawer_open);
    }
}
