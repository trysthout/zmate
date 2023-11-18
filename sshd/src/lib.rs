use std::fmt::{Formatter, Display, Debug};

use russh::{ChannelId, Pty, server::Handle};
use tokio::sync::mpsc::UnboundedSender;

mod handler;
mod session;
mod zellij;
mod ssh_input_output;
mod session_util;
pub mod server;


pub enum ZellijClientData {
    Data(String),
    Exit,
}

#[derive(Clone)]
pub struct ServerHandle(Handle);

impl Debug for ServerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HandleWrapper")
    }
}

#[derive(Clone)]
pub struct ServerOutput {
    sender: UnboundedSender<ZellijClientData>,
}

impl std::io::Write for ServerOutput {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = self.sender.send(ZellijClientData::Data(
            String::from_utf8_lossy(buf).to_string(),
        ));

        Ok(buf.len())
    }
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        self.write(buf).map(|_| ())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
#[derive(Clone, Debug)]
pub struct PtyRequest {
    pub term: String,
    pub col_width: u32,
    pub row_height: u32,
    pub pix_width: u32,
    pub pix_height: u32,
    pub modes: Vec<(Pty, u32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Hash, Eq)]
pub struct ServerChannelId(pub ChannelId);

impl Display for ServerChannelId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}