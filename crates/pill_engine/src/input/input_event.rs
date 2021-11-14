use winit::{
    dpi::PhysicalPosition, 
    event::{ElementState, MouseButton, MouseScrollDelta, VirtualKeyCode}
};

pub enum InputEvent {
    KeyboardKey { key: VirtualKeyCode, state: ElementState },
    MouseKey {  key: MouseButton, state: ElementState },
    MouseWheel { delta: MouseScrollDelta },
    MouseMotion { position: PhysicalPosition<f64> }
}


