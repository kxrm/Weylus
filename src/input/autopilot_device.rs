use autopilot::geometry::Size;
use autopilot::mouse;
use autopilot::mouse::ScrollDirection;
use autopilot::screen::size as screen_size;

use tracing::warn;

use crate::input::device::{InputDevice, InputDeviceType};
use crate::protocol::{Button, KeyboardEvent, KeyboardEventType, PointerEvent, PointerEventType, WheelEvent};

use crate::capturable::{Capturable, Geometry};

#[cfg(target_os = "macos")]
use core_graphics::event::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton, EventField};
#[cfg(target_os = "macos")]
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
#[cfg(target_os = "macos")]
use core_graphics::geometry::CGPoint;

// Mouse event subtype for tablet point events
#[cfg(target_os = "macos")]
const MOUSE_EVENT_SUBTYPE_TABLET_POINT: i64 = 1;

// Tablet pointer types
#[cfg(target_os = "macos")]
const TABLET_POINTER_TYPE_PEN: i64 = 1;

// Capability mask bits from IOLLEvent.h - indicates what data the tablet provides
// NX_TABLET_CAPABILITY_ABSXMASK = 0x0002
// NX_TABLET_CAPABILITY_ABSYMASK = 0x0004
// NX_TABLET_CAPABILITY_PRESSUREMASK = 0x0400
#[cfg(target_os = "macos")]
const TABLET_CAPABILITY_MASK: i64 = 0x0406; // X + Y + Pressure

// Virtual device IDs (arbitrary but consistent)
#[cfg(target_os = "macos")]
const VIRTUAL_VENDOR_ID: i64 = 0x1234;
#[cfg(target_os = "macos")]
const VIRTUAL_TABLET_ID: i64 = 0x0001;
#[cfg(target_os = "macos")]
const VIRTUAL_DEVICE_ID: i64 = 1;

pub struct AutoPilotDevice {
    capturable: Box<dyn Capturable>,
    left_button_down: bool,
    #[cfg(target_os = "macos")]
    in_proximity: bool,
}

impl AutoPilotDevice {
    pub fn new(capturable: Box<dyn Capturable>) -> Self {
        Self {
            capturable,
            left_button_down: false,
            #[cfg(target_os = "macos")]
            in_proximity: false,
        }
    }

    #[cfg(target_os = "macos")]
    fn send_tablet_proximity_event(&self, entering: bool) {
        if let Ok(source) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) {
            if let Ok(event) = CGEvent::new(source) {
                event.set_type(CGEventType::TabletProximity);

                // Set proximity state (1 = entering, 0 = leaving)
                event.set_integer_value_field(
                    EventField::TABLET_PROXIMITY_EVENT_ENTER_PROXIMITY,
                    if entering { 1 } else { 0 }
                );

                // Set device identification
                event.set_integer_value_field(EventField::TABLET_PROXIMITY_EVENT_VENDOR_ID, VIRTUAL_VENDOR_ID);
                event.set_integer_value_field(EventField::TABLET_PROXIMITY_EVENT_TABLET_ID, VIRTUAL_TABLET_ID);
                event.set_integer_value_field(EventField::TABLET_PROXIMITY_EVENT_DEVICE_ID, VIRTUAL_DEVICE_ID);
                event.set_integer_value_field(EventField::TABLET_PROXIMITY_EVENT_SYSTEM_TABLET_ID, VIRTUAL_DEVICE_ID);
                event.set_integer_value_field(EventField::TABLET_PROXIMITY_EVENT_POINTER_ID, 1);

                // Set pointer type to pen
                event.set_integer_value_field(EventField::TABLET_PROXIMITY_EVENT_POINTER_TYPE, TABLET_POINTER_TYPE_PEN);
                event.set_integer_value_field(EventField::TABLET_PROXIMITY_EVENT_VENDOR_POINTER_TYPE, TABLET_POINTER_TYPE_PEN);

                // Set capability mask (indicates pressure support)
                event.set_integer_value_field(EventField::TABLET_PROXIMITY_EVENT_CAPABILITY_MASK, TABLET_CAPABILITY_MASK);

                event.post(CGEventTapLocation::HID);
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn send_mouse_event(&self, event_type: CGEventType, x: f64, y: f64, pressure: f64) {
        let point = CGPoint::new(x, y);
        if let Ok(source) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) {
            if let Ok(event) = CGEvent::new_mouse_event(source, event_type, point, CGMouseButton::Left) {
                // Set tablet subtype for pen-like input
                event.set_integer_value_field(EventField::MOUSE_EVENT_SUB_TYPE, MOUSE_EVENT_SUBTYPE_TABLET_POINT);

                // Set pressure values
                event.set_double_value_field(EventField::MOUSE_EVENT_PRESSURE, pressure);
                event.set_double_value_field(EventField::TABLET_EVENT_POINT_PRESSURE, pressure);

                // Link to the virtual tablet device
                event.set_integer_value_field(EventField::TABLET_EVENT_DEVICE_ID, VIRTUAL_DEVICE_ID);

                event.post(CGEventTapLocation::HID);
            }
        }
    }
}

impl InputDevice for AutoPilotDevice {
    fn send_wheel_event(&mut self, event: &WheelEvent) {
        match event.dy {
            1..=i32::MAX => mouse::scroll(ScrollDirection::Up, 1),
            i32::MIN..=-1 => mouse::scroll(ScrollDirection::Down, 1),
            0 => {}
        }
    }

    #[cfg(target_os = "macos")]
    fn send_pointer_event(&mut self, event: &PointerEvent) {
        if !event.is_primary {
            return;
        }
        if let Err(err) = self.capturable.before_input() {
            warn!("Failed to activate window, sending no input ({})", err);
            return;
        }
        let (x_rel, y_rel, width_rel, height_rel) = match self.capturable.geometry().unwrap() {
            Geometry::Relative(x, y, width, height) => (x, y, width, height),
        };
        let (_, _, width, height) = match crate::capturable::core_graphics::screen_coordsys() {
            Ok(bounds) => bounds,
            Err(err) => {
                warn!("Could not determine global coordinate system: {}", err);
                return;
            }
        };

        let screen_x = (event.x * width_rel + x_rel) * width;
        let screen_y = (event.y * height_rel + y_rel) * height;

        // Use CoreGraphics directly for proper drag support on macOS
        // Pressure from stylus (0.0 to 1.0)
        let pressure = event.pressure;

        match event.event_type {
            PointerEventType::DOWN => {
                if !self.left_button_down {
                    // Send tablet proximity enter event before first touch
                    if !self.in_proximity {
                        self.in_proximity = true;
                        self.send_tablet_proximity_event(true);
                    }
                    self.left_button_down = true;
                    self.send_mouse_event(CGEventType::LeftMouseDown, screen_x, screen_y, pressure);
                }
            }
            PointerEventType::UP | PointerEventType::CANCEL | PointerEventType::LEAVE | PointerEventType::OUT => {
                if self.left_button_down {
                    self.left_button_down = false;
                    self.send_mouse_event(CGEventType::LeftMouseUp, screen_x, screen_y, 0.0);
                    // Send tablet proximity leave event after pen lifts
                    if self.in_proximity {
                        self.in_proximity = false;
                        self.send_tablet_proximity_event(false);
                    }
                }
            }
            PointerEventType::MOVE | PointerEventType::OVER | PointerEventType::ENTER => {
                // Key fix: use LeftMouseDragged when button is held, MouseMoved otherwise
                if self.left_button_down {
                    self.send_mouse_event(CGEventType::LeftMouseDragged, screen_x, screen_y, pressure);
                } else {
                    self.send_mouse_event(CGEventType::MouseMoved, screen_x, screen_y, 0.0);
                }
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn send_pointer_event(&mut self, event: &PointerEvent) {
        if !event.is_primary {
            return;
        }
        if let Err(err) = self.capturable.before_input() {
            warn!("Failed to activate window, sending no input ({})", err);
            return;
        }
        let (x_rel, y_rel, width_rel, height_rel) = match self.capturable.geometry().unwrap() {
            Geometry::Relative(x, y, width, height) => (x, y, width, height),
            #[cfg(target_os = "windows")]
            _ => {
                warn!("Failed to get window geometry, sending no input");
                return;
            }
        };
        let Size { width, height } = screen_size();
        if let Err(err) = mouse::move_to(autopilot::geometry::Point::new(
            (event.x * width_rel + x_rel) * width,
            (event.y * height_rel + y_rel) * height,
        )) {
            warn!("Could not move mouse: {}", err);
        }

        match event.event_type {
            PointerEventType::DOWN => {
                if !self.left_button_down {
                    self.left_button_down = true;
                    mouse::toggle(mouse::Button::Left, true);
                }
            }
            PointerEventType::UP | PointerEventType::CANCEL | PointerEventType::LEAVE | PointerEventType::OUT => {
                if self.left_button_down {
                    self.left_button_down = false;
                    mouse::toggle(mouse::Button::Left, false);
                }
            }
            _ => {}
        }
    }

    fn send_keyboard_event(&mut self, event: &KeyboardEvent) {
        use autopilot::key::{Character, Code, KeyCode};

        let state = match event.event_type {
            KeyboardEventType::UP => false,
            KeyboardEventType::DOWN => true,
            // autopilot doesn't handle this, so just do nothing
            KeyboardEventType::REPEAT => return,
        };

        fn map_key(code: &str) -> Option<KeyCode> {
            match code {
                "Escape" => Some(KeyCode::Escape),
                "Enter" => Some(KeyCode::Return),
                "Backspace" => Some(KeyCode::Backspace),
                "Tab" => Some(KeyCode::Tab),
                "Space" => Some(KeyCode::Space),
                "CapsLock" => Some(KeyCode::CapsLock),
                "F1" => Some(KeyCode::F1),
                "F2" => Some(KeyCode::F2),
                "F3" => Some(KeyCode::F3),
                "F4" => Some(KeyCode::F4),
                "F5" => Some(KeyCode::F5),
                "F6" => Some(KeyCode::F6),
                "F7" => Some(KeyCode::F7),
                "F8" => Some(KeyCode::F8),
                "F9" => Some(KeyCode::F9),
                "F10" => Some(KeyCode::F10),
                "F11" => Some(KeyCode::F11),
                "F12" => Some(KeyCode::F12),
                "F13" => Some(KeyCode::F13),
                "F14" => Some(KeyCode::F14),
                "F15" => Some(KeyCode::F15),
                "F16" => Some(KeyCode::F16),
                "F17" => Some(KeyCode::F17),
                "F18" => Some(KeyCode::F18),
                "F19" => Some(KeyCode::F19),
                "F20" => Some(KeyCode::F20),
                "F21" => Some(KeyCode::F21),
                "F22" => Some(KeyCode::F22),
                "F23" => Some(KeyCode::F23),
                "F24" => Some(KeyCode::F24),
                "Home" => Some(KeyCode::Home),
                "ArrowUp" => Some(KeyCode::UpArrow),
                "PageUp" => Some(KeyCode::PageUp),
                "ArrowLeft" => Some(KeyCode::LeftArrow),
                "ArrowRight" => Some(KeyCode::RightArrow),
                "End" => Some(KeyCode::End),
                "ArrowDown" => Some(KeyCode::DownArrow),
                "PageDown" => Some(KeyCode::PageDown),
                "Delete" => Some(KeyCode::Delete),
                "ControlLeft" | "ControlRight" => Some(KeyCode::Control),
                "AltLeft" | "AltRight" => Some(KeyCode::Alt),
                "MetaLeft" | "MetaRight" => Some(KeyCode::Meta),
                "ShiftLeft" | "ShiftRight" => Some(KeyCode::Shift),
                _ => None,
            }
        }
        let key = map_key(&event.code);
        let mut flags = Vec::new();
        if event.ctrl {
            flags.push(autopilot::key::Flag::Control);
        }
        if event.alt {
            flags.push(autopilot::key::Flag::Alt);
        }
        if event.meta {
            flags.push(autopilot::key::Flag::Meta);
        }
        if event.shift {
            flags.push(autopilot::key::Flag::Shift);
        }
        match key {
            Some(key) => autopilot::key::toggle(&Code(key), state, &flags, 0),
            None => {
                for c in event.key.chars() {
                    autopilot::key::toggle(&Character(c), state, &flags, 0);
                }
            }
        }
    }

    fn set_capturable(&mut self, capturable: Box<dyn Capturable>) {
        self.capturable = capturable;
    }

    fn device_type(&self) -> InputDeviceType {
        InputDeviceType::AutoPilotDevice
    }
}
