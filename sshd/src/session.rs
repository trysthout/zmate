use russh::{CryptoVec, server::Handle, Sig};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use zellij_utils::{cli::CliArgs, envs, cli::Command, cli::Sessions};

use crate::{handler::HandlerEvent, ZellijClientData, zellij::{start_client, init_server}, ServerHandle, PtyRequest, ServerChannelId};

pub struct Session {
    handle: Option<Handle>,
    zellij_cli_args: CliArgs,
    pty_request: Option<PtyRequest>,
    channel_id: Option<ServerChannelId>,
    rx: UnboundedReceiver<HandlerEvent>,
    server_sender: crossbeam_channel::Sender<Vec<u8>>,
    server_receiver: crossbeam_channel::Receiver<Vec<u8>>,
    server_signal_sender: crossbeam_channel::Sender<Sig>,
    server_signal_receiver: crossbeam_channel::Receiver<Sig>,
}

impl Session {
    pub fn new(args: CliArgs, rx: UnboundedReceiver<HandlerEvent>) -> Self {
        let (server_sender, server_receiver) = crossbeam_channel::unbounded::<Vec<u8>>();
        let (server_signal_sender, server_signal_receiver) = crossbeam_channel::unbounded::<Sig>();

        Self {
            zellij_cli_args: args,
            rx,
            handle: None,
            channel_id: None,
            server_receiver,
            server_sender,
            pty_request: None,
            server_signal_sender,
            server_signal_receiver,
        }
    }

    pub async fn run(&mut self) {
        loop {
            if let Some(event) = self.rx.recv().await {
                self.handle_handler_event(event, self.zellij_cli_args.clone())
                    .await
            }
        }
    }

    async fn handle_handler_event(&mut self, event: HandlerEvent, args: CliArgs) {
        match event {
            HandlerEvent::Authenticated(handle, tx) => {
                self.handle = Some(handle.0);

                if envs::get_session_name().is_err() {
                    init_server(self.zellij_cli_args.clone());
                }

                self.zellij_cli_args.command = Some(Command::Sessions(Sessions::Attach { 
                    session_name: envs::get_session_name().ok(), 
                    create: false, 
                    index: None, options: None, force_run_commands: false }));

                let _ = tx.send(());
            },
            HandlerEvent::PtyRequest(channel_id, pty_request) => {
                self.pty_request = Some(pty_request);
                self.channel_id = Some(channel_id);
            },
            HandlerEvent::ShellRequest(channel_id) => {
                let (sender, mut recv) = unbounded_channel::<ZellijClientData>();
                let pty_request = self.pty_request.as_ref().unwrap();
                let win_size = libc::winsize {
                    ws_row: pty_request.row_height as u16,
                    ws_col: pty_request.col_width as u16,
                    ws_xpixel: pty_request.pix_width as u16,
                    ws_ypixel: pty_request.pix_height as u16,
                };
                let handle = self.handle.clone().unwrap();
                let server_receiver = self.server_receiver.clone();
                let server_signal_receiver = self.server_signal_receiver.clone();
                std::thread::spawn(move || {
                    start_client(
                        args,
                        sender,
                        server_receiver,
                        server_signal_receiver,
                        ServerHandle(handle),
                        channel_id.0,
                        win_size,
                    );
                });

                let handle = self.handle.clone().unwrap();
                let channel_id = self.channel_id.unwrap().0;
                tokio::spawn(async move {
                    loop {
                        if let Some(event) = recv.recv().await {
                            match event {
                                ZellijClientData::Data(data) => {
                                    let _ = handle.data(channel_id, CryptoVec::from(data)).await;
                                },
                                ZellijClientData::Exit => {
                                    let _ = handle.close(channel_id).await;
                                },
                            }
                        }
                    }
                });
            },
            HandlerEvent::Data(_channel_id, data) => {
                let _ = self.server_sender.send(data);
            },
            HandlerEvent::WindowChangeRequest(_, _winsize) => {},
            HandlerEvent::Signal(_, signal) => {
                let _ = self.server_signal_sender.send(signal);
            },
        }
    }
}

