use esp_idf_hal::gpio::{AnyOutputPin, Output, Pin, PinDriver};

const PWM_TABLE: [u16; 256] = [
    65535, 65508, 65479, 65451, 65422, 65394, 65365, 65337, 65308, 65280, 65251, 65223, 65195,
    65166, 65138, 65109, 65081, 65052, 65024, 64995, 64967, 64938, 64909, 64878, 64847, 64815,
    64781, 64747, 64711, 64675, 64637, 64599, 64559, 64518, 64476, 64433, 64389, 64344, 64297,
    64249, 64200, 64150, 64099, 64046, 63992, 63937, 63880, 63822, 63763, 63702, 63640, 63577,
    63512, 63446, 63379, 63310, 63239, 63167, 63094, 63019, 62943, 62865, 62785, 62704, 62621,
    62537, 62451, 62364, 62275, 62184, 62092, 61998, 61902, 61804, 61705, 61604, 61501, 61397,
    61290, 61182, 61072, 60961, 60847, 60732, 60614, 60495, 60374, 60251, 60126, 59999, 59870,
    59739, 59606, 59471, 59334, 59195, 59053, 58910, 58765, 58618, 58468, 58316, 58163, 58007,
    57848, 57688, 57525, 57361, 57194, 57024, 56853, 56679, 56503, 56324, 56143, 55960, 55774,
    55586, 55396, 55203, 55008, 54810, 54610, 54408, 54203, 53995, 53785, 53572, 53357, 53140,
    52919, 52696, 52471, 52243, 52012, 51778, 51542, 51304, 51062, 50818, 50571, 50321, 50069,
    49813, 49555, 49295, 49031, 48764, 48495, 48223, 47948, 47670, 47389, 47105, 46818, 46529,
    46236, 45940, 45641, 45340, 45035, 44727, 44416, 44102, 43785, 43465, 43142, 42815, 42486,
    42153, 41817, 41478, 41135, 40790, 40441, 40089, 39733, 39375, 39013, 38647, 38279, 37907,
    37531, 37153, 36770, 36385, 35996, 35603, 35207, 34808, 34405, 33999, 33589, 33175, 32758,
    32338, 31913, 31486, 31054, 30619, 30181, 29738, 29292, 28843, 28389, 27932, 27471, 27007,
    26539, 26066, 25590, 25111, 24627, 24140, 23649, 23153, 22654, 22152, 21645, 21134, 20619,
    20101, 19578, 19051, 18521, 17986, 17447, 16905, 16358, 15807, 15252, 14693, 14129, 13562,
    12990, 12415, 11835, 11251, 10662, 10070, 9473, 8872, 8266, 7657, 7043, 6424, 5802, 5175, 4543,
    3908, 3267, 2623, 1974, 1320, 662, 0,
];

// https://ledshield.wordpress.com/2012/11/13/led-brightness-to-your-eye-gamma-correction-no/
pub fn lightness_correct(value: u8) -> u8 {
    let corrected_16bit = PWM_TABLE[value as usize];

    // The table gives us the "corrected" 16-bit value, but we need to map this properly
    // Table[0] = 65535 (brightest), Table[255] = 0 (darkest)
    // We want: input 0 = output 0, input 255 = output 255
    // So we need to invert the table output
    let inverted_16bit = 65535 - corrected_16bit;

    // Convert to 8-bit
    (inverted_16bit >> 8) as u8
}

/// This struct takes ownership of the necessary output pins
/// but writes directly to them in batches, so they are not used
pub struct Pins<'d> {
    pub oe_pin: u8,
    pub lat_pin: u8,
    pub clk_pin: u8,
    pub rgb_mask: u32,
    pub addr_mask: u32,
    _r1: PinDriver<'d, AnyOutputPin, Output>,
    _g1: PinDriver<'d, AnyOutputPin, Output>,
    _b1: PinDriver<'d, AnyOutputPin, Output>,
    _r2: PinDriver<'d, AnyOutputPin, Output>,
    _g2: PinDriver<'d, AnyOutputPin, Output>,
    _b2: PinDriver<'d, AnyOutputPin, Output>,
    _a: PinDriver<'d, AnyOutputPin, Output>,
    _b: PinDriver<'d, AnyOutputPin, Output>,
    _c: PinDriver<'d, AnyOutputPin, Output>,
    _d: PinDriver<'d, AnyOutputPin, Output>,
    _e: PinDriver<'d, AnyOutputPin, Output>,
    _clk: PinDriver<'d, AnyOutputPin, Output>,
    _lat: PinDriver<'d, AnyOutputPin, Output>,
    _oe: PinDriver<'d, AnyOutputPin, Output>,
}

impl<'d> Pins<'d> {
    /// The pins must be 0..=31 to be part of the control register 0.
    /// * A, B, C, D must be contiguous
    /// * R1, G1, B1 must be n, n+2, n+3 (2, 4, 5)
    /// * R2, G2, B2 must be n, n+1, n+3 (18, 19, 21)
    pub fn new(
        r1: AnyOutputPin,
        g1: AnyOutputPin,
        b1: AnyOutputPin,
        r2: AnyOutputPin,
        g2: AnyOutputPin,
        b2: AnyOutputPin,
        a: AnyOutputPin,
        b: AnyOutputPin,
        c: AnyOutputPin,
        d: AnyOutputPin,
        e: AnyOutputPin,
        clk: AnyOutputPin,
        lat: AnyOutputPin,
        oe: AnyOutputPin,
    ) -> Pins<'d> {
        for p in [
            r1.pin(),
            g1.pin(),
            b1.pin(),
            r2.pin(),
            g2.pin(),
            b2.pin(),
            a.pin(),
            b.pin(),
            c.pin(),
            d.pin(),
            clk.pin(),
            lat.pin(),
            oe.pin(),
        ] {
            assert!(p < 32);
        }

        let rgb1_mask: u32 = (1 << r1.pin()) | (1 << g1.pin()) | (1 << b1.pin());
        let rgb2_mask: u32 = (1 << r2.pin()) | (1 << g2.pin()) | (1 << b2.pin());
        let rgb_mask = rgb1_mask | rgb2_mask;

        let addr_mask: u32 =
            (1 << a.pin()) | (1 << b.pin()) | (1 << c.pin()) | (1 << d.pin()) | (1 << e.pin());

        let _r1 = PinDriver::output(r1).unwrap();
        let _g1 = PinDriver::output(g1).unwrap();
        let _b1 = PinDriver::output(b1).unwrap();
        let _r2 = PinDriver::output(r2).unwrap();
        let _g2 = PinDriver::output(g2).unwrap();
        let _b2 = PinDriver::output(b2).unwrap();
        let _a = PinDriver::output(a).unwrap();
        let _b = PinDriver::output(b).unwrap();
        let _c = PinDriver::output(c).unwrap();
        let _d = PinDriver::output(d).unwrap();
        let _e = PinDriver::output(e).unwrap();
        let _clk = PinDriver::output(clk).unwrap();
        let _lat = PinDriver::output(lat).unwrap();
        let _oe = PinDriver::output(oe).unwrap();
        Pins {
            oe_pin: _oe.pin() as u8,
            lat_pin: _lat.pin() as u8,
            clk_pin: _clk.pin() as u8,
            rgb_mask,
            addr_mask,
            _r1,
            _g1,
            _b1,
            _r2,
            _g2,
            _b2,
            _a,
            _b,
            _c,
            _d,
            _e,
            _clk,
            _lat,
            _oe,
        }
    }
}
pub struct Hub75<'d> {
    pub pins: Pins<'d>,
}

impl<'d> Hub75<'d> {
    pub fn get_all_pin_mask(&self) -> u32 {
        self.pins.rgb_mask
            | self.pins.addr_mask
            | (1 << self.pins.oe_pin)
            | (1 << self.pins.lat_pin)
            | (1 << self.pins.clk_pin)
    }

    pub fn render_unoptimized(&mut self, image: &image::RgbImage) -> Vec<u32> {
        assert!(image.dimensions() == (64, 64));

        let mut gpio_states = Vec::new();

        const BIT_DEPTH: usize = 5;

        let oe_pin = self.pins.oe_pin;
        let clk_pin = self.pins.clk_pin;
        let lat_pin = self.pins.lat_pin;
        let r1_pin = self.pins._r1.pin() as u8;
        let g1_pin = self.pins._g1.pin() as u8;
        let b1_pin = self.pins._b1.pin() as u8;
        let r2_pin = self.pins._r2.pin() as u8;
        let g2_pin = self.pins._g2.pin() as u8;
        let b2_pin = self.pins._b2.pin() as u8;
        let a_pin = self.pins._a.pin() as u8;
        let b_pin = self.pins._b.pin() as u8;
        let c_pin = self.pins._c.pin() as u8;
        let d_pin = self.pins._d.pin() as u8;
        let e_pin = self.pins._e.pin() as u8;

        // Start with a clean state - all pins LOW
        let mut current_gpio_state: u32 = 0;

        // Set initial control pin states
        current_gpio_state |= 1 << clk_pin; // CLK HIGH
        current_gpio_state |= 1 << lat_pin; // LAT HIGH
        current_gpio_state |= 1 << oe_pin; // OE HIGH (output disabled)

        gpio_states.push(current_gpio_state);

        // BCM rendering - each bit plane gets displayed for 2^bit_plane frames
        for bit_plane in (0..BIT_DEPTH).rev() {
            let frames_to_display = 1 << bit_plane; // 2^bit_plane frames

            for _ in 0..frames_to_display {
                // Scan through all 32 rows (each row drives 2 physical rows)
                for row in 0..32 {
                    // Clear all RGB data pins before loading new data
                    current_gpio_state &= !(1 << r1_pin);
                    current_gpio_state &= !(1 << g1_pin);
                    current_gpio_state &= !(1 << b1_pin);
                    current_gpio_state &= !(1 << r2_pin);
                    current_gpio_state &= !(1 << g2_pin);
                    current_gpio_state &= !(1 << b2_pin);

                    // Clock in pixel data for this row (64 columns)
                    for col in 0..64 {
                        // Get pixels for upper and lower half
                        // Upper half: row 0-31 maps to display rows 0-31
                        // Lower half: row 0-31 maps to display rows 32-63
                        let pixel_upper = image.get_pixel(col as u32, row as u32);
                        let pixel_lower = image.get_pixel(col as u32, (row + 32) as u32);

                        let bit_offset = 8 - BIT_DEPTH + bit_plane;

                        let r1_bit = (lightness_correct(pixel_upper[2]) >> bit_offset) & 1;
                        let g1_bit = (lightness_correct(pixel_upper[0]) >> bit_offset) & 1;
                        let b1_bit = (lightness_correct(pixel_upper[1]) >> bit_offset) & 1;

                        let r2_bit = (lightness_correct(pixel_lower[2]) >> bit_offset) & 1;
                        let g2_bit = (lightness_correct(pixel_lower[0]) >> bit_offset) & 1;
                        let b2_bit = (lightness_correct(pixel_lower[1]) >> bit_offset) & 1;

                        if r1_bit != 0 {
                            current_gpio_state |= 1 << r1_pin;
                        } else {
                            current_gpio_state &= !(1 << r1_pin);
                        }
                        if g1_bit != 0 {
                            current_gpio_state |= 1 << g1_pin;
                        } else {
                            current_gpio_state &= !(1 << g1_pin);
                        }
                        if b1_bit != 0 {
                            current_gpio_state |= 1 << b1_pin;
                        } else {
                            current_gpio_state &= !(1 << b1_pin);
                        }
                        if r2_bit != 0 {
                            current_gpio_state |= 1 << r2_pin;
                        } else {
                            current_gpio_state &= !(1 << r2_pin);
                        }
                        if g2_bit != 0 {
                            current_gpio_state |= 1 << g2_pin;
                        } else {
                            current_gpio_state &= !(1 << g2_pin);
                        }
                        if b2_bit != 0 {
                            current_gpio_state |= 1 << b2_pin;
                        } else {
                            current_gpio_state &= !(1 << b2_pin);
                        }
                        gpio_states.push(current_gpio_state);

                        // Clock low then high
                        current_gpio_state &= !(1 << clk_pin);
                        gpio_states.push(current_gpio_state);
                        current_gpio_state |= 1 << clk_pin;
                        gpio_states.push(current_gpio_state);
                    }

                    // Disable output briefly during latch to prevent glitches
                    current_gpio_state |= 1 << oe_pin;
                    gpio_states.push(current_gpio_state);

                    // Latch the data from shift registers to output latches
                    current_gpio_state &= !(1 << lat_pin); // LAT LOW
                    gpio_states.push(current_gpio_state);
                    current_gpio_state |= 1 << lat_pin; // LAT HIGH
                    gpio_states.push(current_gpio_state);

                    // Set row address (A, B, C, D, E pins)
                    // A, B, C, D select which of the 32 rows (0-31)
                    if (row & 1) != 0 {
                        current_gpio_state |= 1 << a_pin;
                    } else {
                        current_gpio_state &= !(1 << a_pin);
                    }
                    if (row & 2) != 0 {
                        current_gpio_state |= 1 << b_pin;
                    } else {
                        current_gpio_state &= !(1 << b_pin);
                    }
                    if (row & 4) != 0 {
                        current_gpio_state |= 1 << c_pin;
                    } else {
                        current_gpio_state &= !(1 << c_pin);
                    }
                    if (row & 8) != 0 {
                        current_gpio_state |= 1 << d_pin;
                    } else {
                        current_gpio_state &= !(1 << d_pin);
                    }
                    if (row & 16) != 0 {
                        current_gpio_state |= 1 << e_pin;
                    } else {
                        current_gpio_state &= !(1 << e_pin);
                    }
                    gpio_states.push(current_gpio_state);

                    // Enable output to display this row - and keep it enabled
                    current_gpio_state &= !(1 << oe_pin); // OE LOW (enable)
                    gpio_states.push(current_gpio_state);

                    // Row stays enabled until the next row needs to be loaded
                    // This maximizes the display time for each row
                }
            }
        }
        // Disable the output - equivalent to fast_pin_up(oe_pin) - MATCH render_capture
        current_gpio_state |= 1 << oe_pin;
        gpio_states.push(current_gpio_state);

        gpio_states
    }
}
