//! Colours, all in one place. The palette follows the Tinta Office azulejo tile: ivory ground,
//! the Tinta Correo teal as accent.

use egui::Color32;

#[derive(Debug, Clone, Copy)]
pub struct Tokens {
    pub accent: Color32,
    pub accent_soft: Color32,
    pub rail: Color32,
    pub rail_icon: Color32,
    pub pane: Color32,
    pub list: Color32,
    pub reading: Color32,
    pub text: Color32,
    pub text_weak: Color32,
    pub unread: Color32,
    pub flag: Color32,
    pub border: Color32,
    pub today: Color32,
}

impl Tokens {
    pub fn light() -> Tokens {
        Tokens {
            accent: Color32::from_rgb(0x1F, 0x7A, 0x8C),
            accent_soft: Color32::from_rgb(0xD5, 0xEA, 0xEE),
            rail: Color32::from_rgb(0x1E, 0x25, 0x47),
            rail_icon: Color32::from_rgb(0xF3, 0xED, 0xE1),
            pane: Color32::from_rgb(0xF3, 0xED, 0xE1),
            list: Color32::from_rgb(0xFB, 0xF8, 0xF2),
            reading: Color32::WHITE,
            text: Color32::from_rgb(0x1E, 0x25, 0x47),
            text_weak: Color32::from_rgb(0x6B, 0x6F, 0x80),
            unread: Color32::from_rgb(0x1F, 0x7A, 0x8C),
            flag: Color32::from_rgb(0xB0, 0x3A, 0x2E),
            border: Color32::from_rgb(0xD8, 0xCB, 0xB2),
            today: Color32::from_rgb(0x1F, 0x7A, 0x8C),
        }
    }

    pub fn dark() -> Tokens {
        Tokens {
            accent: Color32::from_rgb(0x4F, 0xB3, 0xC4),
            accent_soft: Color32::from_rgb(0x1D, 0x3C, 0x44),
            rail: Color32::from_rgb(0x12, 0x16, 0x2B),
            rail_icon: Color32::from_rgb(0xE6, 0xE0, 0xD4),
            pane: Color32::from_rgb(0x1E, 0x21, 0x2B),
            list: Color32::from_rgb(0x24, 0x27, 0x33),
            reading: Color32::from_rgb(0x2A, 0x2D, 0x3A),
            text: Color32::from_rgb(0xEC, 0xE8, 0xDF),
            text_weak: Color32::from_rgb(0xA0, 0xA3, 0xB0),
            unread: Color32::from_rgb(0x4F, 0xB3, 0xC4),
            flag: Color32::from_rgb(0xE0, 0x6A, 0x5C),
            border: Color32::from_rgb(0x3A, 0x3E, 0x4C),
            today: Color32::from_rgb(0x4F, 0xB3, 0xC4),
        }
    }
}
