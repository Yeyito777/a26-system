//! The Discord app: `a26-system --app discord`, a client of the Discord
//! bridge (docs/social-protocol.md).

use crate::social::starter::{self, Theme};

const THEME: Theme = Theme {
    name: "Discord",
    background: 0x313338,
    bar: 0x1e1f22,
    text: 0xf2f3f5,
    secondary: 0x949ba4,
    accent: 0x5865f2,
    mine: 0x383a40,
    theirs: 0x2b2d31,
    badge: 0xf23f43,
};

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    starter::run("discord", THEME)
}
