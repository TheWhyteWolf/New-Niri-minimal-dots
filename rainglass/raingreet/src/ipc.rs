// SPDX-License-Identifier: GPL-3.0-or-later
// The greetd IPC worker thread. greetd owns PAM on the other side of
// $GREETD_SOCK; this thread speaks the length-prefixed JSON protocol via the
// greetd_ipc crate and reports back through a calloop channel, so pam_unix's
// ~2s failure delay never freezes the render loop (same motivation as
// rainlock's two-process auth split — here greetd is the other process).
//
// Password lifetime: it arrives as a transient String (copied out of the
// mlocked SecureBuf at submit), is serialized by greetd_ipc — the protocol
// itself ships it as plaintext JSON over the socket — and the String is
// zeroized right after the write. The serde encode buffer is an unavoidable
// transient copy inherent to greetd's protocol.

use greetd_ipc::codec::SyncCodec;
use greetd_ipc::{AuthMessageType, ErrorType, Request, Response};
use smithay_client_toolkit::reexports::calloop::channel::Sender as EventSender;
use std::os::unix::net::UnixStream;
use std::sync::mpsc;
use zeroize::Zeroize;

pub enum IpcCmd {
    CreateSession { username: String },
    PostAuth { password: Option<String> },
    StartSession { cmd: Vec<String> },
    Cancel,
}

#[derive(Debug)]
pub enum IpcEvent {
    /// greetd wants an answer to a prompt. The first (secret) one is expected
    /// and already covered by the password UI; any further prompt is not.
    Prompt { secret: bool, message: String },
    /// Display-only PAM Info/Error line.
    Info(String),
    /// Authentication succeeded; the app should send StartSession.
    AuthOk,
    /// Authentication failed; the app resets and sends Cancel.
    AuthFailed(String),
    /// start_session accepted — exit 0 so greetd launches the session.
    SessionStarted,
    /// The socket is gone or was never there. Not recoverable from inside;
    /// the app shows the message and stays up (VT switch still works).
    Fatal(String),
}

/// Where worker events go. The app uses the calloop channel (which wakes the
/// event loop); tests use a plain mpsc receiver.
trait EventSink: Send + 'static {
    fn emit(&self, ev: IpcEvent);
}

impl EventSink for EventSender<IpcEvent> {
    fn emit(&self, ev: IpcEvent) {
        let _ = self.send(ev);
    }
}

#[cfg(test)]
impl EventSink for mpsc::Sender<IpcEvent> {
    fn emit(&self, ev: IpcEvent) {
        let _ = self.send(ev);
    }
}

pub struct Ipc {
    tx: mpsc::Sender<IpcCmd>,
}

impl Ipc {
    pub fn send(&self, cmd: IpcCmd) {
        let _ = self.tx.send(cmd);
    }
}

pub fn spawn(events: EventSender<IpcEvent>) -> Ipc {
    let (tx, rx) = mpsc::channel::<IpcCmd>();
    std::thread::spawn(move || match std::env::var("GREETD_SOCK") {
        Ok(sock) => worker(rx, events, &sock),
        Err(_) => events.emit(IpcEvent::Fatal(
            "GREETD_SOCK not set — run under greetd".into(),
        )),
    });
    Ipc { tx }
}

fn worker(rx: mpsc::Receiver<IpcCmd>, events: impl EventSink, sock: &str) {
    let mut stream = match UnixStream::connect(sock) {
        Ok(s) => s,
        Err(e) => return events.emit(IpcEvent::Fatal(format!("cannot connect to {sock}: {e}"))),
    };

    // A session exists on the greetd side from CreateSession until Success
    // (started), an acknowledged Cancel, or a socket death.
    let mut in_session = false;

    while let Ok(cmd) = rx.recv() {
        let ok = match cmd {
            IpcCmd::CreateSession { username } => {
                // A lingering failed session blocks a new one; clear it first.
                if in_session {
                    let _ = cancel(&mut stream);
                    in_session = false;
                }
                match write(&mut stream, Request::CreateSession { username }) {
                    Ok(()) => {
                        in_session = true;
                        pump_auth(&mut stream, &events)
                    }
                    Err(e) => Err(e),
                }
            }
            IpcCmd::PostAuth { password } => {
                if !in_session {
                    continue; // stale submit after a failure raced the reset
                }
                let mut req = Request::PostAuthMessageResponse { response: password };
                let res = req.write_to(&mut stream).map_err(|e| e.to_string());
                if let Request::PostAuthMessageResponse { response: Some(s) } = &mut req {
                    s.zeroize();
                }
                match res {
                    Ok(()) => pump_auth(&mut stream, &events),
                    Err(e) => Err(e),
                }
            }
            IpcCmd::StartSession { cmd } => {
                match write(&mut stream, Request::StartSession { cmd, env: vec![] }) {
                    Ok(()) => match Response::read_from(&mut stream) {
                        Ok(Response::Success) => {
                            events.emit(IpcEvent::SessionStarted);
                            return; // nothing left to do; app exits 0
                        }
                        Ok(Response::Error { description, .. }) => {
                            in_session = false;
                            events.emit(IpcEvent::AuthFailed(format!(
                                "start_session: {description}"
                            )));
                            Ok(())
                        }
                        Ok(_) => Err("unexpected reply to start_session".to_string()),
                        Err(e) => Err(e.to_string()),
                    },
                    Err(e) => Err(e),
                }
            }
            IpcCmd::Cancel => {
                if in_session {
                    in_session = false;
                    cancel(&mut stream)
                } else {
                    Ok(())
                }
            }
        };
        if let Err(e) = ok {
            return events.emit(IpcEvent::Fatal(e));
        }
    }
}

fn write(stream: &mut UnixStream, req: Request) -> Result<(), String> {
    req.write_to(stream).map_err(|e| e.to_string())
}

fn cancel(stream: &mut UnixStream) -> Result<(), String> {
    write(stream, Request::CancelSession)?;
    // Ack (Success, or Error if greetd already dropped it) — ignore either.
    Response::read_from(stream)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// After CreateSession/PostAuth: read responses until greetd wants something
/// from the user (Secret/Visible prompt), succeeds, or fails. Info/Error
/// messages are display-only — forward them and auto-acknowledge with an
/// empty response so the conversation continues.
fn pump_auth(stream: &mut UnixStream, events: &impl EventSink) -> Result<(), String> {
    loop {
        match Response::read_from(stream).map_err(|e| e.to_string())? {
            Response::Success => {
                events.emit(IpcEvent::AuthOk);
                return Ok(());
            }
            Response::Error {
                error_type,
                description,
            } => {
                let msg = match error_type {
                    ErrorType::AuthError => {
                        if description.is_empty() {
                            "authentication failed".to_string()
                        } else {
                            description
                        }
                    }
                    ErrorType::Error => description,
                };
                events.emit(IpcEvent::AuthFailed(msg));
                return Ok(());
            }
            Response::AuthMessage {
                auth_message_type,
                auth_message,
            } => match auth_message_type {
                AuthMessageType::Secret | AuthMessageType::Visible => {
                    events.emit(IpcEvent::Prompt {
                        secret: matches!(auth_message_type, AuthMessageType::Secret),
                        message: auth_message,
                    });
                    return Ok(()); // next IpcCmd carries the answer
                }
                AuthMessageType::Info | AuthMessageType::Error => {
                    events.emit(IpcEvent::Info(auth_message));
                    write(stream, Request::PostAuthMessageResponse { response: None })?;
                }
            },
        }
    }
}

/// Debug-only stand-in for greetd so the whole UI loop can run as a normal
/// niri window: password "ok" authenticates, anything else fails after the
/// realistic PAM-ish delay. Never compiled into release binaries.
#[cfg(debug_assertions)]
pub fn spawn_stub(events: EventSender<IpcEvent>) -> Ipc {
    use std::time::Duration;
    let (tx, rx) = mpsc::channel::<IpcCmd>();
    std::thread::spawn(move || {
        while let Ok(cmd) = rx.recv() {
            match cmd {
                IpcCmd::CreateSession { username } => {
                    std::thread::sleep(Duration::from_millis(100));
                    eprintln!("raingreet[stub]: create_session for {username:?}");
                    let _ = events.send(IpcEvent::Prompt {
                        secret: true,
                        message: "Password:".into(),
                    });
                }
                IpcCmd::PostAuth { password } => {
                    std::thread::sleep(Duration::from_millis(800));
                    if password.as_deref() == Some("ok") {
                        let _ = events.send(IpcEvent::AuthOk);
                    } else {
                        let _ = events.send(IpcEvent::AuthFailed("stub: wrong password".into()));
                    }
                }
                IpcCmd::StartSession { cmd } => {
                    eprintln!("raingreet[stub]: would start {cmd:?}");
                    let _ = events.send(IpcEvent::SessionStarted);
                }
                IpcCmd::Cancel => eprintln!("raingreet[stub]: cancel_session"),
            }
        }
    });
    Ipc { tx }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::time::Duration;

    /// Worker + in-process mock greetd on a private socket path. The mock
    /// script mirrors tests/mock-greetd.py: password "ok" wins.
    fn harness(
        name: &str,
        script: impl FnOnce(&mut UnixStream) + Send + 'static,
    ) -> (Ipc, mpsc::Receiver<IpcEvent>) {
        let path =
            std::env::temp_dir().join(format!("raingreet-ipc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            script(&mut conn);
        });

        let (ev_tx, ev_rx) = mpsc::channel::<IpcEvent>();
        let (cmd_tx, cmd_rx) = mpsc::channel::<IpcCmd>();
        let sock = path.to_str().unwrap().to_string();
        std::thread::spawn(move || worker(cmd_rx, ev_tx, &sock));
        (Ipc { tx: cmd_tx }, ev_rx)
    }

    fn next(rx: &mpsc::Receiver<IpcEvent>) -> IpcEvent {
        rx.recv_timeout(Duration::from_secs(5))
            .expect("worker event")
    }

    fn read_req(conn: &mut UnixStream) -> Request {
        Request::read_from(conn).unwrap()
    }

    fn secret_prompt(conn: &mut UnixStream) {
        Response::AuthMessage {
            auth_message_type: AuthMessageType::Secret,
            auth_message: "Password: ".into(),
        }
        .write_to(conn)
        .unwrap();
    }

    #[test]
    fn full_login_conversation() {
        let (ipc, rx) = harness("full", |conn| {
            // Attempt 1: wrong password.
            assert!(
                matches!(read_req(conn), Request::CreateSession { username } if username == "voyd")
            );
            secret_prompt(conn);
            assert!(
                matches!(read_req(conn), Request::PostAuthMessageResponse { response: Some(p) } if p == "wrong")
            );
            Response::Error {
                error_type: ErrorType::AuthError,
                description: String::new(),
            }
            .write_to(conn)
            .unwrap();
            // App resets: cancel.
            assert!(matches!(read_req(conn), Request::CancelSession));
            Response::Success.write_to(conn).unwrap();
            // Attempt 2: right password, with a display-only Info first.
            assert!(matches!(read_req(conn), Request::CreateSession { .. }));
            secret_prompt(conn);
            assert!(
                matches!(read_req(conn), Request::PostAuthMessageResponse { response: Some(p) } if p == "ok")
            );
            Response::AuthMessage {
                auth_message_type: AuthMessageType::Info,
                auth_message: "Last login: yesterday".into(),
            }
            .write_to(conn)
            .unwrap();
            assert!(matches!(
                read_req(conn),
                Request::PostAuthMessageResponse { response: None }
            ));
            Response::Success.write_to(conn).unwrap();
            // Session start.
            assert!(
                matches!(read_req(conn), Request::StartSession { cmd, .. } if cmd == ["niri-session"])
            );
            Response::Success.write_to(conn).unwrap();
        });

        ipc.send(IpcCmd::CreateSession {
            username: "voyd".into(),
        });
        assert!(matches!(next(&rx), IpcEvent::Prompt { secret: true, .. }));
        ipc.send(IpcCmd::PostAuth {
            password: Some("wrong".into()),
        });
        assert!(matches!(next(&rx), IpcEvent::AuthFailed(_)));
        ipc.send(IpcCmd::Cancel);
        ipc.send(IpcCmd::CreateSession {
            username: "voyd".into(),
        });
        assert!(matches!(next(&rx), IpcEvent::Prompt { secret: true, .. }));
        ipc.send(IpcCmd::PostAuth {
            password: Some("ok".into()),
        });
        assert!(matches!(next(&rx), IpcEvent::Info(m) if m.contains("Last login")));
        assert!(matches!(next(&rx), IpcEvent::AuthOk));
        ipc.send(IpcCmd::StartSession {
            cmd: vec!["niri-session".into()],
        });
        assert!(matches!(next(&rx), IpcEvent::SessionStarted));
    }

    #[test]
    fn second_secret_prompt_is_surfaced() {
        // 2FA-shaped stack: the worker forwards the extra prompt; the app
        // treats it as unsupported and cancels.
        let (ipc, rx) = harness("twostep", |conn| {
            assert!(matches!(read_req(conn), Request::CreateSession { .. }));
            secret_prompt(conn);
            assert!(matches!(
                read_req(conn),
                Request::PostAuthMessageResponse { .. }
            ));
            secret_prompt(conn); // wants a second answer
            assert!(matches!(read_req(conn), Request::CancelSession));
            Response::Success.write_to(conn).unwrap();
        });

        ipc.send(IpcCmd::CreateSession {
            username: "voyd".into(),
        });
        assert!(matches!(next(&rx), IpcEvent::Prompt { .. }));
        ipc.send(IpcCmd::PostAuth {
            password: Some("pw".into()),
        });
        assert!(matches!(next(&rx), IpcEvent::Prompt { secret: true, .. }));
        ipc.send(IpcCmd::Cancel); // what the app does with an unexpected prompt
    }

    #[test]
    fn lingering_failed_session_is_cancelled_before_recreate() {
        let (ipc, rx) = harness("relogin", |conn| {
            assert!(matches!(read_req(conn), Request::CreateSession { .. }));
            secret_prompt(conn);
            assert!(matches!(
                read_req(conn),
                Request::PostAuthMessageResponse { .. }
            ));
            Response::Error {
                error_type: ErrorType::AuthError,
                description: "denied".into(),
            }
            .write_to(conn)
            .unwrap();
            // No app-side Cancel this time: CreateSession must self-clean.
            assert!(matches!(read_req(conn), Request::CancelSession));
            Response::Success.write_to(conn).unwrap();
            assert!(matches!(read_req(conn), Request::CreateSession { .. }));
            secret_prompt(conn);
        });

        ipc.send(IpcCmd::CreateSession {
            username: "voyd".into(),
        });
        assert!(matches!(next(&rx), IpcEvent::Prompt { .. }));
        ipc.send(IpcCmd::PostAuth {
            password: Some("pw".into()),
        });
        assert!(matches!(next(&rx), IpcEvent::AuthFailed(m) if m == "denied"));
        ipc.send(IpcCmd::CreateSession {
            username: "voyd".into(),
        });
        assert!(matches!(next(&rx), IpcEvent::Prompt { .. }));
    }

    #[test]
    fn dead_socket_is_fatal_not_hang() {
        let (ipc, rx) = harness("dead", |conn| {
            assert!(matches!(read_req(conn), Request::CreateSession { .. }));
            // Server dies mid-conversation.
            conn.shutdown(std::net::Shutdown::Both).unwrap();
        });
        ipc.send(IpcCmd::CreateSession {
            username: "voyd".into(),
        });
        assert!(matches!(next(&rx), IpcEvent::Fatal(_)));
    }

    #[test]
    fn missing_socket_is_fatal() {
        let (ev_tx, ev_rx) = mpsc::channel::<IpcEvent>();
        let (_cmd_tx, cmd_rx) = mpsc::channel::<IpcCmd>();
        std::thread::spawn(move || worker(cmd_rx, ev_tx, "/nonexistent/greetd.sock"));
        assert!(matches!(next(&ev_rx), IpcEvent::Fatal(_)));
    }
}
