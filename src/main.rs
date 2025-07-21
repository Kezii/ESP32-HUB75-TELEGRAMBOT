use anyhow::Result;

use base64::{prelude::BASE64_STANDARD, Engine};
use bot_api::{telegram_post_multipart, Esp32Api};
use embedded_svc::http::client::Client;
use esp_idf_hal::{gpio::PinDriver, io::Write, task::thread::ThreadSpawnConfiguration};
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::peripherals::Peripherals,
    http::{client::EspHttpConnection, server::EspHttpServer, Method},
};
use esp_idf_sys::esp_restart;

use frankenstein::{
    ForwardMessageParams, GetUpdatesParams, SendChatActionParams, SendMessageParams, TelegramApi,
};
use image::{GenericImage, GenericImageView};
use log::{error, info};
use std::sync::{Mutex, RwLock};

use crate::hub75::{Frame, Hub75, Pins};
use crate::wifi::my_wifi;

mod bot_api;
mod hub75;
mod wifi;

// Global frame buffer to avoid stack overflow
static FRAME_BUFFER: RwLock<Frame> = RwLock::new([[[0; 64]; 32]; 3]);

// Gamma correction lookup table for more natural color appearance
// Converts linear 8-bit values to gamma-corrected values
const GAMMA_TABLE: [u8; 256] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2,
    2, 2, 2, 3, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 6, 6, 6, 7, 7, 7, 8, 8, 8, 9, 9, 9, 10, 10, 11,
    11, 11, 12, 12, 13, 13, 13, 14, 14, 15, 15, 16, 16, 17, 17, 18, 18, 19, 19, 20, 21, 21, 22, 22,
    23, 23, 24, 25, 25, 26, 27, 27, 28, 29, 29, 30, 31, 31, 32, 33, 34, 34, 35, 36, 37, 37, 38, 39,
    40, 40, 41, 42, 43, 44, 45, 46, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61,
    62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 76, 77, 78, 79, 80, 81, 83, 84, 85, 86, 88,
    89, 90, 91, 93, 94, 95, 96, 98, 99, 100, 102, 103, 104, 106, 107, 109, 110, 111, 113, 114, 116,
    117, 119, 120, 121, 123, 124, 126, 128, 129, 131, 132, 134, 135, 137, 138, 140, 142, 143, 145,
    146, 148, 150, 151, 153, 155, 157, 158, 160, 162, 163, 165, 167, 169, 170, 172, 174, 176, 178,
    179, 181, 183, 185, 187, 189, 191, 193, 194, 196, 198, 200, 202, 204, 206, 208, 210, 212, 214,
    216, 218, 220, 222, 224, 227, 229, 231, 233, 235, 237, 239, 241, 244, 246, 248, 250, 252, 255,
];

// Apply gamma correction to a color value
fn gamma_correct(value: u8) -> u8 {
    GAMMA_TABLE[value as usize]
}

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

    let mut total_bytes_read = 0;
    let mut buffer = [0u8; 1024];

    while let Ok(bytes_read) = response.read(&mut buffer) {
        if bytes_read == 0 {
            break;
        }
        total_bytes_read += bytes_read;
        out_buffer.extend_from_slice(&buffer[0..bytes_read]);
    }

    Ok(out_buffer.len())
}

// Create a blank frame buffer
fn create_blank_frame() -> Frame {
    [[[0; 64]; 32]; 3]
}

// Set a pixel in the framebuffer with proper BCM bit distribution
fn set_pixel(buffer: &mut Frame, x: usize, y: usize, r: u8, g: u8, b: u8) {
    if x >= 64 || y >= 64 {
        return; // Out of bounds
    }

    let bit_depth = buffer.len(); // Using all 6 bit planes

    // In a 64x64 matrix:
    // - Each row drives two physical rows simultaneously
    // - Row 0 drives physical rows 0 and 32
    // - Row 1 drives physical rows 1 and 33
    // - etc.

    let row = y % 32; // Which row in the buffer (0-31)
    let col = x;
    let is_bottom_half = y >= 32; // Is this the bottom half (rows 32-63)?

    // For each bit plane, extract the corresponding bit from the color values
    for plane in 0..bit_depth {
        // Extract bit from the top 6 bits of each 8-bit color value
        // Plane 0 gets bit 2, plane 1 gets bit 3, ..., plane 5 gets bit 7
        let bit_position = plane + 8 - bit_depth;
        let r_bit = (r >> bit_position) & 1;
        let g_bit = (g >> bit_position) & 1;
        let b_bit = (b >> bit_position) & 1;

        // Top half uses RGB1 pins (bits 7, 6, 4)
        // Bottom half uses RGB2 pins (bits 3, 1, 0)
        if !is_bottom_half {
            if r_bit > 0 {
                buffer[plane][row][col] |= 0b10000000; // R1 bit
            }
            if g_bit > 0 {
                buffer[plane][row][col] |= 0b01000000; // G1 bit
            }
            if b_bit > 0 {
                buffer[plane][row][col] |= 0b00010000; // B1 bit
            }
        } else {
            if r_bit > 0 {
                buffer[plane][row][col] |= 0b00001000; // R2 bit
            }
            if g_bit > 0 {
                buffer[plane][row][col] |= 0b00000010; // G2 bit
            }
            if b_bit > 0 {
                buffer[plane][row][col] |= 0b00000001; // B2 bit
            }
        }
    }
}

// Draw an image to the framebuffer with proper BCM color depth
fn draw_image_to_framebuffer(image: &image::DynamicImage) {
    let mut frame_buffer = FRAME_BUFFER.write().unwrap();
    *frame_buffer = create_blank_frame();

    // Draw the scaled image to the framebuffer with full 8-bit color values and gamma correction
    for y in 0..64 {
        for x in 0..64 {
            let pixel = image.get_pixel(x as u32, y as u32);
            // Apply gamma correction to make colors appear more natural
            let r = gamma_correct(pixel[0]); // Gamma-corrected red value
            let g = gamma_correct(pixel[1]); // Gamma-corrected green value
            let b = gamma_correct(pixel[2]); // Gamma-corrected blue value
            set_pixel(&mut frame_buffer, x, y, b, r, g); // Note: swapping r and b for correct color mapping
        }
    }
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

    let frame_buffer_img = std::sync::Arc::new(RwLock::new(image::RgbImage::new(64, 64)));

    ThreadSpawnConfiguration {
        name: Some(b"fb writer\0"),
        pin_to_core: Some(esp_idf_svc::hal::cpu::Core::Core1),
        ..Default::default()
    }
    .set()
    .unwrap();

    let frame_buffer_img_clone = frame_buffer_img.clone();
    std::thread::spawn(move || loop {
        h.render(&FRAME_BUFFER.read().unwrap());
        //h.render_unoptimized(&frame_buffer_img_clone.read().unwrap());
        std::thread::sleep(std::time::Duration::from_millis(1));
    });
    ThreadSpawnConfiguration::default().set().unwrap();

    {
        let d = include_bytes!("color_wheel.webp");

        let image = image::load(std::io::Cursor::new(d), image::ImageFormat::WebP).unwrap();

        draw_image_to_framebuffer(&image);
    }

    let wifi = match my_wifi("maolol", "canegatto", peripherals.modem, sysloop) {
        Ok(inner) => inner,
        Err(err) => {
            error!("Could not connect to Wi-Fi network: {:?}", err);

            unsafe { esp_restart() };
        }
    };

    let mut bot_state = BotState {
        owner_id: FIXME,
        bot_token: FIXME,
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
                        info!("{:?}", sticker);

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

                        println!("{:?}", file_path.result.file_path);

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

                            draw_image_to_framebuffer(&scaled);

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
