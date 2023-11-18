use crossbeam_channel::Receiver;
use interprocess::local_socket::LocalSocketStream;

use russh::{ChannelId, Sig};

use std::os::unix::io::RawFd;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::{io, time};
use tokio::sync::mpsc::UnboundedSender;
use zellij_client::os_input_output::{ClientOsApi, StdinPoller};
use zellij_utils::{
    anyhow::{Context, Result},
    pane_size::Size,
    data::Palette,
    errors::ErrorContext,
    ipc::{ClientToServerMsg, IpcReceiverWithContext, IpcSenderWithContext, ServerToClientMsg},
    shared::default_palette,
    interprocess, libc, nix,
};

use crate::{ServerHandle, ServerOutput, ZellijClientData};

const ENABLE_MOUSE_SUPPORT: &str = "\u{1b}[?1000h\u{1b}[?1002h\u{1b}[?1015h\u{1b}[?1006h";
const DISABLE_MOUSE_SUPPORT: &str = "\u{1b}[?1006l\u{1b}[?1015l\u{1b}[?1002l\u{1b}[?1000l";

#[derive(Clone)]
pub struct SshInputOutput {
    pub handle: ServerHandle,
    pub win_size: libc::winsize,
    pub channel_id: ChannelId,
    pub send_instructions_to_server: Arc<Mutex<Option<IpcSenderWithContext<ClientToServerMsg>>>>,
    pub receive_instructions_from_server:
        Arc<Mutex<Option<IpcReceiverWithContext<ServerToClientMsg>>>>,
    pub reading_from_stdin: Arc<Mutex<Option<Vec<u8>>>>,
    pub session_name: Arc<Mutex<Option<String>>>,
    pub sender: UnboundedSender<ZellijClientData>,
    pub server_receiver: Receiver<Vec<u8>>,
    pub server_signal_receiver: Receiver<Sig>,
}

impl zellij_client::os_input_output::ClientOsApi for SshInputOutput {
    fn get_terminal_size_using_fd(&self, _: i32) -> Size {
        Size {
            rows: self.win_size.ws_row as usize,
            cols: self.win_size.ws_col as usize,
        }
    }

    fn set_terminal_size(&mut self, win_size: libc::winsize) {
        self.win_size = win_size
    }

    fn set_raw_mode(&mut self, _: RawFd) {
        //into_raw_mode(fd);
    }

    fn unset_raw_mode(&self, _: RawFd) -> Result<(), nix::Error> {
        Ok(())
    }

    fn box_clone(&self) -> Box<dyn ClientOsApi> {
        Box::new((*self).clone())
    }

    fn update_session_name(&mut self, new_session_name: String) {
        *self.session_name.lock().unwrap() = Some(new_session_name);
    }

    fn read_from_stdin(&mut self) -> Result<Vec<u8>, &'static str> {
        let session_name_at_calltime = { self.session_name.lock().unwrap().clone() };
        // here we wait for a lock in case another thread is holding stdin
        // this can happen for example when switching sessions, the old thread will only be
        // released once it sees input over STDIN
        //
        // when this happens, we detect in the other thread that our session is ended (by comparing
        // the session name at the beginning of the call and the one after we read from STDIN), and
        // so place what we read from STDIN inside a buffer (the "reading_from_stdin" on our state)
        // and release the lock
        //
        // then, another thread will see there's something in the buffer immediately as it acquires
        // the lock (without having to wait for STDIN itself) forward this buffer and proceed to
        // wait for the "real" STDIN net time it is called
        let mut buffered_bytes = self.reading_from_stdin.lock().unwrap();

        match buffered_bytes.take() {
            Some(buffered_bytes) => Ok(buffered_bytes),
            None => {
                let read_buf = if let Ok(data) = self.server_receiver.recv() {
                    data
                } else {
                    return Err("sshd channel disconnected");
                };
                //let mut read_buf = Vec::with_capacity(128);
                //loop {
                //    let mut read_bytes = if let Ok(data) = self.server_receiver.recv() {
                //        data
                //    } else {
                //        return Err("sshd channel disconnected");
                //    };

                //    if read_bytes.last().unwrap().eq(&('\n' as u8)) {
                //        read_buf.append(&mut read_bytes);
                //        break;
                //    }

                //    read_buf.append(&mut read_bytes);
                //}

                let session_name_after_reading_from_stdin =
                    { self.session_name.lock().unwrap().clone() };
                if session_name_at_calltime.is_some()
                    && session_name_at_calltime != session_name_after_reading_from_stdin
                {
                    *buffered_bytes = Some(read_buf);
                    Err("Session ended")
                } else {
                    Ok(read_buf)
                }
            },
        }
    }

    fn get_stdout_writer(&self) -> Box<dyn io::Write> {
        Box::new(ServerOutput {
            sender: self.sender.clone(),
        })
    }
    fn get_stdin_reader(&self) -> Box<dyn io::Read> {
        let stdin = ::std::io::stdin();
        Box::new(stdin)
    }

    fn send_to_server(&self, msg: ClientToServerMsg) {
        // TODO: handle the error here, right now we silently ignore it
        let _ = self
            .send_instructions_to_server
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .send(msg);
    }
    fn recv_from_server(&self) -> Option<(ServerToClientMsg, ErrorContext)> {
        self.receive_instructions_from_server
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .recv()
    }
    fn handle_signals(&self, _sigwinch_cb: Box<dyn Fn()>, quit_cb: Box<dyn Fn()>) {
        let _sigwinch_cb_timestamp = time::Instant::now();
        match self.server_signal_receiver.recv() {
            Ok(sig) => match sig {
                Sig::TERM | Sig::INT | Sig::QUIT | Sig::HUP => {
                    quit_cb();
                },
                _ => unreachable!(),
            },

            Err(_) => {},
        }
        //let mut signals = Signals::new(&[SIGWINCH, SIGTERM, SIGINT, SIGQUIT, SIGHUP]).unwrap();
        //for signal in signals.forever() {
        //    match signal {
        //        SIGWINCH => {
        //            // throttle sigwinch_cb calls, reduce excessive renders while resizing
        //            if sigwinch_cb_timestamp.elapsed() < SIGWINCH_CB_THROTTLE_DURATION {
        //                thread::sleep(SIGWINCH_CB_THROTTLE_DURATION);
        //            }
        //            sigwinch_cb_timestamp = time::Instant::now();
        //            sigwinch_cb();
        //        },
        //        SIGTERM | SIGINT | SIGQUIT | SIGHUP => {
        //            quit_cb();
        //            break;
        //        },
        //        _ => unreachable!(),
        //    }
        //}
    }
    fn connect_to_server(&self, path: &Path) {
        let socket;
        loop {
            match LocalSocketStream::connect(path) {
                Ok(sock) => {
                    socket = sock;
                    break;
                },
                Err(_) => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                },
            }
        }
        let sender = IpcSenderWithContext::new(socket);
        let receiver = sender.get_receiver();
        *self.send_instructions_to_server.lock().unwrap() = Some(sender);
        *self.receive_instructions_from_server.lock().unwrap() = Some(receiver);
    }
    fn load_palette(&self) -> Palette {
        // this was removed because termbg doesn't release stdin in certain scenarios (we know of
        // windows terminal and FreeBSD): https://github.com/zellij-org/zellij/issues/538
        //
        // let palette = default_palette();
        // let timeout = std::time::Duration::from_millis(100);
        // if let Ok(rgb) = termbg::rgb(timeout) {
        //     palette.bg = PaletteColor::Rgb((rgb.r as u8, rgb.g as u8, rgb.b as u8));
        //     // TODO: also dynamically get all other colors from the user's terminal
        //     // this should be done in the same method (OSC ]11), but there might be other
        //     // considerations here, hence using the library
        // };
        default_palette()
    }
    fn enable_mouse(&self) -> Result<()> {
        let err_context = "failed to enable mouse mode";
        let mut stdout = self.get_stdout_writer();
        stdout
            .write_all(ENABLE_MOUSE_SUPPORT.as_bytes())
            .context(err_context)?;
        stdout.flush().context(err_context)?;
        Ok(())
    }

    fn disable_mouse(&self) -> Result<()> {
        let err_context = "failed to enable mouse mode";
        let mut stdout = self.get_stdout_writer();
        stdout
            .write_all(DISABLE_MOUSE_SUPPORT.as_bytes())
            .context(err_context)?;
        stdout.flush().context(err_context)?;
        Ok(())
    }

    fn stdin_poller(&self) -> Box<dyn StdinPoller> {
        Box::new(ServerStdinPoller::new(self.server_receiver.clone()))
    }

    fn env_variable(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    fn close(&self) {
        let _ = self.sender.send(ZellijClientData::Exit);
    }
}

pub struct ServerStdinPoller {
    receiver: crossbeam_channel::Receiver<Vec<u8>>,
}

impl ServerStdinPoller {
    fn new(receiver: crossbeam_channel::Receiver<Vec<u8>>) -> Self {
        Self { receiver }
    }
}

impl StdinPoller for ServerStdinPoller {
    fn ready(&mut self) -> bool {
        std::thread::sleep(std::time::Duration::from_millis(10));
        !self.receiver.is_empty()
    }
}
