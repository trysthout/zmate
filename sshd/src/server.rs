use std::sync::Arc;

use russh::{server, MethodSet};
use tokio::sync::mpsc::unbounded_channel;
use zellij_utils::{cli::CliArgs, ssh::Ssh};

use crate::{
    handler::{Handler, HandlerEvent},
    session::Session,
};

pub struct Server {
    args: CliArgs,
    ssh_opts: Ssh,
}

impl Server {
    pub fn new(args: CliArgs, ssh_opts: Ssh) -> Self {
        Self { args, ssh_opts }
    }

    pub async fn listen(self) -> Result<(), std::io::Error> {
        let config = russh::server::Config {
            inactivity_timeout: Some(std::time::Duration::from_secs(3600)),
            auth_rejection_time: std::time::Duration::from_secs(3),
            auth_rejection_time_initial: Some(std::time::Duration::from_secs(0)),
            keys: vec![russh_keys::key::KeyPair::generate_ed25519().unwrap()],
            methods: MethodSet::PUBLICKEY,
            ..Default::default()
        };
        let config = Arc::new(config);
        russh::server::run(config, ("0.0.0.0", self.ssh_opts.port), self).await
    }
}

impl server::Server for Server {
    type Handler = Handler;

    fn new_client(&mut self, _peer_addr: Option<std::net::SocketAddr>) -> Self::Handler {
        let (event_tx, event_rx) = unbounded_channel::<HandlerEvent>();
        let mut sess = Session::new(self.args.clone(), event_rx);
        tokio::spawn(async move { sess.run().await });

        Handler::new(event_tx)
    }
}
