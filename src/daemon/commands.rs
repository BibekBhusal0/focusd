use crate::{
  daemon::{run::stop_daemon, socket_path},
  timer::{
    engine::{
      add_time, next_session, render_time, reset_session, select_preset, start_session,
      start_specific_session,
    },
    hooks::{pause_hooks, resume_hooks, session_start_hook},
    state::{SessionType, TimerSnapShot, TimerState},
  },
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
  io::{BufRead, BufReader, Error, ErrorKind, Result, Write},
  os::unix::net::UnixStream,
  sync::{Arc, Mutex},
  time::Duration,
};

pub const MAX_LINE_BYTES: usize = 65536;
pub const CLIENT_READ_TIMEOUT: Duration = Duration::from_secs(5);
pub const DAEMON_READ_TIMEOUT: Duration = Duration::from_secs(30);

pub fn own_uid() -> u32 {
  unsafe { libc::getuid() }
}

/// Reject peers running as another user.
pub fn check_peer(stream: &UnixStream) -> Result<()> {
  if peer_uid(stream)? != own_uid() {
    return Err(Error::new(
      ErrorKind::PermissionDenied,
      "socket peer is not the current user",
    ));
  }
  Ok(())
}

fn peer_uid(stream: &UnixStream) -> Result<u32> {
  use std::os::unix::io::AsRawFd;
  let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
  let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
  let ret = unsafe {
    libc::getsockopt(
      stream.as_raw_fd(),
      libc::SOL_SOCKET,
      libc::SO_PEERCRED,
      &mut cred as *mut _ as *mut libc::c_void,
      &mut len,
    )
  };
  if ret != 0 {
    return Err(Error::last_os_error());
  }
  Ok(cred.uid)
}

/// One capped line; overlong input and timeouts are errors.
pub fn read_bounded_line(reader: &mut impl BufRead, cap: usize) -> Result<String> {
  let mut buf = Vec::new();
  let mut byte = [0u8; 1];
  loop {
    match reader.read(&mut byte) {
      Ok(0) => break,
      Ok(_) => {
        if buf.len() >= cap {
          return Err(Error::new(ErrorKind::InvalidData, "response too long"));
        }
        buf.push(byte[0]);
        if byte[0] == b'\n' {
          break;
        }
      }
      Err(e) => return Err(e),
    }
  }
  String::from_utf8(buf).map_err(|_| Error::new(ErrorKind::InvalidData, "response not utf-8"))
}

#[derive(PartialEq)]
pub enum Message {
  // Sessions
  StartSession,
  PauseSession,
  ResumeSession,
  ToggleSession,
  NextSession,
  GetSession,
  ResetSession,

  // Test
  Running,

  //Other
  StopDaemon,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub enum PayloadMessage {
  SelectSession,
  SelectPreset,
  AddMinutes,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PayloadType {
  message: PayloadMessage,
  payload: Value,
}

pub fn seperate_message_and_payload(message: String) -> Result<PayloadType> {
  let p = message.strip_prefix("MESSAGE_PAYLOAD: ").unwrap();
  let payload: PayloadType = serde_json::from_str(p)?;
  Ok(payload)
}

impl PayloadMessage {
  pub fn send(self, payload: Value) -> Result<String> {
    // let message = self;
    let p = PayloadType {
      message: self,
      payload,
    };
    let json = serde_json::to_string(&p)?;
    let command = format!("MESSAGE_PAYLOAD: {json}");
    send_command(command)
  }
}

impl Message {
  pub fn send(self) -> Result<String> {
    let cmd = self.to_string();
    send_command(cmd)
  }

  pub fn from_string(s: String) -> Option<Self> {
    match s.trim() {
      "StartSession" => Some(Self::StartSession),
      "PauseSession" => Some(Self::PauseSession),
      "ResumeSession" => Some(Self::ResumeSession),
      "ToggleSession" => Some(Self::ToggleSession),
      "NextSession" => Some(Self::NextSession),
      "GetSession" => Some(Self::GetSession),
      "ResetSession" => Some(Self::ResetSession),
      "RUNNING" => Some(Self::Running),
      "StopDaemon" => Some(Self::StopDaemon),
      _ => None,
    }
  }
}

impl std::fmt::Display for PayloadMessage {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      Self::SelectPreset => write!(f, "SelectPreset"),
      Self::SelectSession => write!(f, "SelectSession"),
      Self::AddMinutes => write!(f, "AddMinutes"),
    }
  }
}

impl std::fmt::Display for Message {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      Self::StartSession => write!(f, "StartSession"),
      Self::PauseSession => write!(f, "PauseSession"),
      Self::ResumeSession => write!(f, "ResumeSession"),
      Self::ToggleSession => write!(f, "ToggleSession"),
      Self::NextSession => write!(f, "NextSession"),
      Self::GetSession => write!(f, "GetSession"),
      Self::ResetSession => write!(f, "ResetSession"),
      Self::StopDaemon => write!(f, "StopDaemon"),
      Self::Running => write!(f, "RUNNING"),
    }
  }
}

pub fn contains_payload(message: &str) -> bool {
  message.trim().starts_with("MESSAGE_PAYLOAD:")
}

pub fn handle_stream(mut stream: UnixStream, state: Arc<Mutex<TimerState>>) -> Result<()> {
  check_peer(&stream)?;
  stream.set_read_timeout(Some(DAEMON_READ_TIMEOUT))?;
  let mut reader = BufReader::new(stream.try_clone()?);
  let line = read_bounded_line(&mut reader, MAX_LINE_BYTES)?;

  let mut s = state
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner());
  let l = line.trim().to_string();
  println!("Got message {l}");

  let response;
  if !contains_payload(&l) {
    response = match Message::from_string(l) {
      Some(m) => match m {
        Message::Running => "YES".to_string(),
        Message::GetSession => {
          let snapshot = TimerSnapShot::from(&*s);
          serde_json::to_string(&snapshot).unwrap()
        }
        Message::NextSession => {
          next_session(&mut s);
          format!("{} Session started", s.session_type.name())
        }
        Message::StopDaemon => {
          stop_daemon();
          "OK".to_string()
        }
        Message::ResetSession => {
          reset_session(&mut s);
          format!("{} Session restarted", s.session_type.name())
        }
        Message::StartSession => {
          start_session(&mut s);
          format!("{} Session started", s.session_type.name())
        }
        Message::ResumeSession | Message::PauseSession | Message::ToggleSession => {
          let new_running = if m == Message::ResumeSession {
            true
          } else if m == Message::PauseSession {
            false
          } else {
            !s.running
          };
          s.running = new_running;

          let mut status = "resumed";
          if new_running {
            if s.time_remaining == s.session_type.get_time() {
              session_start_hook(s.session_type);
            } else {
              resume_hooks(s.session_type);
            }
          } else {
            pause_hooks(s.session_type);
            status = "paused";
          }
          format!(
            "{} {} time remaining: {}",
            s.session_type.name(),
            status,
            render_time(&s)
          )
        }
      },
      None => "Unknown command".to_string(),
    };
  } else {
    let p = seperate_message_and_payload(l)?;
    match p.message {
      PayloadMessage::SelectSession => {
        let value = p.payload.as_str().unwrap();
        let session_type = SessionType::from_string(value);
        start_specific_session(&mut s, session_type);
        response = format!("{} Session started", session_type.name());
      }
      PayloadMessage::SelectPreset => {
        let value = p.payload.as_str().unwrap();
        match select_preset(&mut s, value) {
          Ok(_) => response = format!("Preset {} selected", value),
          Err(e) => response = e.to_string(),
        }
      }
      PayloadMessage::AddMinutes => {
        let value = p.payload.as_u64().unwrap();
        add_time(&mut s, Duration::from_mins(value));
        response = format!("Added {} minutes", value);
      }
    }
  }

  stream.write_all(format!("{}\n", response).as_bytes())?;
  Ok(())
}

pub fn send_command(command: String) -> Result<String> {
  let mut stream = UnixStream::connect(socket_path())?;
  check_peer(&stream)?;
  stream.set_read_timeout(Some(CLIENT_READ_TIMEOUT))?;
  stream.write_all(format!("{}\n", command).as_bytes())?;
  let mut reader = BufReader::new(stream);
  read_bounded_line(&mut reader, MAX_LINE_BYTES)
}

pub fn get_timer_state() -> Result<TimerState> {
  let m = Message::GetSession.send()?;
  let state: TimerSnapShot = serde_json::from_str(&m)?;
  Ok(TimerState::from(&state))
}
