use std::{
    io::{Read, Write},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use buttplug::{
    client::{ButtplugClient, ButtplugClientDevice, ButtplugClientEvent, VibrateCommand},
    core::connector::ButtplugInProcessClientConnectorBuilder,
    server::{
        device::hardware::communication::{
            btleplug::BtlePlugCommunicationManagerBuilder,
            lovense_dongle::LovenseHIDDongleCommunicationManagerBuilder,
            xinput::XInputDeviceCommunicationManagerBuilder,
        },
        ButtplugServerBuilder,
    },
};
use futures::StreamExt;
use log::LevelFilter;
use once_cell::sync::{Lazy, OnceCell};
use serde::Deserialize;
use std::sync::mpsc::Sender;
use tokio::{sync::Mutex, time::sleep};

use crate::{hooks, global::{PLAYER_1_STATE, PLAYER_2_STATE}};

#[derive(Debug, Deserialize)]
pub struct Config {
    vibration_strength: f64,
    vibration_time: u64,
    log_level: LevelFilter,
}

pub enum Event {
    Hit,
}

pub static CONFIG: OnceCell<Config> = OnceCell::new();
pub static CHANNEL_TX: Lazy<Mutex<Option<Sender<Event>>>> = Lazy::new(|| Mutex::new(None));
pub static HIT_CHANNEL_TX: Lazy<Mutex<Option<Sender<Event>>>> = Lazy::new(|| Mutex::new(None));

const DEFAULT_CONFIG: &str = include_str!("default_config.toml");

/// User code for initializing the DLL goes here
pub fn initialize() {
    let config_result = setup_config();

    if let Err(ref e) = config_result {
        unsafe {
            windows::Win32::System::Console::AllocConsole();
        }
        println!("error: {e}")
    }

    let config = CONFIG.get_or_init(|| {
        config_result.unwrap_or(Config {
            vibration_strength: 1.0,
            vibration_time: 300,
            log_level: LevelFilter::Error,
        })
    });

    if let Ok(logfile) = std::fs::File::create("acprmod.log") {
        simplelog::WriteLogger::init(
            config.log_level,
            simplelog::ConfigBuilder::default()
                .set_location_level(simplelog::LevelFilter::Off)
                .build(),
            logfile,
        )
        .unwrap();
    }

    std::panic::set_hook(Box::new(|e| {
        log::error!("panicked!: {e}");
    }));

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run())
}

fn setup_config() -> Result<Config, String> {
    let config_path = PathBuf::from("./acpr_buttplug_config.toml");

    if !config_path.exists() {
        if let Ok(mut f) = std::fs::File::create(&config_path) {
            f.write_all(DEFAULT_CONFIG.as_bytes())
                .map_err(|e| e.to_string())?
        }
    }

    std::fs::File::open(&config_path)
        .map_err(|e| e.to_string())
        .and_then(|mut f| {
            let mut s = String::new();
            f.read_to_string(&mut s).map_err(|e| e.to_string())?;
            toml::from_str::<Config>(&s).map_err(|e| e.to_string())
        })
}

static DEVICES: Lazy<Mutex<Vec<Arc<ButtplugClientDevice>>>> = Lazy::new(|| Mutex::new(Vec::new()));

async fn run() {
    log::info!("setting up buttplug.rs...");

    let builder = ButtplugServerBuilder::default()
        .comm_manager(BtlePlugCommunicationManagerBuilder::default())
        .comm_manager(LovenseHIDDongleCommunicationManagerBuilder::default())
        .comm_manager(XInputDeviceCommunicationManagerBuilder::default())
        .finish();

    if let Err(e) = builder {
        log::error!("error building server: {e}");
        return;
    }

    log::trace!("server built");

    let connector = ButtplugInProcessClientConnectorBuilder::default()
        .server(builder.unwrap())
        .finish();

    let client = ButtplugClient::new("Buttplug Mod");
    if let Err(e) = client.connect(connector).await {
        log::debug!("error connecting: {}", e)
    }

    let mut events = client.event_stream();
    tokio::spawn(async move {
        while let Some(event) = events.next().await {
            match event {
                ButtplugClientEvent::DeviceAdded(device) => {
                    log::info!("Device {} Connected!", device.name());
                    let mut devices = DEVICES.lock().await;
                    devices.push(device);
                }
                ButtplugClientEvent::DeviceRemoved(removed) => {
                    log::info!("Device {} Removed!", removed.name());
                    let mut devices = DEVICES.lock().await;

                    // clear the device from our device list
                    let mut disconnected_devices = Vec::new();
                    for (idx, device) in devices.iter().enumerate() {
                        if !device.connected() {
                            disconnected_devices.push(idx);
                        }
                    }

                    disconnected_devices.into_iter().for_each(|idx| {
                        devices.remove(idx);
                    });
                }
                _ => {}
            }
        }
    });

    if let Err(e) = client.start_scanning().await {
        log::error!("error scanning for devices: {e}")
    }

    let (tx, rx) = std::sync::mpsc::channel::<Event>();
    // set up channels for communication between hook threads and event loop
    {
        let mut channel = HIT_CHANNEL_TX.lock().await;
        *channel = Some(tx);
    }

    unsafe {
        // currently broken due to some compiler optimization?
        // game crashes on hit when built in release mode
        // hooks::setup_hooks();
    }

    let mut hitstop = 0;

    loop {
        tokio::time::sleep(Duration::from_millis(16)).await;
        unsafe {
            hitstop = get_current_hitstop();
        }

        if hitstop == 0 {
            continue
        }

        log::trace!("hitstop = {hitstop}");

        let strength = hitstop as f64 / 60.0;

        log::trace!("vibrating at {strength}");

        let mut vibes = Vec::new();
        for dev in DEVICES.lock().await.iter() {
            vibes.push(vibrate_device(dev.clone(), strength));
        }

        for vibe in vibes {
            vibe.await
        }

        continue;
    }
}

unsafe fn get_current_hitstop() -> u8 {
    let player1_addr = PLAYER_1_STATE.get_address() as *const *const u8;
    let player2_addr = PLAYER_2_STATE.get_address() as *const *const u8;

    if (*player1_addr).is_null() || (*player2_addr).is_null() {
        return 0
    }

    let p1_hitstop = (*player1_addr).offset(0xFD).read_unaligned();
    let p2_hitstop = (*player2_addr).offset(0xFD).read_unaligned();

    p1_hitstop.max(p2_hitstop)
}

async fn vibrate_device(dev: Arc<ButtplugClientDevice>, strength: f64) {
    let config = CONFIG.get().expect("config should exist");

    if dev.message_attributes().scalar_cmd().is_some() {
        if let Err(e) = dev
            .vibrate(&VibrateCommand::Speed((strength * config.vibration_strength).clamp(0.0, 1.0)))
            .await
        {
            log::error!("Error sending vibrate command to device! {}", e);
            return;
        }

        log::trace!("{} should start vibrating!", dev.name());
        sleep(Duration::from_millis(config.vibration_time)).await;

        if let Err(e) = dev.stop().await {
            log::error!("Error stopping device: {e}")
        }
        log::trace!("{} should stop vibrating!", dev.name());
    } else {
        log::trace!("{} doesn't vibrate! This code should be updated to handle rotation and linear movement!", dev.name());
    }
}
