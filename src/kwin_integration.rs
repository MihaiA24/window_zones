mod client;
mod companion;
mod kglobalaccel;
mod protocol;
mod server;

pub use client::{KwinHotkeySystem, KwinWindowSystem};
pub use protocol::{
    KWIN_INTERFACE, KWIN_OBJECT_PATH, KWIN_PROTOCOL_MAJOR, KWIN_SERVICE_NAME, KwinIntegrationError,
};
pub use server::run_kwin_companion_service;

#[cfg(test)]
mod test_bus;

#[cfg(test)]
#[path = "kwin_integration/tests/native_script.rs"]
mod native_script_tests;
