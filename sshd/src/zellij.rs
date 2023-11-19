
use std::{process, time::Duration, path::PathBuf, fs::File, io::Read, sync::{Arc, Mutex}, thread::{JoinHandle, self}};
use dialoguer::Confirm;
use log::info;
use russh::{Sig, ChannelId};
use tokio::sync::mpsc::UnboundedSender;
use zellij_client::{old_config_converter::{convert_old_yaml_files, config_yaml_to_config_kdl, layout_yaml_to_layout_kdl}, ClientInfo, os_input_output::{get_client_os_input, ClientOsApi}, ssh_client::start_client_ssh};
use zellij_server::{os_input_output::get_server_os_input, start_server};
use zellij_utils::{cli::{CliArgs, Command, Sessions, SessionCommand}, setup::Setup, input::{config::{ConfigError, Config}, options::Options, actions::Action, layout::Layout}, miette::{Report, Result}, data::{ConnectToSession, Style}, envs, nix, consts::ZELLIJ_SOCK_DIR, shared::set_permissions, ipc::{ClientAttributes, ClientToServerMsg}};
use crate::{ServerHandle, ZellijClientData, session_util::{assert_session_ne, resurrection_layout,  kill_session as kill_session_impl, delete_session as delete_session_impl, SessionNameMatch, get_active_session, ActiveSession, match_session_name, session_exists, get_sessions_sorted_by_mtime, print_sessions, get_sessions, get_resurrectable_sessions, assert_dead_session, assert_session, print_sessions_with_index, list_sessions, get_name_generator}, ssh_input_output::SshInputOutput};



pub(crate) fn kill_all_sessions(yes: bool) {
    match get_sessions() {
        Ok(sessions) if sessions.is_empty() => {
            eprintln!("No active zellij sessions found.");
            process::exit(1);
        },
        Ok(sessions) => {
            if !yes {
                println!("WARNING: this action will kill all sessions.");
                if !Confirm::new()
                    .with_prompt("Do you want to continue?")
                    .interact()
                    .unwrap()
                {
                    println!("Abort.");
                    process::exit(1);
                }
            }
            for session in &sessions {
                kill_session_impl(&session.0);
            }
            process::exit(0);
        },
        Err(e) => {
            eprintln!("Error occurred: {:?}", e);
            process::exit(1);
        },
    }
}

pub(crate) fn delete_all_sessions(yes: bool, force: bool) {
    let active_sessions: Vec<String> = get_sessions()
        .unwrap_or_default()
        .iter()
        .map(|s| s.0.clone())
        .collect();
    let resurrectable_sessions = get_resurrectable_sessions();
    let dead_sessions: Vec<_> = if force {
        resurrectable_sessions
    } else {
        resurrectable_sessions
            .iter()
            .filter(|(name, _, _)| !active_sessions.contains(name))
            .cloned()
            .collect()
    };
    if !yes {
        println!("WARNING: this action will delete all resurrectable sessions.");
        if !Confirm::new()
            .with_prompt("Do you want to continue?")
            .interact()
            .unwrap()
        {
            println!("Abort.");
            process::exit(1);
        }
    }
    for session in &dead_sessions {
        delete_session_impl(&session.0, force);
    }
    process::exit(0);
}

pub(crate) fn kill_session(target_session: &Option<String>) {
    match target_session {
        Some(target_session) => {
            assert_session(target_session);
            kill_session_impl(target_session);
            process::exit(0);
        },
        None => {
            println!("Please specify the session name to kill.");
            process::exit(1);
        },
    }
}

pub(crate) fn delete_session(target_session: &Option<String>, force: bool) {
    match target_session {
        Some(target_session) => {
            assert_dead_session(target_session, force);
            delete_session_impl(target_session, force);
            process::exit(0);
        },
        None => {
            println!("Please specify the session name to delete.");
            process::exit(1);
        },
    }
}

pub(crate) fn get_os_input<OsInputOutput>(
    fn_get_os_input: fn() -> Result<OsInputOutput, nix::Error>,
) -> OsInputOutput {
    match fn_get_os_input() {
        Ok(os_input) => os_input,
        Err(e) => {
            eprintln!("failed to open terminal:\n{}", e);
            process::exit(1);
        },
    }
}


fn create_new_client() -> ClientInfo {
    ClientInfo::New(generate_unique_session_name())
}

fn find_indexed_session(
    sessions: Vec<String>,
    config_options: Options,
    index: usize,
    create: bool,
) -> ClientInfo {
    match sessions.get(index) {
        Some(session) => ClientInfo::Attach(session.clone(), config_options),
        None if create => create_new_client(),
        None => {
            println!(
                "No session indexed by {} found. The following sessions are active:",
                index
            );
            print_sessions_with_index(sessions);
            process::exit(1);
        },
    }
}

/// Client entrypoint for all [`zellij_utils::cli::CliAction`]
///
/// Checks session to send the action to and attaches with client
pub(crate) fn send_action_to_session(
    cli_action: zellij_utils::cli::CliAction,
    requested_session_name: Option<String>,
    config: Option<Config>,
) {
    match get_active_session() {
        ActiveSession::None => {
            eprintln!("There is no active session!");
            std::process::exit(1);
        },
        ActiveSession::One(session_name) => {
            if let Some(requested_session_name) = requested_session_name {
                if requested_session_name != session_name {
                    eprintln!(
                        "Session '{}' not found. The following sessions are active:",
                        requested_session_name
                    );
                    eprintln!("{}", session_name);
                    std::process::exit(1);
                }
            }
            attach_with_cli_client(cli_action, &session_name, config);
        },
        ActiveSession::Many => {
            let existing_sessions: Vec<String> = get_sessions()
                .unwrap_or_default()
                .iter()
                .map(|s| s.0.clone())
                .collect();
            if let Some(session_name) = requested_session_name {
                if existing_sessions.contains(&session_name) {
                    attach_with_cli_client(cli_action, &session_name, config);
                } else {
                    eprintln!(
                        "Session '{}' not found. The following sessions are active:",
                        session_name
                    );
                    list_sessions(false, false);
                    std::process::exit(1);
                }
            } else if let Ok(session_name) = envs::get_session_name() {
                attach_with_cli_client(cli_action, &session_name, config);
            } else {
                eprintln!("Please specify the session name to send actions to. The following sessions are active:");
                list_sessions(false, false);
                std::process::exit(1);
            }
        },
    };
}
pub(crate) fn convert_old_config_file(old_config_file: PathBuf) {
    match File::open(&old_config_file) {
        Ok(mut handle) => {
            let mut raw_config_file = String::new();
            let _ = handle.read_to_string(&mut raw_config_file);
            match config_yaml_to_config_kdl(&raw_config_file, false) {
                Ok(kdl_config) => {
                    println!("{}", kdl_config);
                    process::exit(0);
                },
                Err(e) => {
                    eprintln!("Failed to convert config: {}", e);
                    process::exit(1);
                },
            }
        },
        Err(e) => {
            eprintln!("Failed to open file: {}", e);
            process::exit(1);
        },
    }
}

pub(crate) fn convert_old_layout_file(old_layout_file: PathBuf) {
    match File::open(&old_layout_file) {
        Ok(mut handle) => {
            let mut raw_layout_file = String::new();
            let _ = handle.read_to_string(&mut raw_layout_file);
            match layout_yaml_to_layout_kdl(&raw_layout_file) {
                Ok(kdl_layout) => {
                    println!("{}", kdl_layout);
                    process::exit(0);
                },
                Err(e) => {
                    eprintln!("Failed to convert layout: {}", e);
                    process::exit(1);
                },
            }
        },
        Err(e) => {
            eprintln!("Failed to open file: {}", e);
            process::exit(1);
        },
    }
}

pub(crate) fn convert_old_theme_file(old_theme_file: PathBuf) {
    match File::open(&old_theme_file) {
        Ok(mut handle) => {
            let mut raw_config_file = String::new();
            let _ = handle.read_to_string(&mut raw_config_file);
            match config_yaml_to_config_kdl(&raw_config_file, true) {
                Ok(kdl_config) => {
                    println!("{}", kdl_config);
                    process::exit(0);
                },
                Err(e) => {
                    eprintln!("Failed to convert config: {}", e);
                    process::exit(1);
                },
            }
        },
        Err(e) => {
            eprintln!("Failed to open file: {}", e);
            process::exit(1);
        },
    }
}

fn attach_with_cli_client(
    cli_action: zellij_utils::cli::CliAction,
    session_name: &str,
    config: Option<Config>,
) {
    let os_input = get_os_input(zellij_client::os_input_output::get_cli_client_os_input);
    let get_current_dir = || std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match Action::actions_from_cli(cli_action, Box::new(get_current_dir), config) {
        Ok(actions) => {
            zellij_client::cli_client::start_cli_client(Box::new(os_input), session_name, actions);
            std::process::exit(0);
        },
        Err(e) => {
            eprintln!("{}", e);
            log::error!("Error sending action: {}", e);
            std::process::exit(2);
        },
    }
}

fn attach_with_session_index(config_options: Options, index: usize, create: bool) -> ClientInfo {
    // Ignore the session_name when `--index` is provided
    match get_sessions_sorted_by_mtime() {
        Ok(sessions) if sessions.is_empty() => {
            if create {
                create_new_client()
            } else {
                eprintln!("No active zellij sessions found.");
                process::exit(1);
            }
        },
        Ok(sessions) => find_indexed_session(sessions, config_options, index, create),
        Err(e) => {
            eprintln!("Error occurred: {:?}", e);
            process::exit(1);
        },
    }
}

fn attach_with_session_name(
    session_name: Option<String>,
    config_options: Options,
    create: bool,
) -> ClientInfo {
    match &session_name {
        Some(session) if create => {
            if session_exists(session).unwrap() {
                ClientInfo::Attach(session_name.unwrap(), config_options)
            } else {
                ClientInfo::New(session_name.unwrap())
            }
        },
        Some(prefix) => match match_session_name(prefix).unwrap() {
            SessionNameMatch::UniquePrefix(s) | SessionNameMatch::Exact(s) => {
                ClientInfo::Attach(s, config_options)
            },
            SessionNameMatch::AmbiguousPrefix(sessions) => {
                println!(
                    "Ambiguous selection: multiple sessions names start with '{}':",
                    prefix
                );
                print_sessions(
                    sessions
                        .iter()
                        .map(|s| (s.clone(), Duration::default(), false))
                        .collect(),
                    false,
                    false,
                );
                process::exit(1);
            },
            SessionNameMatch::None => {
                eprintln!("No session with the name '{}' found!", prefix);
                process::exit(1);
            },
        },
        None => match get_active_session() {
            ActiveSession::None if create => create_new_client(),
            ActiveSession::None => {
                eprintln!("No active zellij sessions found.");
                process::exit(1);
            },
            ActiveSession::One(session_name) => ClientInfo::Attach(session_name, config_options),
            ActiveSession::Many => {
                println!("Please specify the session to attach to, either by using the full name or a unique prefix.\nThe following sessions are active:");
                list_sessions(false, false);
                process::exit(1);
            },
        },
    }
}

pub(crate) fn start_client(
    opts: CliArgs,
    sender: UnboundedSender<ZellijClientData>,
    server_receiver: crossbeam_channel::Receiver<Vec<u8>>,
    server_signal_receiver: crossbeam_channel::Receiver<Sig>,
    handle: ServerHandle,
    channel_id: ChannelId,
    win_size: libc::winsize,
) {
    // look for old YAML config/layout/theme files and convert them to KDL
    convert_old_yaml_files(&opts);
    let (config, layout, config_options) = match Setup::from_cli_args(&opts) {
        Ok(results) => results,
        Err(e) => {
            if let ConfigError::KdlError(error) = e {
                let report: Report = error.into();
                eprintln!("{:?}", report);
            } else {
                eprintln!("{}", e);
            }
            process::exit(1);
        },
    };
    let mut reconnect_to_session: Option<ConnectToSession> = None;
    let os_input = get_ssh_client_input(handle, channel_id, win_size, sender, server_receiver, server_signal_receiver);
    loop {
        let os_input = os_input.clone();
        let config = config.clone();
        let layout = layout.clone();
        let mut config_options = config_options.clone();
        let mut opts = opts.clone();
        let mut is_a_reconnect = false;

        if let Some(reconnect_to_session) = &reconnect_to_session {
            // this is integration code to make session reconnects work with this existing,
            // untested and pretty involved function
            //
            // ideally, we should write tests for this whole function and refctor it
            if reconnect_to_session.name.is_some() {
                opts.command = Some(Command::Sessions(Sessions::Attach {
                    session_name: reconnect_to_session.name.clone(),
                    create: true,
                    force_run_commands: false,
                    index: None,
                    options: None,
                }));
            } else {
                opts.command = None;
                opts.session = None;
                config_options.attach_to_session = None;
            }
            is_a_reconnect = true;
        }

        let start_client_plan = |session_name: std::string::String| {
            assert_session_ne(&session_name);
        };

        if let Some(Command::Sessions(Sessions::Attach {
            session_name,
            create,
            force_run_commands,
            index,
            options,
        })) = opts.command.clone()
        {
            let config_options = match options.as_deref() {
                Some(SessionCommand::Options(o)) => {
                    config_options.merge_from_cli(o.to_owned().into())
                },
                None => config_options,
            };

            let client = if let Some(idx) = index {
                attach_with_session_index(config_options.clone(), idx, create)
            } else {
                let session_exists = session_name
                    .as_ref()
                    .and_then(|s| session_exists(&s).ok())
                    .unwrap_or(false);
                let resurrection_layout =
                    session_name.as_ref().and_then(|s| resurrection_layout(&s));
                if create && !session_exists && resurrection_layout.is_none() {
                    session_name.clone().map(start_client_plan);
                }
                match (session_name.as_ref(), resurrection_layout) {
                    (Some(session_name), Some(mut resurrection_layout)) if !session_exists => {
                        if force_run_commands {
                            resurrection_layout.recursively_add_start_suspended(Some(false));
                        }
                        ClientInfo::Resurrect(session_name.clone(), resurrection_layout)
                    },
                    _ => attach_with_session_name(session_name, config_options.clone(), create),
                }
            };

            let attach_layout = match &client {
                ClientInfo::Attach(_, _) => None,
                ClientInfo::New(_) => Some(layout),
                ClientInfo::Resurrect(_session_name, layout_to_resurrect) => {
                    Some(layout_to_resurrect.clone())
                },
            };

            let tab_position_to_focus = reconnect_to_session
                .as_ref()
                .and_then(|r| r.tab_position.clone());
            let pane_id_to_focus = reconnect_to_session
                .as_ref()
                .and_then(|r| r.pane_id.clone());
            reconnect_to_session = start_client_ssh(
                Box::new(os_input),
                opts,
                config,
                config_options,
                client,
                attach_layout,
                tab_position_to_focus,
                pane_id_to_focus,
                is_a_reconnect,
            );
        } else {
            if let Some(session_name) = opts.session.clone() {
                start_client_plan(session_name.clone());
                reconnect_to_session = start_client_ssh(
                    Box::new(os_input),
                    opts,
                    config,
                    config_options,
                    ClientInfo::New(session_name),
                    Some(layout),
                    None,
                    None,
                    is_a_reconnect,
                );
            } else {
                if let Some(session_name) = config_options.session_name.as_ref() {
                    if let Ok(val) = envs::get_session_name() {
                        // This prevents the same type of recursion as above, only that here we
                        // don't get the command to "attach", but to start a new session instead.
                        // This occurs for example when declaring the session name inside a layout
                        // file and then, from within this session, trying to open a new zellij
                        // session with the same layout. This causes an infinite recursion in the
                        // `zellij_server::terminal_bytes::listen` task, flooding the server and
                        // clients with infinite `Render` requests.
                        if *session_name == val {
                            eprintln!("You are trying to attach to the current session (\"{}\"). Zellij does not support nesting a session in itself.", session_name);
                            process::exit(1);
                        }
                    }
                    match config_options.attach_to_session {
                        Some(true) => {
                            let client = attach_with_session_name(
                                Some(session_name.clone()),
                                config_options.clone(),
                                true,
                            );
                            let attach_layout = match &client {
                                ClientInfo::Attach(_, _) => None,
                                ClientInfo::New(_) => Some(layout),
                                ClientInfo::Resurrect(_, resurrection_layout) => {
                                    Some(resurrection_layout.clone())
                                },
                            };
                            reconnect_to_session = start_client_ssh(
                                Box::new(os_input),
                                opts,
                                config,
                                config_options,
                                client,
                                attach_layout,
                                None,
                                None,
                                is_a_reconnect,
                            );
                        },
                        _ => {
                            start_client_plan(session_name.clone());
                            reconnect_to_session = start_client_ssh(
                                Box::new(os_input),
                                opts,
                                config,
                                config_options.clone(),
                                ClientInfo::New(session_name.clone()),
                                Some(layout),
                                None,
                                None,
                                is_a_reconnect,
                            );
                        },
                    }
                    if reconnect_to_session.is_some() {
                        continue;
                    }
                    // after we detach, this happens and so we need to exit before the rest of the
                    // function happens
                    process::exit(0);
                }

                let session_name = generate_unique_session_name();
                start_client_plan(session_name.clone());
                reconnect_to_session = start_client_ssh(
                    Box::new(os_input),
                    opts,
                    config,
                    config_options,
                    ClientInfo::New(session_name),
                    Some(layout),
                    None,
                    None,
                    is_a_reconnect,
                );
            }
        }
        if reconnect_to_session.is_none() {
            break;
        }
    }
}

fn get_ssh_client_input(
    handle: ServerHandle,
    channel_id: ChannelId,
    win_size: libc::winsize,
    sender: UnboundedSender<ZellijClientData>,
    server_receiver: crossbeam_channel::Receiver<Vec<u8>>,
    server_signal_receiver: crossbeam_channel::Receiver<Sig>,
) -> SshInputOutput {
    let reading_from_stdin = Arc::new(Mutex::new(None));
    SshInputOutput {
        handle,
        win_size,
        channel_id,
        sender,
        server_receiver,
        server_signal_receiver,
        send_instructions_to_server: Arc::new(Mutex::new(None)),
        receive_instructions_from_server: Arc::new(Mutex::new(None)),
        reading_from_stdin,
        session_name: Arc::new(Mutex::new(None)),
    }
}


pub fn init_zellij_server(opts: CliArgs) -> JoinHandle<()> {
    if let Some(ref name) = opts.session {
        envs::set_session_name(name.clone());
    } else {
        if envs::get_session_name().is_err() {
            envs::set_session_name(generate_unique_session_name())
        }
    }

    log::info!("session_name: {:?}", envs::get_session_name());

    zellij_utils::consts::DEBUG_MODE.set(opts.debug).unwrap();
    let os_input = get_os_input(get_server_os_input);

    let thread_join_handle = thread::spawn(move || start_server(Box::new(os_input), create_ipc_pipe(), true));

    let (config, layout, config_options) = match Setup::from_cli_args(&opts) {
        Ok(results) => results,
        Err(e) => {
            if let ConfigError::KdlError(error) = e {
                let report: Report = error.into();
                eprintln!("{report:?}");
            } else {
                eprintln!("{e}");
            }
            std::process::exit(1);
        },
    };

    let os_input = get_os_input(get_client_os_input);

    init_zellij_client(
        Box::new(os_input),
        opts,
        config,
        config_options,
        Some(layout),
        None,
        None,
        create_ipc_pipe(),
    );
    thread_join_handle
}


pub fn init_zellij_client(
    os_input: Box<dyn ClientOsApi>,
    opts: zellij_utils::cli::CliArgs,
    config: Config,
    config_options: Options,
    layout: Option<Layout>,
    _tab_position_to_focus: Option<usize>,
    _pane_id_to_focus: Option<(u32, bool)>, // (pane_id, is_plugin)
    ipc: PathBuf,
) {
    info!("Initialize Zellij client!");

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

    let first_msg = ClientToServerMsg::NewClient(
        client_attributes,
        Box::new(opts),
        Box::new(config_options),
        Box::new(layout.unwrap()),
        Some(config.plugins),
    );

    os_input.connect_to_server(&ipc);
    os_input.send_to_server(first_msg);
    os_input.send_to_server(ClientToServerMsg::DetachSession(vec![1]))
}



fn generate_unique_session_name() -> String {
    let sessions = get_sessions().map(|sessions| {
        sessions
            .iter()
            .map(|s| s.0.clone())
            .collect::<Vec<String>>()
    });
    let dead_sessions: Vec<String> = get_resurrectable_sessions()
        .iter()
        .map(|(s, _, _)| s.clone())
        .collect();
    let Ok(sessions) = sessions else {
        eprintln!("Failed to list existing sessions: {:?}", sessions);
        process::exit(1);
    };

    let name = get_name_generator()
        .take(1000)
        .find(|name| !sessions.contains(name) && !dead_sessions.contains(name));

    if let Some(name) = name {
        return name;
    } else {
        eprintln!("Failed to generate a unique session name, giving up");
        process::exit(1);
    }
}

fn create_ipc_pipe() -> PathBuf {
    let mut sock_dir = ZELLIJ_SOCK_DIR.clone();
    std::fs::create_dir_all(&sock_dir).unwrap();
    set_permissions(&sock_dir, 0o700).unwrap();
    sock_dir.push(envs::get_session_name().unwrap());
    sock_dir
}
