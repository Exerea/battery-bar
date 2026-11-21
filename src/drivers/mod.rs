pub mod bluetooth;
pub mod hid_generic;
pub mod logitech;
pub mod phonelink;
pub mod system_battery;
pub mod xinput;

use crate::model::DeviceReading;

/// Poll all 6 read-only local battery sources and return a deduplicated list
/// (connected devices first, then alphabetical by name).
pub fn poll_all() -> Vec<DeviceReading> {
    let mut results = Vec::new();
    results.extend(logitech::poll());
    results.extend(bluetooth::poll());
    results.extend(hid_generic::poll());
    results.extend(xinput::poll());
    results.extend(phonelink::poll());
    results.extend(system_battery::poll());

    // Prefer richer source when the same physical device appears across multiple drivers.
    let mut deduped: Vec<DeviceReading> = Vec::new();
    for item in results {
        let norm = item.name.to_ascii_lowercase();
        if let Some(existing) = deduped.iter_mut().find(|e| e.name.to_ascii_lowercase() == norm) {
            if source_priority(&item.source) > source_priority(&existing.source) {
                *existing = item;
            }
            continue;
        }
        deduped.push(item);
    }

    deduped.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then_with(|| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()))
    });
    deduped
}

fn source_priority(src: &str) -> u8 {
    match src {
        "logitech-hidpp" => 4,
        "phonelink" => 3,
        "hid-standard" => 2,
        "bluetooth" => 1,
        _ => 0,
    }
}
