//! Native selection scrolling until Reactor exposes an imperative bring-into-view API.
//! UI Automation calls must run away from the thread hosting their own provider.
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use windows::{
    Win32::{
        Foundation::HWND,
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize,
            },
            Variant::VARIANT,
        },
        UI::Accessibility::{
            CUIAutomation8, IUIAutomation, IUIAutomation2, IUIAutomationScrollItemPattern,
            IUIAutomationSelectionItemPattern, TreeScope_Descendants, UIA_AutomationIdPropertyId,
            UIA_ScrollItemPatternId, UIA_SelectionItemPatternId,
        },
    },
    core::{Interface, Result},
};

struct Request {
    window: usize,
    row: String,
}
type Queue = Arc<(Mutex<Option<Request>>, Condvar)>;
static QUEUE: OnceLock<Option<Queue>> = OnceLock::new();

pub(crate) fn request(window: usize, row: String) {
    if std::env::var_os("CONSOLE_UI_TRACE_SCROLL").is_some() {
        eprintln!("Native scroll queued for window {window:x}: {row}");
    }
    let queue = QUEUE.get_or_init(|| {
        let queue: Queue = Arc::new((Mutex::new(None), Condvar::new()));
        let worker = queue.clone();
        match std::thread::Builder::new()
            .name("native-selection-scroll".into())
            .spawn(move || {
                if let Err(error) = run(worker) {
                    eprintln!("Initialize native selection scrolling: {error}");
                }
            }) {
            Ok(_) => Some(queue),
            Err(error) => {
                eprintln!("Start native selection scrolling: {error}");
                None
            }
        }
    });
    if let Some(queue) = queue {
        // Coalesce key repeat to the most recent selection instead of queuing RPCs.
        *queue.0.lock().unwrap() = Some(Request { window, row });
        queue.1.notify_one();
    }
}

fn run(queue: Queue) -> Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
    }
    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe {
                CoUninitialize();
            }
        }
    }
    let _apartment = Apartment;
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)? };
    let timeouts: IUIAutomation2 = automation.cast()?;
    unsafe {
        timeouts.SetConnectionTimeout(2000)?;
        timeouts.SetTransactionTimeout(2000)?;
    }
    loop {
        let request = {
            let pending = queue.0.lock().unwrap();
            let mut pending = queue
                .1
                .wait_while(pending, |pending| pending.is_none())
                .unwrap();
            pending.take().unwrap()
        };
        let mut process = 0;
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(
                request.window as _,
                &mut process,
            );
        }
        if process != std::process::id() {
            eprintln!(
                "Native scroll skipped: window belongs to process {process}, expected {}",
                std::process::id()
            );
            continue;
        }
        if let Err(error) = scroll(&automation, &request) {
            // A closed tab/window or a new session removes its uniquely named row.
            eprintln!("Bring selected row into view: {error}");
        }
    }
}

fn scroll(automation: &IUIAutomation, request: &Request) -> Result<()> {
    unsafe {
        let window = automation.ElementFromHandle(HWND(request.window as _))?;
        let condition = automation.CreatePropertyCondition(
            UIA_AutomationIdPropertyId,
            &VARIANT::from(request.row.as_str()),
        )?;
        let row = window.FindFirst(TreeScope_Descendants, &condition)?;
        let selected: IUIAutomationSelectionItemPattern =
            row.GetCurrentPatternAs(UIA_SelectionItemPatternId)?;
        if selected.CurrentIsSelected()?.as_bool() {
            let scroll: IUIAutomationScrollItemPattern =
                row.GetCurrentPatternAs(UIA_ScrollItemPatternId)?;
            scroll.ScrollIntoView()?;
            if std::env::var_os("CONSOLE_UI_TRACE_SCROLL").is_some() {
                eprintln!("Native scroll completed: {}", request.row);
            }
        } else if std::env::var_os("CONSOLE_UI_TRACE_SCROLL").is_some() {
            eprintln!("Native scroll skipped deselected row: {}", request.row);
        }
    }
    Ok(())
}
