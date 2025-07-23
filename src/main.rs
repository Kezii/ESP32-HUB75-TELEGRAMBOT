use anyhow::Result;

use bot_api::Esp32Api;
use embedded_svc::http::client::Client;
use esp_idf_hal::task::thread::ThreadSpawnConfiguration;
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::peripherals::Peripherals,
    http::{client::EspHttpConnection, Method},
};
use esp_idf_sys::{esp_restart, GPIO_OUT_W1TC_REG, GPIO_OUT_W1TS_REG};

use frankenstein::{
    ForwardMessageParams, GetUpdatesParams, SendChatActionParams, SendMessageParams, TelegramApi,
};
use image::GenericImageView;
use log::{error, info};
use std::sync::RwLock;

use crate::{config::get_config, hub75::Hub75};
use crate::{hub75::lightness_correct, wifi::my_wifi};

mod bot_api;
mod config;
mod hub75;
mod wifi;

struct BotState {
    owner_id: i64,
    bot_token: &'static str,
}

fn download_file_into_buffer(url: &str, out_buffer: &mut Vec<u8>) -> Result<usize> {
    info!("Downloading file from {}", url);

    let mut client = Client::wrap(EspHttpConnection::new(&Default::default())?);

    let headers = [("accept", "image/*")];

    let request = client.request(Method::Get, url, &headers)?;

    let mut response = request.submit()?;

    let status = response.status();
    info!("<- {status}");

    if status != 200 {
        return Err(anyhow::anyhow!("Status code: {}", status));
    }

    let mut buffer = [0u8; 1024];

    while let Ok(bytes_read) = response.read(&mut buffer) {
        if bytes_read == 0 {
            break;
        }
        out_buffer.extend_from_slice(&buffer[0..bytes_read]);
    }

    Ok(out_buffer.len())
}

fn main() -> Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let sysloop = EspSystemEventLoop::take()?;

    let peripherals = Peripherals::take().unwrap();

    let _pins = hub75::Pins::new(
        peripherals.pins.gpio12.into(), //r1
        peripherals.pins.gpio13.into(), //g1
        peripherals.pins.gpio14.into(), //b1
        peripherals.pins.gpio15.into(), //r2
        peripherals.pins.gpio16.into(), //g2
        peripherals.pins.gpio17.into(), //b2
        peripherals.pins.gpio4.into(),  //a
        peripherals.pins.gpio5.into(),  //b
        peripherals.pins.gpio6.into(),  //c
        peripherals.pins.gpio7.into(),  //d
        peripherals.pins.gpio8.into(),  // E pin for 64x64
        peripherals.pins.gpio3.into(),  //clk
        peripherals.pins.gpio9.into(),  //lat
        peripherals.pins.gpio10.into(), //oe
    );

    let mut h = Hub75 { pins: _pins };

    let image = image::load(
        std::io::Cursor::new(include_bytes!("color_wheel.webp")),
        image::ImageFormat::WebP,
    )
    .unwrap();

    let states = h.render_unoptimized(&image.to_rgb8());
    info!("states: {:?}", states.len());
    let states = std::sync::Arc::new(RwLock::new(states));

    ThreadSpawnConfiguration {
        name: Some(b"fb writer\0"),
        pin_to_core: Some(esp_idf_svc::hal::cpu::Core::Core1),
        ..Default::default()
    }
    .set()
    .unwrap();

    let hub75_mask = h.get_all_pin_mask();

    let states_clone = states.clone();
    std::thread::spawn(move || loop {
        for &state in states_clone.read().unwrap().iter() {
            unsafe {
                core::ptr::write_volatile(esp_idf_sys::GPIO_OUT_REG as *mut _, state);
                // & hub75_mask);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    });
    ThreadSpawnConfiguration::default().set().unwrap();

    let wifi = match my_wifi("maolol", "canegatto", peripherals.modem, sysloop) {
        Ok(inner) => inner,
        Err(err) => {
            error!("Could not connect to Wi-Fi network: {:?}", err);

            unsafe { esp_restart() };
        }
    };

    let config = get_config();
    let bot_state = BotState {
        owner_id: config.bot_owner_id,
        bot_token: config.bot_token,
    };

    let api = Esp32Api::new(bot_state.bot_token);

    let send_owner_info = |bot_state: &BotState| {
        let mut rssi = 0;
        unsafe {
            esp_idf_sys::esp_wifi_sta_get_rssi(&mut rssi);
        }
        api.send_message(
            &SendMessageParams::builder()
                .chat_id(bot_state.owner_id)
                .text(format!(
                    "IP: {}\nRSSI: {}",
                    wifi.sta_netif().get_ip_info().unwrap().ip,
                    rssi,
                ))
                .build(),
        )
        .ok();
    };

    send_owner_info(&bot_state);

    let updates = api
        .get_updates(&GetUpdatesParams::builder().limit(1u32).offset(-1).build())
        .unwrap();

    let mut offset = if let Some(update) = updates.result.first() {
        update.update_id as i64 + 1
    } else {
        0
    };

    let mut webp_buffer = Vec::new();

    loop {
        let updates = api
            .get_updates(
                &GetUpdatesParams::builder()
                    .timeout(120u32)
                    .limit(1u32)
                    .offset(offset)
                    .build(),
            )
            .unwrap();

        for update in updates.result {
            offset = update.update_id as i64 + 1;

            if let frankenstein::UpdateContent::Message(message) = update.content {
                info!(
                    "message id {} from chat {}",
                    message.message_id, message.chat.id
                );

                if let Some(sticker) = message.sticker {
                    if sticker.is_animated == false {
                        let file_id = if let Some(thumbnail) = sticker.thumbnail {
                            thumbnail.file_id.clone()
                        } else {
                            sticker.file_id.clone()
                        };

                        let file_path = api
                            .get_file(
                                &frankenstein::GetFileParams::builder()
                                    .file_id(file_id)
                                    .build(),
                            )
                            .unwrap();

                        if let Some(file_path) = file_path.result.file_path {
                            let url = format!(
                                "https://api.telegram.org/file/bot{}/{}",
                                bot_state.bot_token, file_path
                            );

                            api.send_chat_action(
                                &SendChatActionParams::builder()
                                    .chat_id(message.chat.id)
                                    .action(frankenstein::ChatAction::UploadPhoto)
                                    .build(),
                            )
                            .ok();

                            // download the file with esp32 http library
                            let bytes_read =
                                download_file_into_buffer(&url, &mut webp_buffer).unwrap();

                            info!("Downloaded {} bytes", bytes_read);

                            info!("Loading image");
                            let image = image::load(
                                std::io::Cursor::new(&webp_buffer),
                                image::ImageFormat::WebP,
                            )
                            .unwrap();

                            info!("Resizing image");

                            let scaled =
                                image.resize_exact(64, 64, image::imageops::FilterType::Lanczos3);

                            let states_ = h.render_unoptimized(&scaled.to_rgb8());

                            *states.write().unwrap() = states_;

                            webp_buffer.clear();
                        }
                    } else {
                        api.send_message(
                            &SendMessageParams::builder()
                                .chat_id(message.chat.id)
                                .text("Animated stickers are not supported!")
                                .build(),
                        )
                        .ok();
                    }
                }

                match message.text.unwrap_or_default().as_str() {
                    "/start" | "/help" => {
                        api.send_message(
                            &SendMessageParams::builder()
                                .chat_id(message.chat.id)
                                .text("Hello! Send a sticker to display it on the screen")
                                .build(),
                        )
                        .ok();

                        if message.chat.id == bot_state.owner_id {
                            send_owner_info(&bot_state);
                        }
                    }
                    _ => {}
                }

                if message.chat.type_field == frankenstein::ChatType::Private {
                    api.forward_message(
                        &ForwardMessageParams::builder()
                            .chat_id(bot_state.owner_id)
                            .from_chat_id(message.chat.id)
                            .message_id(message.message_id)
                            .build(),
                    )
                    .ok();
                }
            }
        }
    }
}
