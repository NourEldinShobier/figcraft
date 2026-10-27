//! Which graphics API draws the window.
//!
//! eframe offers wgpu every backend it was built with, and between equal GPUs wgpu takes Vulkan
//! first, on Windows too. A Vulkan driver that faults there takes the app down as the window opens:
//! an access violation inside the vendor's driver, which no guard of ours can catch (seen on a laptop
//! with two AMD GPUs whose drivers were years apart). Direct3D 12 is part of Windows, so on Windows
//! wgpu is offered Direct3D 12, and OpenGL for hardware without a Direct3D 12 driver.
//!
//! `WGPU_BACKEND` (`dx12`, `vulkan`, `gl`, `metal`) still names the backend on every system, and
//! systems other than Windows keep eframe's defaults.

use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup};
use eframe::wgpu::Backends;

/// Direct3D 12, then OpenGL: wgpu ranks a hardware GPU on either ahead of Direct3D's software one.
const WINDOWS: Backends = Backends::DX12.union(Backends::GL);

/// Vulkan, then OpenGL: the second try when Direct3D 12 opened a device that is lost at once (a
/// machine with two GPUs where wgpu picks the one whose Direct3D 12 driver fails; Vulkan and OpenGL
/// open fine there).
const FALLBACK: Backends = Backends::VULKAN.union(Backends::GL);

/// Whether a failed start is worth one more try on [`FALLBACK`]: on Windows, with no `WGPU_BACKEND`
/// naming a backend, when the error says the device was lost.
pub fn retry_on_fallback(error: &str, windows: bool, requested: Option<Backends>) -> bool {
    windows && requested.is_none() && error.to_ascii_lowercase().contains("device is lost")
}

/// The backends wgpu chooses from: the ones `WGPU_BACKEND` asks for (`requested`), else [`WINDOWS`]
/// on Windows, else eframe's `defaults`.
pub fn backends(requested: Option<Backends>, windows: bool, defaults: Backends) -> Backends {
    match requested {
        Some(requested) => requested,
        None if windows => WINDOWS,
        None => defaults,
    }
}

/// Offer wgpu [`backends`] instead of eframe's defaults.
pub fn configure(options: &mut WgpuConfiguration, fallback: bool) {
    if let WgpuSetup::CreateNew(create) = &mut options.wgpu_setup {
        let offered = &mut create.instance_descriptor.backends;
        *offered = if fallback { FALLBACK } else { backends(Backends::from_env(), cfg!(windows), *offered) };
    }
}
