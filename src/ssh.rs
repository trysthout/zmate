use zellij_utils::{cli::CliArgs, ssh::Ssh};
use sshd::server::Server;
use tokio::runtime::Builder;


// In ssh mode, it will first the start server,then start client, finally deatch the session
pub(crate) fn start(args: CliArgs, ssh_opts: Ssh) {
    let server = Server::new(args, ssh_opts);
    let rt  = Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(server.listen()).unwrap();
}