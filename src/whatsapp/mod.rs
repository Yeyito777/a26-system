//! The WhatsApp app: `a26-system --app whatsapp`, a client of the WhatsApp
//! bridge (docs/social-protocol.md).

use crate::social::starter::{self, Theme};

const THEME: Theme = Theme {
    name: "WhatsApp",
    background: 0x0b141a,
    bar: 0x0b141a,
    text: 0xe9edef,
    secondary: 0x8696a0,
    accent: 0x00a884,
    mine: 0x005c4b,
    theirs: 0x202c33,
    badge: 0x00a884,
};

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    starter::run("whatsapp", THEME)
}
