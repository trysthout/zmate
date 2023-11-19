pub mod cli;
pub mod consts;
pub mod data;
pub mod envs;
pub mod errors;
pub mod home;
pub mod input;
pub mod kdl;
pub mod pane_size;
pub mod plugin_api;
pub mod position;
pub mod session_serialization;
pub mod setup;
pub mod shared;
pub mod ssh;

// The following modules can't be used when targeting wasm
#[cfg(not(target_family = "wasm"))]
pub mod channels; // Requires async_std
#[cfg(not(target_family = "wasm"))]
pub mod downloader; // Requires async_std
#[cfg(not(target_family = "wasm"))]
pub mod ipc; // Requires interprocess
#[cfg(not(target_family = "wasm"))]
pub mod logging; // Requires log4rs

#[cfg(not(target_family = "wasm"))]
pub use ::{
    anyhow, async_channel, async_std, clap, common_path, humantime, interprocess, lazy_static,
    libc, miette, nix, notify_debouncer_full, regex, serde, signal_hook, surf, tempfile, termwiz,
    vte,
};

pub use ::prost;
use serde::{Serialize, Deserialize};


#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ServerMode  {
    Ssh,
    Normal
}