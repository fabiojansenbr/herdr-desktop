//! Client for `zwlr_virtual_pointer_v1` on a private Wayland display and
//! pointer injection comparator for the native test harness (spec 007 pointer-prep).

use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use wayland_client::protocol::wl_pointer::{Axis, AxisSource, ButtonState};
use wayland_client::protocol::wl_registry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1;
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1;

/// Standard Linux evdev button codes (used by wl_pointer / virtual pointer protocol).
pub const BTN_LEFT: u32 = 0x110; // 272
pub const BTN_RIGHT: u32 = 0x111; // 273
pub const BTN_MIDDLE: u32 = 0x112; // 274

#[derive(Default)]
struct State {
    manager: Option<ZwlrVirtualPointerManagerV1>,
    seat: Option<WlSeat>,
    pointer: Option<ZwlrVirtualPointerV1>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            if interface == "zwlr_virtual_pointer_manager_v1" {
                let mgr = registry.bind::<ZwlrVirtualPointerManagerV1, _, _>(
                    name,
                    version.min(2),
                    qh,
                    (),
                );
                state.manager = Some(mgr);
            } else if interface == "wl_seat" && state.seat.is_none() {
                let seat = registry.bind::<WlSeat, _, _>(name, version.min(7), qh, ());
                state.seat = Some(seat);
            }
        }
    }
}

impl Dispatch<ZwlrVirtualPointerManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrVirtualPointerManagerV1,
        _: <ZwlrVirtualPointerManagerV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrVirtualPointerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrVirtualPointerV1,
        _: <ZwlrVirtualPointerV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlSeat,
        _: <WlSeat as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// A client connection that creates and controls a virtual pointer device via `zwlr_virtual_pointer_v1`.
pub struct VirtualPointerClient {
    _conn: Connection,
    event_queue: wayland_client::EventQueue<State>,
    state: State,
    start_time: Instant,
}

impl VirtualPointerClient {
    /// Connects directly to the private Wayland AF_UNIX socket (never falls back to user display).
    pub fn connect(socket_path: &Path) -> Result<Self, String> {
        if !socket_path.is_absolute() {
            return Err(format!(
                "wayland socket path {} is not absolute",
                socket_path.display()
            ));
        }
        let stream = UnixStream::connect(socket_path)
            .map_err(|e| format!("connect to {}: {e}", socket_path.display()))?;
        let conn = Connection::from_socket(stream)
            .map_err(|e| format!("wayland Connection::from_socket: {e}"))?;

        let mut event_queue = conn.new_event_queue();
        let qh = event_queue.handle();
        let display = conn.display();
        let _registry = display.get_registry(&qh, ());

        let mut state = State::default();
        event_queue
            .roundtrip(&mut state)
            .map_err(|e| format!("registry roundtrip: {e}"))?;

        let mgr = state
            .manager
            .as_ref()
            .ok_or("compositor does not advertise zwlr_virtual_pointer_manager_v1")?;

        let pointer = mgr.create_virtual_pointer(state.seat.as_ref(), &qh, ());
        state.pointer = Some(pointer);

        event_queue
            .roundtrip(&mut state)
            .map_err(|e| format!("pointer creation roundtrip: {e}"))?;

        Ok(Self {
            _conn: conn,
            event_queue,
            state,
            start_time: Instant::now(),
        })
    }

    fn time_ms(&self) -> u32 {
        self.start_time.elapsed().as_millis() as u32
    }

    pub fn pointer(&self) -> Result<&ZwlrVirtualPointerV1, String> {
        self.state
            .pointer
            .as_ref()
            .ok_or_else(|| "virtual pointer not initialized".to_string())
    }

    /// Absolute pointer motion in compositor layout space `(0..x_extent, 0..y_extent)`.
    pub fn motion_absolute(
        &mut self,
        x: u32,
        y: u32,
        x_extent: u32,
        y_extent: u32,
    ) -> Result<(), String> {
        let p = self.pointer()?;
        let t = self.time_ms();
        p.motion_absolute(t, x, y, x_extent, y_extent);
        p.frame();
        self.flush()
    }

    /// Relative pointer motion by displacement `(dx, dy)`.
    pub fn motion(&mut self, dx: f64, dy: f64) -> Result<(), String> {
        let p = self.pointer()?;
        let t = self.time_ms();
        p.motion(t, dx, dy);
        p.frame();
        self.flush()
    }

    /// Press a pointer button (evdev code e.g. [`BTN_LEFT`]).
    pub fn button_press(&mut self, button: u32) -> Result<(), String> {
        let p = self.pointer()?;
        let t = self.time_ms();
        p.button(t, button, ButtonState::Pressed);
        p.frame();
        self.flush()
    }

    /// Release a pointer button.
    pub fn button_release(&mut self, button: u32) -> Result<(), String> {
        let p = self.pointer()?;
        let t = self.time_ms();
        p.button(t, button, ButtonState::Released);
        p.frame();
        self.flush()
    }

    /// Convenience click sequence: move to `(x, y)`, press, then release.
    pub fn click(
        &mut self,
        x: u32,
        y: u32,
        x_extent: u32,
        y_extent: u32,
        button: u32,
    ) -> Result<(), String> {
        self.motion_absolute(x, y, x_extent, y_extent)?;
        std::thread::sleep(std::time::Duration::from_millis(50));
        self.button_press(button)?;
        std::thread::sleep(std::time::Duration::from_millis(50));
        self.button_release(button)?;
        std::thread::sleep(std::time::Duration::from_millis(50));
        Ok(())
    }

    /// Send vertical scroll axis event.
    pub fn axis_vertical(&mut self, value: f64, discrete: Option<i32>) -> Result<(), String> {
        let p = self.pointer()?;
        let t = self.time_ms();
        p.axis_source(AxisSource::Wheel);
        if let Some(steps) = discrete {
            p.axis_discrete(t, Axis::VerticalScroll, value, steps);
        } else {
            p.axis(t, Axis::VerticalScroll, value);
        }
        p.frame();
        self.flush()
    }

    /// Convenience wheel up: move to `(x, y)`, send wheel-up notches (negative axis value in Wayland).
    pub fn wheel_up(
        &mut self,
        x: u32,
        y: u32,
        x_extent: u32,
        y_extent: u32,
        notches: u32,
    ) -> Result<(), String> {
        self.motion_absolute(x, y, x_extent, y_extent)?;
        std::thread::sleep(std::time::Duration::from_millis(50));
        for _ in 0..notches {
            self.axis_vertical(-15.0, Some(-1))?;
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        Ok(())
    }

    pub fn flush(&mut self) -> Result<(), String> {
        self.event_queue
            .roundtrip(&mut self.state)
            .map_err(|e| format!("flush roundtrip: {e}"))?;
        Ok(())
    }
}

impl Drop for VirtualPointerClient {
    fn drop(&mut self) {
        if let Some(p) = self.state.pointer.take() {
            p.destroy();
        }
        if let Some(mgr) = self.state.manager.take() {
            mgr.destroy();
        }
        let _ = self.event_queue.roundtrip(&mut self.state);
    }
}

/// Recorded DOM event from the test page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordedEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub button: Option<i32>,
    pub client_x: f64,
    pub client_y: f64,
    pub delta_y: Option<f64>,
    pub is_trusted: bool,
}

/// Method observation result during comparison.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MethodObservation {
    pub method: String,
    pub injection: String,
    pub observed_events: Vec<RecordedEvent>,
    pub pointerdown_count: usize,
    pub pointerup_count: usize,
    pub wheel_count: usize,
    pub status: String,
}

/// Report containing observations across tested pointer injection methods.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComparisonReport {
    pub observations: Vec<MethodObservation>,
}

impl ComparisonReport {
    pub fn markdown_table(&self) -> String {
        let mut table = String::from(
            "| Método | Ações de Injeção | pointerdown | pointerup | wheel | Total | Veredito |\n\
             |---|---|---|---|---|---|---|\n",
        );
        for obs in &self.observations {
            table.push_str(&format!(
                "| `{}` | {} | {} | {} | {} | {} | {} |\n",
                obs.method,
                obs.injection,
                obs.pointerdown_count,
                obs.pointerup_count,
                obs.wheel_count,
                obs.observed_events.len(),
                obs.status
            ));
        }
        table
    }
}

/// Minimal test HTML page with pointerdown, pointerup, and wheel event recording.
pub const HTML_TEST_PAGE: &str = r#"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<style>
  body, html { margin: 0; padding: 0; width: 100vw; height: 100vh; background: #eef; overflow: hidden; }
  #target { width: 100vw; height: 100vh; display: flex; align-items: center; justify-content: center; font-size: 20px; font-family: monospace; }
</style>
</head>
<body>
<div id="target">Interactive Test Target</div>
<script>
window.__events = [];
function recordEvent(ev) {
  const data = {
    type: ev.type,
    button: ev.button !== undefined ? ev.button : null,
    client_x: ev.clientX,
    client_y: ev.clientY,
    delta_y: ev.deltaY !== undefined ? ev.deltaY : null,
    is_trusted: ev.isTrusted
  };
  window.__events.push(data);
  const jsonStr = JSON.stringify(data);
  console.log("EVENT:" + jsonStr);
  document.title = "EVENT:" + jsonStr;
}
window.addEventListener("pointerdown", recordEvent);
window.addEventListener("pointerup", recordEvent);
window.addEventListener("wheel", recordEvent);
</script>
</body>
</html>"#;

/// Minimal WebKitGTK 4.1 test window runner script.
pub const WINDOW_RUNNER_PY: &str = r#"#!/usr/bin/env python3
import sys, os, gi
gi.require_version('Gtk', '3.0')
gi.require_version('WebKit2', '4.1')
from gi.repository import Gtk, WebKit2

events_file = sys.argv[1]
html_path = sys.argv[2]

win = Gtk.Window()
win.set_title("Pointer Test Window")
win.set_default_size(1280, 720)
win.connect('destroy', Gtk.main_quit)

webview = WebKit2.WebView()
settings = webview.get_settings()
settings.set_enable_write_console_messages_to_stdout(True)

def on_title(view, param):
    t = view.get_title()
    if t and t.startswith("EVENT:"):
        with open(events_file, "a", encoding="utf-8") as f:
            f.write(t[6:] + "\n")
            f.flush()

webview.connect('notify::title', on_title)
webview.load_uri("file://" + html_path)
win.add(webview)
win.show_all()
Gtk.main()
"#;
