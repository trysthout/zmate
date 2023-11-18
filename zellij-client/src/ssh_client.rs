use std::{thread, sync::{Mutex, Arc}};

use zellij_utils::{cli::CliArgs, input::{config::Config, layout::Layout, options::Options}, ipc::{ClientAttributes, ClientToServerMsg, ServerToClientMsg, ExitReason}, envs, data::{Style, ConnectToSession}, shared::set_permissions, consts::ZELLIJ_SOCK_DIR, channels::{ChannelWithContext, SenderWithContext, self}, errors::{FatalError, ContextType}};

use crate::{os_input_output::ClientOsApi, ClientInfo, command_is_executing::CommandIsExecuting, stdin_ansi_parser::{StdinAnsiParser, SyncOutput}, ClientInstruction, stdin_handler::stdin_loop, input_handler::input_loop, spawn_server, InputInstruction};
use log::info;

pub fn start_client_ssh(
    mut os_input: Box<dyn ClientOsApi>,
    opts: CliArgs,
    config: Config,
    config_options: Options,
    info: ClientInfo,
    layout: Option<Layout>,
    tab_position_to_focus: Option<usize>,
    pane_id_to_focus: Option<(u32, bool)>, // (pane_id, is_plugin)
    is_a_reconnect: bool,
) -> Option<ConnectToSession> {
    info!("Starting Zellij ssh client!");

    let mut reconnect_to_session = None;
    let clear_client_terminal_attributes = "\u{1b}[?1l\u{1b}=\u{1b}[r\u{1b}[?1000l\u{1b}[?1002l\u{1b}[?1003l\u{1b}[?1005l\u{1b}[?1006l\u{1b}[?12l";
    let take_snapshot = "\u{1b}[?1049h";
    let bracketed_paste = "\u{1b}[?2004h";

    if !is_a_reconnect {
        // we don't do this for a reconnect because our controlling terminal already has the
        // attributes we want from it, and some terminals don't treat these atomically (looking at
        // your Windows Terminal...)
        let _ = os_input
            .get_stdout_writer()
            .write(take_snapshot.as_bytes())
            .unwrap();
        let _ = os_input
            .get_stdout_writer()
            .write(clear_client_terminal_attributes.as_bytes())
            .unwrap();
    }
    envs::set_zellij("0".to_string());
    config.env.set_vars();

    let palette = config
        .theme_config(&config_options)
        .unwrap_or_else(|| os_input.load_palette());

    let full_screen_ws = os_input.get_terminal_size_using_fd(0);
    let client_attributes = ClientAttributes {
        size: full_screen_ws,
        style: Style {
            colors: palette,
            rounded_corners: config.ui.pane_frames.rounded_corners,
            hide_session_name: config.ui.pane_frames.hide_session_name,
        },
        keybinds: config.keybinds.clone(),
    };

    let create_ipc_pipe = || -> std::path::PathBuf {
        let mut sock_dir = ZELLIJ_SOCK_DIR.clone();
        std::fs::create_dir_all(&sock_dir).unwrap();
        set_permissions(&sock_dir, 0o700).unwrap();
        sock_dir.push(envs::get_session_name().unwrap());
        sock_dir
    };

    let (first_msg, ipc_pipe) = match info {
        ClientInfo::Attach(name, config_options) => {
            envs::set_session_name(name.clone());
            os_input.update_session_name(name);
            let ipc_pipe = create_ipc_pipe();

            (
                ClientToServerMsg::AttachClient(
                    client_attributes,
                    config_options,
                    tab_position_to_focus,
                    pane_id_to_focus,
                ),
                ipc_pipe,
            )
        },
        ClientInfo::New(name) | ClientInfo::Resurrect(name, _) => {
            envs::set_session_name(name.clone());
            os_input.update_session_name(name);
            let ipc_pipe = create_ipc_pipe();

            spawn_server(&*ipc_pipe, opts.debug).unwrap();

            (
                ClientToServerMsg::NewClient(
                    client_attributes,
                    Box::new(opts),
                    Box::new(config_options.clone()),
                    Box::new(layout.unwrap()),
                    Some(config.plugins.clone()),
                ),
                ipc_pipe,
            )
        },
    };

    os_input.connect_to_server(&*ipc_pipe);
    os_input.send_to_server(first_msg);

    let mut command_is_executing = CommandIsExecuting::new();

    let _ = os_input
        .get_stdout_writer()
        .write(bracketed_paste.as_bytes())
        .unwrap();

    let (send_client_instructions, receive_client_instructions): ChannelWithContext<
        ClientInstruction,
    > = channels::bounded(50);
    let send_client_instructions = SenderWithContext::new(send_client_instructions);

    let (send_input_instructions, receive_input_instructions): ChannelWithContext<
        InputInstruction,
    > = channels::bounded(50);
    let send_input_instructions = SenderWithContext::new(send_input_instructions);

    std::panic::set_hook({
        use zellij_utils::errors::handle_panic;
        let send_client_instructions = send_client_instructions.clone();
        Box::new(move |info| {
            handle_panic(info, &send_client_instructions);
        })
    });

    let on_force_close = config_options.on_force_close.unwrap_or_default();
    let stdin_ansi_parser = Arc::new(Mutex::new(StdinAnsiParser::new()));

    let _stdin_thread = thread::Builder::new()
        .name("stdin_handler".to_string())
        .spawn({
            let os_input = os_input.clone();
            let send_input_instructions = send_input_instructions.clone();
            let stdin_ansi_parser = stdin_ansi_parser.clone();
            move || stdin_loop(os_input, send_input_instructions, stdin_ansi_parser)
        });

    let _input_thread = thread::Builder::new()
        .name("input_handler".to_string())
        .spawn({
            let send_client_instructions = send_client_instructions.clone();
            let command_is_executing = command_is_executing.clone();
            let os_input = os_input.clone();
            let default_mode = config_options.default_mode.unwrap_or_default();
            move || {
                input_loop(
                    os_input,
                    config,
                    config_options,
                    command_is_executing,
                    send_client_instructions,
                    default_mode,
                    receive_input_instructions,
                )
            }
        });

    let _signal_thread = thread::Builder::new()
        .name("signal_listener".to_string())
        .spawn({
            let os_input = os_input.clone();
            move || {
                os_input.handle_signals(
                    Box::new({
                        let os_api = os_input.clone();
                        move || {
                            os_api.send_to_server(ClientToServerMsg::TerminalResize(
                                os_api.get_terminal_size_using_fd(0),
                            ));
                        }
                    }),
                    Box::new({
                        let os_api = os_input.clone();
                        move || {
                            os_api.send_to_server(ClientToServerMsg::Action(
                                on_force_close.into(),
                                None,
                                None,
                            ));
                        }
                    }),
                );
            }
        })
        .unwrap();

    let router_thread = thread::Builder::new()
        .name("router".to_string())
        .spawn({
            let os_input = os_input.clone();
            let mut should_break = false;
            move || loop {
                match os_input.recv_from_server() {
                    Some((instruction, err_ctx)) => {
                        err_ctx.update_thread_ctx();
                        if let ServerToClientMsg::Exit(_) = instruction {
                            should_break = true;
                        }
                        send_client_instructions.send(instruction.into()).unwrap();
                        if should_break {
                            break;
                        }
                    },
                    None => {
                        send_client_instructions
                            .send(ClientInstruction::UnblockInputThread)
                            .unwrap();
                        log::error!("Received empty message from server");
                        send_client_instructions
                            .send(ClientInstruction::Error(
                                "Received empty message from server".to_string(),
                            ))
                            .unwrap();
                        break;
                    },
                }
            }
        })
        .unwrap();

    let handle_error = |backtrace: String| {
        let goto_start_of_last_line = format!("\u{1b}[{};{}H", full_screen_ws.rows, 1);
        let restore_snapshot = "\u{1b}[?1049l";
        os_input.disable_mouse().non_fatal();
        let error = format!(
            "{}\n{}{}\n",
            restore_snapshot, goto_start_of_last_line, backtrace
        );
        let _ = os_input
            .get_stdout_writer()
            .write(error.as_bytes())
            .unwrap();
        let _ = os_input.get_stdout_writer().flush().unwrap();
    };

    let mut exit_msg = String::new();
    let mut loading = true;
    let mut pending_instructions = vec![];
    let mut synchronised_output = match os_input.env_variable("TERM").as_deref() {
        Some("alacritty") => Some(SyncOutput::DCS),
        _ => None,
    };

    let mut stdout = os_input.get_stdout_writer();
    stdout
        .write_all("\u{1b}[1m\u{1b}[HLoading Zellij\u{1b}[m\n\r".as_bytes())
        .expect("cannot write to stdout");
    stdout.flush().expect("could not flush");

    loop {
        let (client_instruction, mut err_ctx) = if !loading && !pending_instructions.is_empty() {
            // there are buffered instructions, we need to go through them before processing the
            // new ones
            pending_instructions.remove(0)
        } else {
            receive_client_instructions
                .recv()
                .expect("failed to receive app instruction on channel")
        };

        if loading {
            // when the app is still loading, we buffer instructions and show a loading screen
            match client_instruction {
                ClientInstruction::StartedParsingStdinQuery => {
                    stdout
                        .write_all("Querying terminal emulator for \u{1b}[32;1mdefault colors\u{1b}[m and \u{1b}[32;1mpixel/cell\u{1b}[m ratio...".as_bytes())
                        .expect("cannot write to stdout");
                    stdout.flush().expect("could not flush");
                },
                ClientInstruction::DoneParsingStdinQuery => {
                    stdout
                        .write_all("done".as_bytes())
                        .expect("cannot write to stdout");
                    stdout.flush().expect("could not flush");
                    loading = false;
                },
                instruction => {
                    pending_instructions.push((instruction, err_ctx));
                },
            }
            continue;
        }

        err_ctx.add_call(ContextType::Client((&client_instruction).into()));

        match client_instruction {
            ClientInstruction::Exit(reason) => {
                os_input.send_to_server(ClientToServerMsg::ClientExited);

                if let ExitReason::Error(_) = reason {
                    handle_error(reason.to_string());
                }
                exit_msg = reason.to_string();

                break;
            },
            ClientInstruction::Error(backtrace) => {
                handle_error(backtrace);
            },
            ClientInstruction::Render(output) => {
                let mut stdout = os_input.get_stdout_writer();
                if let Some(sync) = synchronised_output {
                    stdout
                        .write_all(sync.start_seq())
                        .expect("cannot write to stdout");
                }

                let _ = stdout.write(output.as_bytes());
                if let Some(sync) = synchronised_output {
                    stdout
                        .write_all(sync.end_seq())
                        .expect("cannot write to stdout");
                }
                stdout.flush().expect("could not flush");
            },
            ClientInstruction::UnblockInputThread => {
                command_is_executing.unblock_input_thread();
            },
            ClientInstruction::SwitchToMode(input_mode) => {
                send_input_instructions
                    .send(InputInstruction::SwitchToMode(input_mode))
                    .unwrap();
            },
            ClientInstruction::Log(lines_to_log) => {
                for line in lines_to_log {
                    log::info!("{line}");
                }
            },
            ClientInstruction::SwitchSession(connect_to_session) => {
                reconnect_to_session = Some(connect_to_session);
                os_input.send_to_server(ClientToServerMsg::ClientExited);
                break;
            },
            ClientInstruction::SetSynchronizedOutput(enabled) => {
                synchronised_output = enabled;
            },
            _ => {},
        }
    }

    router_thread.join().unwrap();

    if reconnect_to_session.is_none() {
        let reset_style = "\u{1b}[m";
        let show_cursor = "\u{1b}[?25h";
        let restore_snapshot = "\u{1b}[?1049l";
        let goto_start_of_last_line = format!("\u{1b}[{};{}H", full_screen_ws.rows, 1);
        let goodbye_message = format!(
            "{}\n{}{}{}{}\n",
            goto_start_of_last_line, restore_snapshot, reset_style, show_cursor, exit_msg
        );

        os_input.disable_mouse().non_fatal();
        info!("{}", exit_msg);
        let mut stdout = os_input.get_stdout_writer();
        let _ = os_input
            .get_stdout_writer()
            .write(clear_client_terminal_attributes.as_bytes())
            .unwrap();
        let _ = stdout.write(goodbye_message.as_bytes()).unwrap();
        let _ = send_input_instructions.send(InputInstruction::Exit);
        os_input.close();

    } else {
        let clear_screen = "\u{1b}[2J";
        let mut stdout = os_input.get_stdout_writer();
        let _ = stdout.write(clear_screen.as_bytes()).unwrap();
        stdout.flush().unwrap();
    }

    reconnect_to_session
}