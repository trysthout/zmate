use async_trait::async_trait;
use russh::{
    server::{Msg, Session},
    *,
};
use russh_keys::*;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot::*;

use crate::{PtyRequest, ServerChannelId, ServerHandle};

#[derive(Debug)]
pub enum HandlerEvent {
    Authenticated(ServerHandle, Sender<()>),
    PtyRequest(ServerChannelId, PtyRequest),
    ShellRequest(ServerChannelId),
    Data(ServerChannelId, Vec<u8>),
    Signal(ServerChannelId, Sig),
    WindowChangeRequest(ServerChannelId, libc::winsize),
}

#[derive(thiserror::Error, Debug)]
pub enum HandlerError {
    #[error("maybe channel disconnected")]
    ChannelSend,
}

#[derive(Debug)]
pub struct Handler {
    pub tx: UnboundedSender<HandlerEvent>,
}

impl Handler {
    pub fn new(tx: UnboundedSender<HandlerEvent>) -> Self {
        Handler { tx }
    }

    fn send_event(&self, event: HandlerEvent) -> Result<(), HandlerError> {
        self.tx.send(event).map_err(|_| HandlerError::ChannelSend)
    }
}

#[async_trait]
impl server::Handler for Handler {
    type Error = anyhow::Error;

    async fn channel_open_session(
        self,
        _channel: Channel<Msg>,
        session: Session,
    ) -> Result<(Self, bool, Session), Self::Error> {
        Ok((self, true, session))
    }

    async fn auth_succeeded(self, session: Session) -> Result<(Self, Session), Self::Error> {
        let handle = session.handle();
        let (tx, rx) = channel::<()>();
        self.send_event(HandlerEvent::Authenticated(ServerHandle(handle), tx))?;
        let _ = rx.await;
        Ok((self, session))
    }

    async fn auth_none(self, _user: &str) -> Result<(Self, server::Auth), Self::Error> {
        Ok((self, server::Auth::Accept))
    }

    async fn auth_publickey(
        self,
        _: &str,
        _: &key::PublicKey,
    ) -> Result<(Self, server::Auth), Self::Error> {
        Ok((self, server::Auth::Accept))
    }

    async fn data(
        mut self,
        channel: ChannelId,
        data: &[u8],
        session: Session,
    ) -> Result<(Self, Session), Self::Error> {
        let mut data = data.to_vec();
        if data[0] == 4 {
            data = vec![17]
        }

        self.send_event(HandlerEvent::Data(ServerChannelId(channel), data))?;
        Ok((self, session))
    }

    async fn pty_request(
        self,
        channel: ChannelId,
        term: &str,
        col_width: u32,
        row_height: u32,
        pix_width: u32,
        pix_height: u32,
        modes: &[(Pty, u32)],
        mut session: Session,
    ) -> Result<(Self, Session), Self::Error> {
        let term = term.to_string();
        let modes = modes.to_vec();
        self.send_event(HandlerEvent::PtyRequest(
            ServerChannelId(channel),
            PtyRequest {
                term,
                col_width,
                row_height,
                pix_width,
                pix_height,
                modes,
            },
        ))?;

        session.channel_success(channel);
        Ok((self, session))
    }

    async fn shell_request(
        self,
        channel: ChannelId,
        mut session: Session,
    ) -> Result<(Self, Session), Self::Error> {
        self.send_event(HandlerEvent::ShellRequest(ServerChannelId(channel)))?;

        session.channel_success(channel);

        Ok((self, session))
    }

    async fn signal(
        self,
        channel: ChannelId,
        signal: Sig,
        session: Session,
    ) -> Result<(Self, Session), Self::Error> {
        self.send_event(HandlerEvent::Signal(ServerChannelId(channel), signal))?;
        Ok((self, session))
    }

    async fn window_change_request(
        self,
        channel: ChannelId,
        col_width: u32,
        row_height: u32,
        pix_width: u32,
        pix_height: u32,
        session: Session,
    ) -> Result<(Self, Session), Self::Error> {
        self.send_event(HandlerEvent::WindowChangeRequest(
            ServerChannelId(channel),
            libc::winsize {
                ws_row: row_height as u16,
                ws_col: col_width as u16,
                ws_xpixel: pix_width as u16,
                ws_ypixel: pix_height as u16,
            },
        ))?;
        Ok((self, session))
    }
}

#[cfg(test)]
mod test {
    use ansi_term::ANSIByteStrings;

    #[test]
    fn term() {
        ANSIByteStrings(&[
            vec![
                27, 91, 49, 59, 52, 56, 59, 53, 59, 49, 54, 59, 51, 56, 59, 53, 59, 50, 53, 53,
                109, 32, 67, 116, 114, 108, 32, 43, 27, 91, 48, 109, 27, 91, 52, 56, 59, 53, 59,
                49, 54, 59, 51, 56, 59, 53, 59, 49, 54, 109, 238, 130, 176, 27, 91, 48, 109, 27,
                91, 52, 56, 59, 53, 59, 49, 53, 52, 59, 51, 56, 59, 53, 59, 49, 54, 109, 238, 130,
                176, 27, 91, 49, 109, 32, 60, 27, 91, 51, 56, 59, 53, 59, 49, 50, 52, 109, 103, 27,
                91, 51, 56, 59, 53, 59, 49, 54, 109, 62, 32, 76, 79, 67, 75, 32, 27, 91, 52, 56,
                59, 53, 59, 49, 54, 59, 51, 56, 59, 53, 59, 49, 53, 52, 109, 238, 130, 176, 27, 91,
                48, 109, 27, 91, 52, 56, 59, 53, 59, 50, 52, 53, 59, 51, 56, 59, 53, 59, 49, 54,
                109, 238, 130, 176, 27, 91, 50, 59, 51, 109, 32, 60, 62, 32, 80, 65, 78, 69, 32,
                27, 91, 48, 109, 27, 91, 52, 56, 59, 53, 59, 49, 54, 59, 51, 56, 59, 53, 59, 50,
                52, 53, 109, 238, 130, 176, 27, 91, 48, 109, 27, 91, 52, 56, 59, 53, 59, 50, 52,
                53, 59, 51, 56, 59, 53, 59, 49, 54, 109, 238, 130, 176, 27, 91, 50, 59, 51, 109,
                32, 60, 62, 32, 84, 65, 66, 32, 27, 91, 48, 109, 27, 91, 52, 56, 59, 53, 59, 49,
                54, 59, 51, 56, 59, 53, 59, 50, 52, 53, 109, 238, 130, 176, 27, 91, 48, 109, 27,
                91, 52, 56, 59, 53, 59, 50, 52, 53, 59, 51, 56, 59, 53, 59, 49, 54, 109, 238, 130,
                176, 27, 91, 50, 59, 51, 109, 32, 60, 62, 32, 82, 69, 83, 73, 90, 69, 32, 27, 91,
                48, 109, 27, 91, 52, 56, 59, 53, 59, 49, 54, 59, 51, 56, 59, 53, 59, 50, 52, 53,
                109, 238, 130, 176, 27, 91, 48, 109, 27, 91, 52, 56, 59, 53, 59, 50, 52, 53, 59,
                51, 56, 59, 53, 59, 49, 54, 109, 238, 130, 176, 27, 91, 50, 59, 51, 109, 32, 60,
                62, 32, 77, 79, 86, 69, 32, 27, 91, 48, 109, 27, 91, 52, 56, 59, 53, 59, 49, 54,
                59, 51, 56, 59, 53, 59, 50, 52, 53, 109, 238, 130, 176, 27, 91, 48, 109, 27, 91,
                52, 56, 59, 53, 59, 50, 52, 53, 59, 51, 56, 59, 53, 59, 49, 54, 109, 238, 130, 176,
                27, 91, 50, 59, 51, 109, 32, 60, 62, 32, 83, 69, 65, 82, 67, 72, 32, 27, 91, 48,
                109, 27, 91, 52, 56, 59, 53, 59, 49, 54, 59, 51, 56, 59, 53, 59, 50, 52, 53, 109,
                238, 130, 176, 27, 91, 48, 109, 27, 91, 52, 56, 59, 53, 59, 50, 52, 53, 59, 51, 56,
                59, 53, 59, 49, 54, 109, 238, 130, 176, 27, 91, 50, 59, 51, 109, 32, 60, 62, 32,
                83, 69, 83, 83, 73, 79, 78, 32, 27, 91, 48, 109, 27, 91, 52, 56, 59, 53, 59, 49,
                54, 59, 51, 56, 59, 53, 59, 50, 52, 53, 109, 238, 130, 176, 27, 91, 48, 109, 27,
                91, 52, 56, 59, 53, 59, 50, 52, 53, 59, 51, 56, 59, 53, 59, 49, 54, 109, 238, 130,
                176, 27, 91, 50, 59, 51, 109, 32, 60, 62, 32, 81, 85, 73, 84, 32, 27, 91, 48, 109,
                27, 91, 52, 56, 59, 53, 59, 49, 54, 59, 51, 56, 59, 53, 59, 50, 52, 53, 109, 238,
                130, 176, 27, 91, 48, 109, 27, 91, 52, 56, 59, 53, 59, 49, 54, 109, 27, 91, 48, 75,
                10, 13, 27, 91, 109, 27, 91, 49, 59, 51, 56, 59, 53, 59, 50, 53, 53, 109, 32, 45,
                45, 32, 73, 78, 84, 69, 82, 70, 65, 67, 69, 32, 76, 79, 67, 75, 69, 68, 32, 45, 45,
                32, 27, 91, 48, 109, 27, 91, 48, 75,
            ]
            .into(),
            vec![
                27, 91, 52, 56, 59, 53, 59, 48, 109, 27, 91, 48, 75, 10, 13, 27, 91, 109, 32, 84,
                105, 112, 58, 32, 27, 91, 49, 109, 85, 78, 66, 79, 85, 78, 68, 27, 91, 48, 109, 32,
                61, 62, 32, 111, 112, 101, 110, 32, 110, 101, 119, 32, 112, 97, 110, 101, 46, 32,
                27, 91, 49, 109, 85, 78, 66, 79, 85, 78, 68, 27, 91, 48, 109, 32, 61, 62, 32, 110,
                97, 118, 105, 103, 97, 116, 101, 32, 98, 101, 116, 119, 101, 101, 110, 32, 112, 97,
                110, 101, 115, 46, 32, 27, 91, 49, 109, 85, 78, 66, 79, 85, 78, 68, 27, 91, 48,
                109, 32, 61, 62, 32, 105, 110, 99, 114, 101, 97, 115, 101, 47, 100, 101, 99, 114,
                101, 97, 115, 101, 32, 112, 97, 110, 101, 32, 115, 105, 122, 101, 46, 27, 91, 48,
                75,
            ]
            .into(),
            vec![
                27, 91, 52, 56, 59, 53, 59, 48, 109, 27, 91, 48, 75, 10, 13, 27, 91, 109, 27, 91,
                48, 75,
            ]
            .into(),
            vec![
                27, 91, 49, 59, 52, 56, 59, 53, 59, 48, 59, 51, 56, 59, 53, 59, 48, 109, 32, 90,
                101, 108, 108, 105, 106, 32, 27, 91, 48, 109, 27, 91, 52, 56, 59, 53, 59, 48, 59,
                51, 56, 59, 53, 59, 48, 109, 27, 91, 49, 109, 32, 84, 97, 98, 32, 35, 49, 32, 27,
                91, 48, 109, 27, 91, 52, 56, 59, 53, 59, 48, 59, 51, 56, 59, 53, 59, 48, 109, 27,
                91, 48, 109, 27, 91, 52, 56, 59, 53, 59, 48, 109, 27, 91, 48, 75,
            ]
            .into(),
            vec![
                27, 91, 49, 59, 52, 56, 59, 53, 59, 49, 54, 59, 51, 56, 59, 53, 59, 50, 53, 53,
                109, 32, 90, 101, 108, 108, 105, 106, 32, 27, 91, 48, 109, 27, 91, 49, 59, 52, 56,
                59, 53, 59, 49, 54, 59, 51, 56, 59, 53, 59, 50, 53, 53, 109, 40, 97, 97, 41, 32,
                27, 91, 48, 109, 27, 91, 52, 56, 59, 53, 59, 49, 53, 52, 59, 51, 56, 59, 53, 59,
                49, 54, 109, 238, 130, 176, 27, 91, 49, 109, 32, 84, 97, 98, 32, 35, 49, 32, 27,
                91, 48, 109, 27, 91, 52, 56, 59, 53, 59, 49, 54, 59, 51, 56, 59, 53, 59, 49, 53,
                52, 109, 238, 130, 176, 27, 91, 48, 109, 27, 91, 52, 56, 59, 53, 59, 49, 54, 109,
                27, 91, 48, 75,
            ]
            .into(),
        ])
        .write_to(&mut std::io::stdout())
        .unwrap();
    }
}
