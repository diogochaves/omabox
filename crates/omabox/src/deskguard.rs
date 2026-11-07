//! A desk box's guards (#321 step 5): what up refuses for a desk box beyond step 1's (by whether
//! the desk is Omarchy's, and a Hyprland config omabox can drive), and what holds once it is up
//! (its shell's idle timeouts held off).
//! Step 0's placeholders: nothing refused, nothing held.

use crate::cmd::up::Up;
use crate::exit::{Ctx, Res};

/// checks U JAILED: up's checks for a desk box, once its HOME and base are known (up.rs checks).
pub fn checks(_ctx: &Ctx, _u: &mut Up, _jailed: bool) -> Res<()> {
    Ok(())
}

/// A desk box's guards once it is ready (up_box.rs make, after its monitors).
pub fn after_ready(_ctx: &Ctx, _u: &Up) -> Res<()> {
    Ok(())
}
