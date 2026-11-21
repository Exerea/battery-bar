use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

use crate::model::{DeviceKind, DeviceReading};

/// Poll laptop / UPS system battery via GetSystemPowerStatus (omitted on desktops with no battery).
pub fn poll() -> Vec<DeviceReading> {
    let mut st = SYSTEM_POWER_STATUS::default();
    let ok = unsafe { GetSystemPowerStatus(&mut st).is_ok() };
    if !ok {
        return Vec::new();
    }
    // BatteryFlag 128 = No system battery, 255 = Unknown status
    if (st.BatteryFlag & 128) != 0 || st.BatteryFlag == 255 || st.BatteryLifePercent > 100 {
        return Vec::new();
    }
    let charging = st.ACLineStatus == 1 || (st.BatteryFlag & 8) != 0;
    vec![DeviceReading {
        key: "system:battery".to_string(),
        name: "PC バッテリー".to_string(),
        kind: DeviceKind::Other,
        source: "system-power".to_string(),
        percent: Some(st.BatteryLifePercent),
        charging,
        connected: true,
        stale: false,
    }]
}
