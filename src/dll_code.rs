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

use crate::hooks;

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
                ButtplugClientEvent::DeviceRemoved(info) => {
                    log::info!("Device {} Removed!", info.name());
                }
                ButtplugClientEvent::ScanningFinished => {
                    log::info!("Device scanning is finished!");
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
        let mut channel = CHANNEL_TX.lock().await;
        *channel = Some(tx);
    }

    unsafe {
        hooks::setup_hooks();
    }

    while let Ok(a) = rx.recv() {
        match a {
            Event::Hit => {
                log::trace!("hit!");
                let mut vibes = Vec::new();
                for dev in DEVICES.lock().await.iter() {
                    vibes.push(vibrate_device(dev.clone()));
                }

                for vibe in vibes {
                    vibe.await
                }
            }
        }
    }
}

async fn vibrate_device(dev: Arc<ButtplugClientDevice>) {
    let config = CONFIG.get().expect("config should exist");

    if dev.message_attributes().scalar_cmd().is_some() {
        if let Err(e) = dev
            .vibrate(&VibrateCommand::Speed(config.vibration_strength))
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
