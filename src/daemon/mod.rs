use crate::utils::profile::Profile;
use std::path::PathBuf;

pub mod commands;
pub mod run;

pub fn socket_path() -> PathBuf {
  Profile::current().socket_path()
}
