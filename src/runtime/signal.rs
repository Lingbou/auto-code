use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

pub fn install_ctrlc_handler() -> Result<()> {
    ctrlc::set_handler(|| {
        INTERRUPTED.store(true, Ordering::SeqCst);
        eprintln!("\n[autocode] received Ctrl+C, stopping...");
    })
    .context("failed to install Ctrl+C handler")
}

pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}
