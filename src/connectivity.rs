//! Connection state shared between the network task and the UI.
//!
//! The network task is the only writer; the UI watches [`WIFI_STATE`] and re-renders when it
//! changes. `Latest` is interrupt- and task-safe and wakes the UI on every write, so the UI never
//! polls and the network code never touches a widget.

use alloc::sync::Arc;
use alloc::vec::Vec;

use heapless::String;
use twine::prelude::Latest;

use crate::settings::SSID_MAX;

/// Where the Wi-Fi connection is, in the order the boot screen shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WifiState {
    /// No network saved: nano does not connect until one is set in Settings.
    NotConfigured,
    /// The radio is being initialised.
    Starting,
    /// Associating with the access point (`attempt` counts from 1).
    Connecting { attempt: u8 },
    /// Associated; waiting for a DHCP lease.
    ObtainingIp,
    /// Online with this IPv4 address.
    Connected { ip: [u8; 4] },
}

impl WifiState {
    /// Boot progress (0–100) reached once this state is entered.
    pub const fn progress(self) -> i32 {
        match self {
            Self::NotConfigured => 0,
            Self::Starting => 10,
            Self::Connecting { .. } => 35,
            Self::ObtainingIp => 70,
            Self::Connected { .. } => 100,
        }
    }

    /// Progress the boot screen may creep towards while waiting in this state, so the airplane
    /// keeps moving without ever claiming the next stage early.
    pub const fn progress_ceiling(self) -> i32 {
        match self {
            Self::NotConfigured => 0,
            Self::Starting => 30,
            Self::Connecting { .. } => 65,
            Self::ObtainingIp => 95,
            Self::Connected { .. } => 100,
        }
    }

    /// Whether the device is online.
    pub const fn is_connected(self) -> bool {
        matches!(self, Self::Connected { .. })
    }
}

/// The current Wi-Fi state: written by the network task, watched by the UI.
pub static WIFI_STATE: Latest<WifiState> = Latest::new(WifiState::Starting);

/// The signal strength of the joined network, dBm; `None` while not connected.
pub static WIFI_RSSI: Latest<Option<i8>> = Latest::new(None);

/// A network found by a scan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Network {
    /// Its name.
    pub ssid: String<SSID_MAX>,
    /// Signal strength, dBm.
    pub rssi: i8,
    /// It needs a password.
    pub secured: bool,
}

/// Incremented by the UI to ask for a scan; the network task scans once per change.
pub static SCAN_REQUEST: Latest<u32> = Latest::new(0);
/// The networks of the last scan, strongest first; `None` until a scan completed.
pub static NETWORKS: Latest<Option<Arc<Vec<Network>>>> = Latest::new(None);

/// Signal bars (0–4) for an RSSI.
pub const fn bars(rssi: i8) -> u8 {
    match rssi {
        i8::MIN..=-85 => 1,
        -84..=-73 => 2,
        -72..=-62 => 3,
        _ => 4,
    }
}
