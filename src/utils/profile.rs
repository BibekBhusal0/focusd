use std::env;
use std::path::PathBuf;

/// Profiles for app
///
/// different profile uses different path for database, config and socket
/// This is required so that focusd can be used while it's under developement
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
  Dev,
  Prod,
}

impl Profile {
  pub fn current() -> Self {
    let exe = env::current_exe().unwrap_or_default();

    if exe.to_string_lossy().contains("target/debug") {
      Self::Dev
    } else {
      Self::Prod
    }
  }

  pub fn tag(self) -> &'static str {
    match self {
      Self::Dev => "focusd-dev",
      Self::Prod => "focusd",
    }
  }

  /// Per-user socket path; never shared between users.
  pub fn socket_path(self) -> PathBuf {
    let name = format!("{}.sock", self.tag());
    if let Some(dir) = env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from) {
      return dir.join(self.tag()).join(name);
    }
    PathBuf::from(format!("/tmp/{}-{}.sock", self.tag(), unsafe {
      libc::getuid()
    }))
  }

  /// Socket directory. None with the /tmp fallback.
  pub fn runtime_dir(self) -> Option<PathBuf> {
    env::var_os("XDG_RUNTIME_DIR").map(|dir| PathBuf::from(dir).join(self.tag()))
  }

  pub fn db_path(self) -> PathBuf {
    let home = env::var_os("HOME").unwrap();

    match self {
      Self::Dev => PathBuf::from(home).join(".local/share/focusd/db/history-test.db"),
      Self::Prod => PathBuf::from(home).join(".local/share/focusd/db/history.db"),
    }
  }

  pub fn config_path(self) -> PathBuf {
    let home = env::var_os("HOME").unwrap();

    match self {
      Self::Dev => PathBuf::from(home).join(".config/focusd/config-dev.toml"),
      Self::Prod => PathBuf::from(home).join(".config/focusd/config.toml"),
    }
  }
}
