use crate::{
  daemon::{
    commands::{Message, check_peer, handle_stream, own_uid, read_bounded_line, CLIENT_READ_TIMEOUT, MAX_LINE_BYTES},
    socket_path,
  },
  throw,
  timer::{
    engine::{decrease_sec, render_time},
    state::TimerState,
  },
  utils::profile::Profile,
};
use std::{
  fs,
  io::{BufReader, Result},
  os::unix::{
    fs::{DirBuilderExt, MetadataExt},
    net::{UnixListener, UnixStream},
  },
  path::Path,
  process::{self, Command, Stdio},
  sync::{Arc, Mutex},
  thread,
  time::Duration,
};

/// Checks if daemon is already running
///
/// Sends message to daemon to check if it's running
pub fn daemon_active() -> bool {
  let mut stream = match UnixStream::connect(socket_path()) {
    Ok(s) => s,
    Err(_) => return false,
  };
  if check_peer(&stream).is_err() {
    return false;
  }
  if stream.set_read_timeout(Some(CLIENT_READ_TIMEOUT)).is_err() {
    return false;
  }
  if write_running(&mut stream).is_err() {
    return false;
  }
  let mut reader = BufReader::new(stream);
  match read_bounded_line(&mut reader, MAX_LINE_BYTES) {
    Ok(line) => line.starts_with("YES"),
    Err(_) => false,
  }
}

fn write_running(stream: &mut UnixStream) -> Result<()> {
  use std::io::Write;
  stream.write_all(format!("{}\n", Message::Running).as_bytes())
}

/// Make sure daemon is running
///
/// If daemon is not runnign it starts up a background process launching daemon
pub fn ensure_daemon_active(first_try: bool) -> Result<()> {
  if daemon_active() {
    return Ok(());
  }

  Command::new(std::env::current_exe()?)
    .arg("--daemon")
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .spawn()?;

  if first_try {
    for _ in 1..20 {
      if daemon_active() {
        return Ok(());
      } else {
        thread::sleep(Duration::from_millis(50));
        ensure_daemon_active(false).ok();
      }
    }
  }
  throw!("Daemon not active")
}

/// Stop daemon if running. Removes only our own socket file.
pub fn stop_daemon() {
  println!("Stopping the daemon");
  let sp = socket_path();
  if Path::new(&sp).exists() {
    let ours = fs::metadata(&sp)
      .map(|m| m.uid() == own_uid())
      .unwrap_or(false);
    if ours {
      let _ = fs::remove_file(&sp);
    }
  }
  process::exit(0);
}

/// Create socket connection and initialize listener
///
/// Based on command recieved execute specific action
pub fn run_daemon() {
  println!("Starting Daemon");
  if daemon_active() {
    println!("Daemon already running stopping it");
    let _ = Message::StopDaemon.send();
  }
  // Delete the socket path if already exists
  let sp = socket_path();
  if let Some(dir) = Profile::current().runtime_dir() {
    if let Err(e) = fs::DirBuilder::new()
      .recursive(true)
      .mode(0o700)
      .create(&dir)
    {
      eprintln!("Cannot create runtime dir {}: {e}", dir.display());
      process::exit(1);
    }
  }
  if Path::new(&sp).exists() {
    let ours = fs::metadata(&sp)
      .map(|m| m.uid() == own_uid())
      .unwrap_or(false);
    if !ours {
      eprintln!(
        "Socket {} is owned by another user; refusing to replace it",
        sp.display()
      );
      process::exit(1);
    }
    println!("Socket already available removing it");
    if let Err(e) = fs::remove_file(&sp) {
      eprintln!("Cannot remove socket {}: {e}", sp.display());
      process::exit(1);
    }
  }

  let listener = match UnixListener::bind(&sp) {
    Ok(l) => l,
    Err(e) => {
      eprintln!("Cannot bind socket {}: {e}", sp.display());
      process::exit(1);
    }
  };
  let timer_state = Arc::new(Mutex::new(TimerState::default()));

  let s = Arc::clone(&timer_state);
  thread::spawn(move || {
    let tick_rate = Duration::from_secs(1);
    loop {
      thread::sleep(tick_rate);
      {
        let mut state = s.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.running {
          decrease_sec(&mut state);
          println!(
            "Clock is ticking, {} remain in {}",
            render_time(&state),
            state.session_type.name()
          );
        }
      }
    }
  });

  // Execute every command in different thread so that multiple operations could be done
  for stream in listener.incoming() {
    match stream {
      Ok(stream) => {
        let state = Arc::clone(&timer_state);
        thread::spawn(move || match handle_stream(stream, state) {
          Ok(_) => {}
          Err(e) => eprintln!("Error occurred in thread {e}"),
        });
      }
      Err(e) => eprintln!("Error occurred in stream {e}"),
    }
  }
}
