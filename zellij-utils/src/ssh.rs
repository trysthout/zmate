use clap::Args;
use serde::{Serialize, Deserialize};



#[derive(Debug, Default, Clone, Args, Serialize, Deserialize)]
pub struct Ssh {
    #[clap(long, short, default_value="6222")]
    pub port: u16,
}