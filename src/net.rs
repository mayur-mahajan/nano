//! Wi-Fi station and IP stack.
//!
//! [`connection`] owns the radio. It follows [`SETTINGS`]: with no network saved it stays
//! idle (scanning on request, so the settings screen can list networks); with one it joins,
//! waits for a DHCP lease and reconnects whenever the link drops, and when the saved network
//! changes it leaves and joins the new one. Each step is published to
//! [`WIFI_STATE`](crate::connectivity::WIFI_STATE); the signal strength to
//! [`WIFI_RSSI`](crate::connectivity::WIFI_RSSI). [`net_runner`] drives the IP stack.

use alloc::sync::Arc;
use alloc::vec::Vec;

use embassy_futures::select::{Either, Either3, select, select3};
use embassy_net::{Runner, Stack, StackResources};
use embassy_time::{Duration, Timer};
use esp_radio::wifi::scan::ScanConfig;
use esp_radio::wifi::sta::StationConfig;
use esp_radio::wifi::{Config, Interface, WifiController};
use heapless::String;
use static_cell::StaticCell;

use crate::connectivity::{NETWORKS, Network, SCAN_REQUEST, WIFI_RSSI, WIFI_STATE, WifiState};
use crate::settings::{PASSWORD_MAX, SETTINGS, SSID_MAX};

/// Pause before retrying a failed connection.
const RETRY_DELAY: Duration = Duration::from_secs(3);
/// How often the settings and scan requests are checked.
const CHECK_PERIOD: Duration = Duration::from_millis(500);
/// How often the signal strength is read while connected.
const RSSI_PERIOD: Duration = Duration::from_secs(5);
/// Sockets the stack can hold (DHCP, DNS, feeder HTTP, SNTP, spare).
const SOCKETS: usize = 6;

/// The network to join.
#[derive(Clone, PartialEq, Eq)]
struct Credentials {
    ssid: String<SSID_MAX>,
    password: String<PASSWORD_MAX>,
}

impl Credentials {
    fn current() -> Option<Self> {
        let s = SETTINGS.get();
        s.has_wifi().then_some(Self {
            ssid: s.ssid,
            password: s.password,
        })
    }
}

/// Creates the IP stack (DHCPv4) on the station interface.
pub fn stack(
    device: Interface<'static>,
    seed: u64,
) -> (Stack<'static>, Runner<'static, Interface<'static>>) {
    static RESOURCES: StaticCell<StackResources<SOCKETS>> = StaticCell::new();
    embassy_net::new(
        device,
        embassy_net::Config::dhcpv4(Default::default()),
        RESOURCES.init(StackResources::new()),
        seed,
    )
}

/// Runs the IP stack.
#[embassy_executor::task]
pub async fn net_runner(mut runner: Runner<'static, Interface<'static>>) -> ! {
    runner.run().await
}

/// Joins the saved network and keeps the connection up.
#[embassy_executor::task]
pub async fn connection(mut controller: WifiController<'static>, stack: Stack<'static>) -> ! {
    // Station mode without a network: scans work before anything is saved.
    while let Err(e) = controller.set_config(&Config::Station(StationConfig::default())) {
        log::error!("wifi: station mode rejected: {e:?}; retrying");
        Timer::after(RETRY_DELAY).await;
    }
    let mut scans_seen = SCAN_REQUEST.version();
    loop {
        let Some(creds) = Credentials::current() else {
            WIFI_STATE.set(WifiState::NotConfigured);
            WIFI_RSSI.set(None);
            scan_if_asked(&mut controller, &mut scans_seen).await;
            Timer::after(CHECK_PERIOD).await;
            continue;
        };
        join(&mut controller, stack, &creds, &mut scans_seen).await;
    }
}

/// Joins `creds` and stays connected until the saved network changes.
async fn join(
    controller: &mut WifiController<'static>,
    stack: Stack<'static>,
    creds: &Credentials,
    scans_seen: &mut u32,
) {
    WIFI_STATE.set(WifiState::Starting);
    let config = Config::Station(
        StationConfig::default()
            .with_ssid(creds.ssid.as_str())
            .with_password(creds.password.as_str().into()),
    );
    if let Err(e) = controller.set_config(&config) {
        log::error!("wifi: configuration rejected: {e:?}");
        wait_for_change(creds).await;
        return;
    }
    log::info!("wifi: joining \"{}\"", creds.ssid);

    let mut attempt: u8 = 0;
    loop {
        if Credentials::current().as_ref() != Some(creds) {
            return;
        }
        attempt = attempt.saturating_add(1);
        WIFI_STATE.set(WifiState::Connecting { attempt });
        if let Err(e) = controller.connect_async().await {
            log::warn!("wifi: connection attempt {attempt} failed: {e:?}");
            scan_if_asked(controller, scans_seen).await;
            Timer::after(RETRY_DELAY).await;
            continue;
        }
        log::info!("wifi: associated, waiting for DHCP");
        WIFI_STATE.set(WifiState::ObtainingIp);

        match select(
            stack.wait_config_up(),
            controller.wait_for_disconnect_async(),
        )
        .await
        {
            Either::First(()) => {
                let ip = stack
                    .config_v4()
                    .map(|c| c.address.address().octets())
                    .unwrap_or_default();
                log::info!("wifi: online at {}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
                WIFI_STATE.set(WifiState::Connected { ip });
                attempt = 0;
                if stay_connected(controller, creds, scans_seen).await {
                    log::info!("wifi: network changed, leaving \"{}\"", creds.ssid);
                    let _ = controller.disconnect_async().await;
                    WIFI_RSSI.set(None);
                    return;
                }
                log::warn!("wifi: disconnected, reconnecting");
            }
            Either::Second(info) => log::warn!("wifi: dropped before DHCP finished ({info:?})"),
        }
        WIFI_RSSI.set(None);
        Timer::after(RETRY_DELAY).await;
    }
}

/// While connected: reads the RSSI and serves scans. Returns `true` when the saved network
/// changed, `false` when the link dropped.
async fn stay_connected(
    controller: &mut WifiController<'static>,
    creds: &Credentials,
    scans_seen: &mut u32,
) -> bool {
    let mut ticks = 0;
    loop {
        if ticks == 0 {
            if let Ok(rssi) = controller.rssi() {
                WIFI_RSSI.set(Some(rssi.clamp(-127, 0) as i8));
            }
            ticks = RSSI_PERIOD.as_millis() / CHECK_PERIOD.as_millis();
        }
        ticks -= 1;
        let outcome = select3(
            controller.wait_for_disconnect_async(),
            wait_for_change(creds),
            Timer::after(CHECK_PERIOD),
        )
        .await;
        match outcome {
            Either3::First(_) => return false,
            Either3::Second(()) => return true,
            Either3::Third(()) => scan_if_asked(controller, scans_seen).await,
        }
    }
}

/// Returns once the saved network differs from `creds`.
async fn wait_for_change(creds: &Credentials) {
    while Credentials::current().as_ref() == Some(creds) {
        Timer::after(CHECK_PERIOD).await;
    }
}

/// Scans when the UI asked for it since the last scan, and publishes the networks.
async fn scan_if_asked(controller: &mut WifiController<'static>, seen: &mut u32) {
    if SCAN_REQUEST.version() == *seen {
        return;
    }
    *seen = SCAN_REQUEST.version();
    match controller.scan_async(&ScanConfig::default()).await {
        Ok(found) => {
            let mut networks: Vec<Network> = Vec::new();
            for ap in found {
                let ssid = ap.ssid.as_str();
                if ssid.is_empty() || networks.iter().any(|n| n.ssid == ssid) {
                    continue;
                }
                let mut name = String::new();
                let _ = name.push_str(ssid);
                networks.push(Network {
                    ssid: name,
                    rssi: ap.signal_strength,
                    secured: ap
                        .auth_method
                        .is_some_and(|m| m != esp_radio::wifi::AuthenticationMethod::None),
                });
            }
            networks.sort_by(|a, b| b.rssi.cmp(&a.rssi));
            log::info!("wifi: scan found {} networks", networks.len());
            NETWORKS.set(Some(Arc::new(networks)));
        }
        Err(e) => log::warn!("wifi: scan failed: {e:?}"),
    }
}
