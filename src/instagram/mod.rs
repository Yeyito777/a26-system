//! The Instagram app: `a26-system --app instagram`, a client of the
//! Instagram bridge (docs/social-protocol.md).

use crate::social::starter::{self, Theme};

const THEME: Theme = Theme {
    name: "Instagram",
    background: 0x000000,
    bar: 0x000000,
    text: 0xf5f5f5,
    secondary: 0xa8a8a8,
    accent: 0x0095f6,
    mine: 0x3797f0,
    theirs: 0x262626,
    badge: 0xff3040,
};

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    starter::run("instagram", THEME)
}
