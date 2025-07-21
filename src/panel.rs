use esp_idf_hal::gpio::{AnyIOPin, OutputPin, PinDriver, Output};
use esp_idf_sys::{self, esp_err_t, ESP_OK};
use std::sync::{Arc, Mutex};
use std::mem::MaybeUninit;
use anyhow::Result;
use log::{info, error, warn, debug};
use std::ptr;
use std::thread;
use std::time::Duration;

// Constants for HUB75 protocol
const MATRIX_WIDTH: usize = 64;
const MATRIX_HEIGHT: usize = 64; 
const MATRIX_ROWS_IN_PARALLEL: usize = 2; // HUB75 displays 2 rows at once
const SCAN_ROWS: usize = MATRIX_HEIGHT / MATRIX_ROWS_IN_PARALLEL; // 32 scan rows for 64 high panel
const COLOR_DEPTH_BITS: u8 = 8; // 8-bit color per channel

// Pin configuration structure
#[derive(Debug, Clone)]
pub struct PinConfig {
    pub r1: i32,
    pub g1: i32,
    pub b1: i32,
    pub r2: i32,
    pub g2: i32,
    pub b2: i32,
    pub a: i32,
    pub b: i32,
    pub c: i32,
    pub d: i32,
    pub e: i32,
    pub lat: i32,
    pub oe: i32,
    pub clk: i32,
}

impl Default for PinConfig {
    fn default() -> Self {
        Self {
            r1: 38,
            g1: 14,
            b1: 39,
            r2: 21,
            g2: 12,
            b2: 47,
            a: 48,
            b: 10,
            c: 45,
            d: 9,
            e: 11,
            lat: 46,
            oe: 40,
            clk: 0,
        }
    }
}

// RGB color structure
#[derive(Copy, Clone, Debug)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const BLACK: Color = Color { r: 0, g: 0, b: 0 };
    pub const RED: Color = Color { r: 255, g: 0, b: 0 };
    pub const GREEN: Color = Color { r: 0, g: 255, b: 0 };
    pub const BLUE: Color = Color { r: 0, g: 0, b: 255 };
    pub const WHITE: Color = Color { r: 255, g: 255, b: 255 };
}

// Frame buffer for RGB data
pub struct FrameBuffer {
    data: Vec<Color>,
    width: usize,
    height: usize,
}

impl FrameBuffer {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            data: vec![Color::BLACK; width * height],
            width,
            height,
        }
    }

    pub fn fill(&mut self, color: Color) {
        self.data.fill(color);
    }

    pub fn set_pixel(&mut self, x: usize, y: usize, color: Color) {
        if x < self.width && y < self.height {
            self.data[y * self.width + x] = color;
        }
    }

    pub fn get_pixel(&self, x: usize, y: usize) -> Color {
        if x < self.width && y < self.height {
            self.data[y * self.width + x]
        } else {
            Color::BLACK
        }
    }
}

// HUB75 LED Matrix Panel Driver
pub struct MatrixPanel {
    pins: PinConfig,
    framebuffer: Arc<Mutex<FrameBuffer>>,
    refresh_running: Arc<Mutex<bool>>,
    initialized: bool,
}

impl MatrixPanel {
    pub fn new() -> Self {
        Self {
            pins: PinConfig::default(),
            framebuffer: Arc::new(Mutex::new(FrameBuffer::new(MATRIX_WIDTH, MATRIX_HEIGHT))),
            refresh_running: Arc::new(Mutex::new(false)),
            initialized: false,
        }
    }

    pub fn begin(&mut self) -> Result<bool> {
        if self.initialized {
            return Ok(true);
        }

        info!("Initializing HUB75 Matrix Panel {}x{}", MATRIX_WIDTH, MATRIX_HEIGHT);
        info!("Scan rows: {}, Color depth: {} bits", SCAN_ROWS, COLOR_DEPTH_BITS);
        
        // Setup GPIO pins
        self.setup_gpio_pins()?;

        // Initialize to known state
        self.init_pins_state();

        self.initialized = true;

        // Start the refresh thread
        self.start_refresh_thread();

        info!("HUB75 Matrix Panel initialized successfully");
        Ok(true)
    }

    fn setup_gpio_pins(&mut self) -> Result<()> {
        info!("Setting up GPIO pins");
        
        let pins = [
            self.pins.r1, self.pins.g1, self.pins.b1,
            self.pins.r2, self.pins.g2, self.pins.b2,
            self.pins.a, self.pins.b, self.pins.c,
            self.pins.d, self.pins.e, self.pins.lat,
            self.pins.oe, self.pins.clk,
        ];

        for &pin in &pins {
            if pin >= 0 {
                unsafe {
                    esp_idf_sys::gpio_set_direction(pin as i32, esp_idf_sys::gpio_mode_t_GPIO_MODE_OUTPUT);
                    esp_idf_sys::gpio_set_drive_capability(pin as i32, esp_idf_sys::gpio_drive_cap_t_GPIO_DRIVE_CAP_3);
                }
                debug!("Configured GPIO {} for output", pin);
            }
        }
        
        Ok(())
    }

    fn init_pins_state(&self) {
        unsafe {
            // Initialize all pins to safe state
            esp_idf_sys::gpio_set_level(self.pins.oe as i32, 1);  // Disable output
            esp_idf_sys::gpio_set_level(self.pins.lat as i32, 0); // Latch low
            esp_idf_sys::gpio_set_level(self.pins.clk as i32, 0); // Clock low
            
            // RGB pins low
            esp_idf_sys::gpio_set_level(self.pins.r1 as i32, 0);
            esp_idf_sys::gpio_set_level(self.pins.g1 as i32, 0);
            esp_idf_sys::gpio_set_level(self.pins.b1 as i32, 0);
            esp_idf_sys::gpio_set_level(self.pins.r2 as i32, 0);
            esp_idf_sys::gpio_set_level(self.pins.g2 as i32, 0);
            esp_idf_sys::gpio_set_level(self.pins.b2 as i32, 0);
            
            // Address pins low
            esp_idf_sys::gpio_set_level(self.pins.a as i32, 0);
            esp_idf_sys::gpio_set_level(self.pins.b as i32, 0);
            esp_idf_sys::gpio_set_level(self.pins.c as i32, 0);
            esp_idf_sys::gpio_set_level(self.pins.d as i32, 0);
            esp_idf_sys::gpio_set_level(self.pins.e as i32, 0);
        }
    }

    fn start_refresh_thread(&mut self) {
        let refresh_running = self.refresh_running.clone();
        let framebuffer = self.framebuffer.clone();
        let pins = self.pins.clone();
        
        *refresh_running.lock().unwrap() = true;
        
        info!("Starting HUB75 refresh thread");
        
        thread::spawn(move || {
            Self::refresh_loop(refresh_running, framebuffer, pins);
        });
    }

    fn refresh_loop(
        refresh_running: Arc<Mutex<bool>>,
        framebuffer: Arc<Mutex<FrameBuffer>>,
        pins: PinConfig,
    ) {
        info!("HUB75 refresh loop started");
        
        let mut frame_count = 0u32;
        
        while *refresh_running.lock().unwrap() {
            // Binary Code Modulation - refresh for each bit plane
            for bit_plane in 0..COLOR_DEPTH_BITS {
                if !*refresh_running.lock().unwrap() {
                    break;
                }
                
                // Scan through all rows for this bit plane
                for row in 0..SCAN_ROWS {
                    if !*refresh_running.lock().unwrap() {
                        break;
                    }
                    
                    // Disable output while updating
                    unsafe {
                        esp_idf_sys::gpio_set_level(pins.oe as i32, 1);
                    }
                    
                    // Set row address (A, B, C, D, E pins)
                    Self::set_row_address(&pins, row);
                    
                    // Shift out pixel data for this row and bit plane
                    if let Ok(fb) = framebuffer.lock() {
                        Self::shift_row_data(&pins, &fb, row, bit_plane);
                    }
                    
                    // Latch the data
                    Self::latch_data(&pins);
                    
                    // Enable output for this row
                    unsafe {
                        esp_idf_sys::gpio_set_level(pins.oe as i32, 0);
                    }
                    
                    // Display time based on bit plane (Binary Code Modulation)
                    // Bit 0 gets 1 unit, bit 1 gets 2 units, bit 2 gets 4 units, etc.
                    let display_time_us = (1 << bit_plane) * 2; // Base time unit in microseconds
                    unsafe {
                        esp_idf_sys::esp_rom_delay_us(display_time_us as u32);
                    }
                }
            }
            
            frame_count += 1;
            if frame_count % 100 == 0 {
                debug!("Completed {} frames", frame_count);
            }
        }
        
        // Disable output when stopping
        unsafe {
            esp_idf_sys::gpio_set_level(pins.oe as i32, 1);
        }
        
        info!("HUB75 refresh loop stopped");
    }

    fn set_row_address(pins: &PinConfig, row: usize) {
        unsafe {
            // Set address lines A, B, C, D, E based on row number
            esp_idf_sys::gpio_set_level(pins.a as i32, if (row & 0x01) != 0 { 1 } else { 0 });
            esp_idf_sys::gpio_set_level(pins.b as i32, if (row & 0x02) != 0 { 1 } else { 0 });
            esp_idf_sys::gpio_set_level(pins.c as i32, if (row & 0x04) != 0 { 1 } else { 0 });
            esp_idf_sys::gpio_set_level(pins.d as i32, if (row & 0x08) != 0 { 1 } else { 0 });
            esp_idf_sys::gpio_set_level(pins.e as i32, if (row & 0x10) != 0 { 1 } else { 0 });
        }
    }

    fn shift_row_data(pins: &PinConfig, framebuffer: &FrameBuffer, row: usize, bit_plane: u8) {
        // For each column in the matrix
        for col in 0..MATRIX_WIDTH {
            // Get pixels for upper and lower halves
            let pixel_upper = framebuffer.get_pixel(col, row);
            let pixel_lower = framebuffer.get_pixel(col, row + SCAN_ROWS);
            
            // Extract the bit for this bit plane
            let bit_mask = 1 << bit_plane;
            
            let r1 = (pixel_upper.r & bit_mask) != 0;
            let g1 = (pixel_upper.g & bit_mask) != 0;
            let b1 = (pixel_upper.b & bit_mask) != 0;
            
            let r2 = (pixel_lower.r & bit_mask) != 0;
            let g2 = (pixel_lower.g & bit_mask) != 0;
            let b2 = (pixel_lower.b & bit_mask) != 0;
            
            unsafe {
                // Set RGB data lines
                esp_idf_sys::gpio_set_level(pins.r1 as i32, if r1 { 1 } else { 0 });
                esp_idf_sys::gpio_set_level(pins.g1 as i32, if g1 { 1 } else { 0 });
                esp_idf_sys::gpio_set_level(pins.b1 as i32, if b1 { 1 } else { 0 });
                esp_idf_sys::gpio_set_level(pins.r2 as i32, if r2 { 1 } else { 0 });
                esp_idf_sys::gpio_set_level(pins.g2 as i32, if g2 { 1 } else { 0 });
                esp_idf_sys::gpio_set_level(pins.b2 as i32, if b2 { 1 } else { 0 });
                
                // Clock pulse to shift in the data
                esp_idf_sys::gpio_set_level(pins.clk as i32, 1);
                esp_idf_sys::esp_rom_delay_us(1);
                esp_idf_sys::gpio_set_level(pins.clk as i32, 0);
                esp_idf_sys::esp_rom_delay_us(1);
            }
        }
    }

    fn latch_data(pins: &PinConfig) {
        unsafe {
            // Pulse latch pin to transfer shift register data to output latches
            esp_idf_sys::gpio_set_level(pins.lat as i32, 1);
            esp_idf_sys::esp_rom_delay_us(1);
            esp_idf_sys::gpio_set_level(pins.lat as i32, 0);
        }
    }

    pub fn fill_screen(&self, color: Color) {
        if let Ok(mut fb) = self.framebuffer.lock() {
            fb.fill(color);
        }
    }

    pub fn set_pixel(&self, x: usize, y: usize, color: Color) {
        if let Ok(mut fb) = self.framebuffer.lock() {
            fb.set_pixel(x, y, color);
        }
    }

    pub fn stop(&mut self) {
        info!("Stopping HUB75 display");
        *self.refresh_running.lock().unwrap() = false;
    }
}

impl Drop for MatrixPanel {
    fn drop(&mut self) {
        if self.initialized {
            self.stop();
        }
    }
}

// Global panel instance
static mut PANEL_INSTANCE: Option<MatrixPanel> = None;

// Initialize the HUB75 panel
pub fn initialize_panel() -> Result<()> {
    info!("Initializing HUB75 LED Matrix Panel");
    
    let mut panel = MatrixPanel::new();
    
    if panel.begin()? {
        // Store the panel instance globally
        unsafe {
            PANEL_INSTANCE = Some(panel);
        }
        
        info!("HUB75 panel initialized successfully");
        Ok(())
    } else {
        error!("Failed to initialize HUB75 panel");
        Err(anyhow::anyhow!("Failed to initialize HUB75 panel"))
    }
}

// Get access to the global panel instance
pub fn get_panel() -> Option<&'static mut MatrixPanel> {
    unsafe {
        PANEL_INSTANCE.as_mut()
    }
}

// Display functions for easy use
pub fn display_blank_screen() -> Result<()> {
    if let Some(panel) = get_panel() {
        panel.fill_screen(Color::BLACK);
        Ok(())
    } else {
        Err(anyhow::anyhow!("Panel not initialized"))
    }
}

pub fn display_color_screen(r: u8, g: u8, b: u8) -> Result<()> {
    if let Some(panel) = get_panel() {
        panel.fill_screen(Color { r, g, b });
        info!("Filled screen with RGB({}, {}, {})", r, g, b);
        Ok(())
    } else {
        Err(anyhow::anyhow!("Panel not initialized"))
    }
}

// Test pattern functions
pub fn display_test_pattern() -> Result<()> {
    if let Some(panel) = get_panel() {
        // Create red/green/blue vertical stripes
        for x in 0..MATRIX_WIDTH {
            for y in 0..MATRIX_HEIGHT {
                let color = if x < MATRIX_WIDTH / 3 {
                    Color::RED
                } else if x < 2 * MATRIX_WIDTH / 3 {
                    Color::GREEN
                } else {
                    Color::BLUE
                };
                panel.set_pixel(x, y, color);
            }
        }
        info!("Displayed test pattern");
        Ok(())
    } else {
        Err(anyhow::anyhow!("Panel not initialized"))
    }
}

// Simple GPIO test function (kept for debugging)
pub fn gpio_test() -> Result<()> {
    info!("Testing basic GPIO functionality...");
    
    let config = PinConfig::default();
    
    // Configure OE pin for output
    unsafe {
        esp_idf_sys::gpio_set_direction(config.oe as i32, esp_idf_sys::gpio_mode_t_GPIO_MODE_OUTPUT);
        esp_idf_sys::gpio_set_drive_capability(config.oe as i32, esp_idf_sys::gpio_drive_cap_t_GPIO_DRIVE_CAP_3);
    }
    
    info!("Blinking OE pin (GPIO {}) 3 times...", config.oe);
    
    // Blink OE pin 3 times
    for i in 0..3 {
        unsafe {
            esp_idf_sys::gpio_set_level(config.oe as i32, 1); // High
        }
        thread::sleep(Duration::from_millis(500));
        
        unsafe {
            esp_idf_sys::gpio_set_level(config.oe as i32, 0); // Low  
        }
        thread::sleep(Duration::from_millis(500));
        
        info!("Blink {} completed", i + 1);
    }
    
    info!("GPIO test completed");
    Ok(())
}

