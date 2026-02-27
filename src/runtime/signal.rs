use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

pub fn install_ctrlc_handler() -> Result<()> {
    INTERRUPTED.store(false, Ordering::SeqCst);
    ctrlc::set_handler(|| {
        INTERRUPTED.store(true, Ordering::SeqCst);
    })
    .context("failed to install Ctrl+C handler")
}

pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

pub fn reset_interrupted() {
    INTERRUPTED.store(false, Ordering::SeqCst);
}
