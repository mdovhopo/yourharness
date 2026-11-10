//! The Mac's battery, as its menu bar shows it, for the phone's Mac page: IOKit's power sources.

use core_foundation::array::{CFArray, CFArrayRef};
use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;

use crate::wire;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOPSCopyPowerSourcesInfo() -> CFTypeRef;
    fn IOPSCopyPowerSourcesList(blob: CFTypeRef) -> CFArrayRef;
    fn IOPSGetPowerSourceDescription(blob: CFTypeRef, source: CFTypeRef) -> CFDictionaryRef;
}

/// The internal battery now, or `None` on a Mac without one.
pub fn read() -> Option<wire::Power> {
    // SAFETY: the Copy calls hand over ownership (create rule); the description is borrowed
    // from `blob`, which outlives it.
    unsafe {
        let blob = IOPSCopyPowerSourcesInfo();
        if blob.is_null() {
            return None;
        }
        let blob = CFType::wrap_under_create_rule(blob);
        let list = IOPSCopyPowerSourcesList(blob.as_CFTypeRef());
        if list.is_null() {
            return None;
        }
        let list: CFArray<CFType> = CFArray::wrap_under_create_rule(list);
        list.iter().find_map(|source| {
            let description = IOPSGetPowerSourceDescription(blob.as_CFTypeRef(), source.as_CFTypeRef());
            if description.is_null() {
                return None;
            }
            let description: CFDictionary<CFString, CFType> = CFDictionary::wrap_under_get_rule(description);
            let get = |key: &str| description.find(CFString::new(key)).map(|v| v.clone());
            let text = |key: &str| get(key).and_then(|v| v.downcast::<CFString>()).map(|s| s.to_string());
            let number = |key: &str| get(key).and_then(|v| v.downcast::<CFNumber>()).and_then(|n| n.to_i64());
            let flag = |key: &str| get(key).and_then(|v| v.downcast::<CFBoolean>()).is_some_and(bool::from);
            if text("Type").as_deref() != Some("InternalBattery") || !flag("Is Present") {
                return None;
            }
            let (current, max) = (number("Current Capacity")?, number("Max Capacity")?.max(1));
            Some(wire::Power {
                percent: (current * 100 / max).clamp(0, 100) as u8,
                charging: flag("Is Charging"),
                plugged_in: text("Power Source State").as_deref() == Some("AC Power"),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_without_crashing() {
        // A laptop has one; a desktop or a CI runner may not.
        if let Some(p) = super::read() {
            assert!(p.percent <= 100);
        }
    }
}
