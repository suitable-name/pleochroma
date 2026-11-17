//! One remote administration session: the gate that lets it in and the loop that serves it.
//!
//! # The gate
//!
//! [`serve_connection`] runs for a monitor or admin certificate once the viewer port has
//! authenticated it (`crate::serve`). It reads the `HELLO`, applies the viewer port's rule for
//! remote administration (`crate::serve::handshake::check_role_with_admin`: the setting is on,
//! the `HELLO` role is the certificate's role and the certificate is on the role's allowlist,
//! read afresh for every connection) and answers a `WELCOME` or a plain refusal.
//!
//! # The loop
//!
//! [`run`] reads [`AdminRequest`]s and answers each with exactly one
//! [`AdminPush::Response`] (a run of them for `FetchResult`). Every request goes through, in
//! this order:
//!
//! 1. the permission table (`permissions::allowed`): a monitor may read, an administrator may
//!    do everything except make monitor and administrator codes;
//! 2. the limits (`validate::request`);
//! 3. the action, through the same control API the coordinator window uses
//!    ([`ServeCore`]'s methods and `settings::apply_patch`);
//! 4. for a command, the audit trail: a `CoordinatorEvent::AdminAction` for every subscriber
//!    (the coordinator's own window included) and one `info` log line,
//!    `admin action: <name> (<fingerprint>) <action> <target>`.
//!
//! What no request can do (network and port changes, the remote-administration switch,
//! resetting pairings, rotating the issuing secret, exporting a backup, stopping the
//! coordinator, making monitor and administrator codes) is not in the protocol at all: the
//! switch and the rest only exist on the coordinator's own computer, and the one request
//! that could reach a monitor or administrator code (`IssueToken`) is refused here twice.
//!
//! After `Subscribe` the session also pushes, on its own schedule: the status at most once
//! per requested interval (never faster than once a second) and only when it changed, every
//! coordinator event, the log lines at the subscribed level (none for a monitor, because a log
//! line can hold a local folder path), and a heartbeat every 15 seconds. A subscribed session is exempt from the idle close; an unsubscribed one ends
//! after five silent minutes.
//!
//! Nothing here writes a token, a pairing link or a key to a log: a code appears only in the
//! reply that carries it to the administrator who asked.

use super::{
    SessionIo,
    io::{Poll, ReadTimeout, poll_message},
    restart, results,
};
use crate::{
    control::{status, table::device_name_of_common_name},
    pki::role::{allowlist_in, role_name, subject_common_name},
    serve::{
        ServeCore,
        handshake::{self, AdminPolicy, HelloCheck},
    },
    settings,
};
use pleochroma_net::{
    admin::{
        permissions::{self, NeverRemote, access_role_for, never_remote_denial},
        protocol::{
            AdminError, AdminPush, AdminReply, AdminRequest, HEARTBEAT_INTERVAL,
            MIN_STATUS_INTERVAL_MS, idle_close_applies,
        },
        status::{
            AccessRole, CoordinatorEvent, CoordinatorStatus, JobId, LogLevel, LogLine,
            SettingsPatch, fingerprint_short,
        },
        validate,
    },
    framing::IDLE_READ_TIMEOUT,
    handshake::UNKNOWN_BUILD_HASH,
    messages::{NetError, PayloadEncoding, PeerRole, Welcome, error_codes, write_message},
    tls::{Allowlist, Fingerprint, fingerprint, fingerprint_from_hex, fingerprint_to_hex},
};
use std::{
    collections::VecDeque,
    net::SocketAddr,
    sync::{Arc, mpsc::Receiver},
    time::{Duration, Instant},
};

/// How long the loop waits for the first byte of a request before it looks after the pushes.
const POLL: Duration = Duration::from_millis(100);

/// How long a write may take before the administrator is given up on.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// How many log lines a new subscriber gets from before it subscribed.
const LOG_BACKLOG: usize = 200;

/// The most pushes of one kind sent in one round, so a flood cannot starve the requests.
const PUSH_BURST: usize = 256;

/// The refusal for revoking the device that is asking.
const NOT_YOURSELF: &str = "You cannot remove the device you are connected with. Remove it on \
     the coordinator's own computer, or from another administrator.";

/// Whether the certificate `fingerprint` is on `role`'s allowlist in the coordinator's
/// certificate folder. Read from the file every time, so removing a device takes effect at its
/// next connection (and `revoke` closes the live ones).
pub(crate) fn is_listed(host: &ServeCore, role: PeerRole, fingerprint: &Fingerprint) -> bool {
    host.control.pki_dir().is_some_and(|dir| {
        Allowlist::load(&allowlist_in(&dir, role)).is_ok_and(|list| list.contains(fingerprint))
    })
}

/// The device name a certificate stands for: its Common Name without the role prefix, or the
/// short fingerprint when it has none. Control characters are dropped, so the name is safe in
/// a log line.
fn certificate_name(
    certificate: &rustls::pki_types::CertificateDer<'_>,
    fingerprint: &Fingerprint,
) -> String {
    subject_common_name(certificate.as_ref())
        .ok()
        .flatten()
        .map(|cn| device_name_of_common_name(&cn).to_owned())
        .map(|name| name.chars().filter(|c| !c.is_control()).collect::<String>())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| fingerprint_short(&fingerprint_to_hex(fingerprint)))
}

/// Runs the gate and then the session for one authenticated monitor or admin connection.
///
/// `certificate` is the peer's client certificate (always present over TLS). Anything the gate
/// refuses gets a plain `ErrorMsg` in place of `WELCOME`, and the connection ends. `admitted`
/// is called once the `WELCOME` is sent, which is when the connection counts as connected.
///
/// # Errors
///
/// [`NetError`] for a transport failure while reading the `HELLO` or inside the session.
pub fn serve_connection<S: SessionIo>(
    mut stream: S,
    role: PeerRole,
    certificate: Option<&rustls::pki_types::CertificateDer<'static>>,
    peer: Option<SocketAddr>,
    host: &Arc<ServeCore>,
    admitted: &dyn Fn(),
) -> Result<(), NetError> {
    let Some(certificate) = certificate else {
        // Cannot happen over TLS (the server demands a client certificate).
        return Ok(());
    };
    let fingerprint = fingerprint(certificate);
    let name = certificate_name(certificate, &fingerprint);
    let policy = AdminPolicy {
        enabled: host.info.remote_admin && host.control.remote_admin(),
        listed: is_listed(host, role, &fingerprint),
    };
    let check = handshake::read_and_check_hello_admin(&mut stream, Some(role), policy)?;
    match check {
        HelloCheck::Refused(refusal) => {
            handshake::send_refusal(&mut stream, &refusal);
            Ok(())
        }
        HelloCheck::Accepted(hello) if hello.role == role => {
            // The administrator needs the same protocol, not the same render build: the
            // `WELCOME` carries no build identity and no render capability.
            let welcome = Welcome::new(
                UNKNOWN_BUILD_HASH,
                UNKNOWN_BUILD_HASH,
                None,
                false,
                false,
                None,
                PayloadEncoding::Raw,
            );
            write_message(&mut stream, &welcome)?;
            // Admitted: listed as connected from here on.
            admitted();
            tracing::info!(
                "admin session: {name} ({}) connected as {} from {}",
                fingerprint_short(&fingerprint_to_hex(&fingerprint)),
                role_name(role),
                peer.map_or_else(|| "an unknown address".to_owned(), |p| p.ip().to_string()),
            );
            let result = run(&mut stream, role, fingerprint, &name, host);
            tracing::info!(
                "admin session: {name} ({}) disconnected",
                fingerprint_short(&fingerprint_to_hex(&fingerprint))
            );
            result
        }
        HelloCheck::Accepted(_) => Ok(()),
    }
}

/// Serves the requests of one admitted administrator until it disconnects.
///
/// `role` is `Monitor` or `Admin` (anything else ends at once); `fingerprint` and `name` name
/// the device in the audit trail. Ends `Ok` when the peer closes the connection, the idle
/// rule applies, or the connection is closed under it (the device was removed).
///
/// # Errors
///
/// [`NetError`] for a transport failure, a frame over the control limit or bytes that are no
/// request.
pub fn run<S: SessionIo>(
    stream: &mut S,
    role: PeerRole,
    fingerprint: Fingerprint,
    name: &str,
    host: &Arc<ServeCore>,
) -> Result<(), NetError> {
    let Some(access) = access_role_for(role) else {
        return Ok(());
    };
    // Best effort, like every socket tuning: without it a stalled peer is found by keepalive.
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    Session {
        stream,
        timeouts: ReadTimeout::default(),
        access,
        who: format!(
            "{name} ({})",
            fingerprint_short(&fingerprint_to_hex(&fingerprint))
        ),
        fingerprint,
        host,
        push: None,
        last_request: Instant::now(),
    }
    .serve()
}

/// What `Subscribe` switches on.
struct Push {
    /// The least time between two status pushes.
    every: Duration,
    /// When the next status is due.
    next_status: Instant,
    /// The last status sent, without the clock-driven fields (see [`stable_view`]).
    sent: Option<CoordinatorStatus>,
    /// The log level asked for; `None` sends no log lines.
    level: Option<LogLevel>,
    /// Log lines from before the subscription, sent first.
    backlog: VecDeque<LogLine>,
    /// Coordinator events from the subscription on.
    events: Receiver<CoordinatorEvent>,
    /// Log lines from the subscription on.
    logs: Option<Receiver<LogLine>>,
    /// When the next heartbeat is due.
    next_heartbeat: Instant,
}

struct Session<'a, S> {
    stream: &'a mut S,
    timeouts: ReadTimeout,
    access: AccessRole,
    /// "Laptop (ab12cd34)": who the audit trail names.
    who: String,
    fingerprint: Fingerprint,
    host: &'a Arc<ServeCore>,
    push: Option<Push>,
    last_request: Instant,
}

/// The status without what changes with the clock alone (a device's "last seen" is the moment
/// of the snapshot while it is online), so an idle coordinator pushes nothing.
fn stable_view(status: &CoordinatorStatus) -> CoordinatorStatus {
    let mut view = status.clone();
    for device in &mut view.devices {
        device.last_seen = None;
    }
    view
}

/// An operation that could not be done, as a reply (the sentence is plain words).
fn failed(message: String) -> AdminError {
    AdminError::new(error_codes::UNSUPPORTED_REQUEST, message)
}

/// Whether the control API itself publishes the audit event for `request` (the job and
/// render-computer calls know the label the event shows).
const fn publishes_its_own_audit(request: &AdminRequest) -> bool {
    matches!(
        request,
        AdminRequest::CancelJob { .. }
            | AdminRequest::DeleteJob { .. }
            | AdminRequest::DisconnectWorker { .. }
    )
}

impl<S: SessionIo> Session<'_, S> {
    fn serve(&mut self) -> Result<(), NetError> {
        loop {
            match poll_message::<S, AdminRequest>(self.stream, POLL, &mut self.timeouts)? {
                Poll::Closed => return Ok(()),
                Poll::Message(request) => {
                    self.last_request = Instant::now();
                    self.handle(&request)?;
                }
                Poll::Pending => {}
            }
            if self.push.is_some() {
                self.pump()?;
            }
            if idle_close_applies(self.push.is_some())
                && self.last_request.elapsed() >= IDLE_READ_TIMEOUT
            {
                tracing::info!(
                    "admin session: closing {}, which sent nothing for {} s (idle timeout)",
                    self.who,
                    IDLE_READ_TIMEOUT.as_secs()
                );
                return Ok(());
            }
        }
    }

    fn send(&mut self, push: &AdminPush) -> Result<(), NetError> {
        write_message(&mut *self.stream, push)
    }

    /// Answers one request.
    fn handle(&mut self, request: &AdminRequest) -> Result<(), NetError> {
        let req_id = request.req_id();
        if let Err(denied) = permissions::allowed(self.access, request) {
            tracing::warn!(
                "admin request refused: {} may not {}",
                self.who,
                request.action()
            );
            return self.send(&AdminPush::err(req_id, denied));
        }
        if let Err(reason) = validate::request(request) {
            return self.send(&AdminPush::err(req_id, AdminError::invalid(reason)));
        }
        match request {
            AdminRequest::Subscribe {
                status_ms, level, ..
            } => {
                self.subscribe(*status_ms, *level);
                self.send(&AdminPush::ok(req_id, AdminReply::Done))
            }
            AdminRequest::Status { .. } => {
                let status = presented(self.access, status::status(self.host));
                self.send(&AdminPush::ok(req_id, AdminReply::Status(status)))
            }
            AdminRequest::FetchResult { job, .. } => self.fetch(req_id, *job),
            _ => {
                let answer = self.command(request);
                self.send(&match answer {
                    Ok(reply) => AdminPush::ok(req_id, reply),
                    Err(error) => AdminPush::err(req_id, error),
                })
            }
        }
    }

    /// Does one command and, when it worked, records it.
    fn command(&self, request: &AdminRequest) -> Result<AdminReply, AdminError> {
        // Looked up first: a removed device's name is gone once the access is.
        let target = self.target_of(request);
        let reply = self.act(request)?;
        self.audit(request, &target);
        Ok(reply)
    }

    /// The action itself, through the control API.
    fn act(&self, request: &AdminRequest) -> Result<AdminReply, AdminError> {
        let host = self.host;
        let done = |result: Result<(), String>| result.map(|()| AdminReply::Done).map_err(failed);
        match request {
            AdminRequest::IssueToken { role, name, .. } => {
                // Refused by the permission table already; refused again here, because a code
                // for a monitor or an administrator must never be made over the network.
                if matches!(role, PeerRole::Monitor | PeerRole::Admin) {
                    return Err(never_remote_denial(NeverRemote::PrivilegedToken));
                }
                host.issue_token(*role, name)
                    .map(AdminReply::Token)
                    .map_err(failed)
            }
            AdminRequest::CancelToken { id, .. } => {
                host.cancel_token(*id);
                Ok(AdminReply::Done)
            }
            AdminRequest::Revoke { fingerprint, .. } => {
                let fp = parse_fingerprint(fingerprint)?;
                if fp == self.fingerprint {
                    return Err(AdminError::invalid(NOT_YOURSELF));
                }
                done(host.revoke(&fp))
            }
            AdminRequest::Rename {
                fingerprint, name, ..
            } => {
                let fp = parse_fingerprint(fingerprint)?;
                done(host.rename(&fp, name))
            }
            AdminRequest::CancelJob { id, .. } => done(host.cancel_job_as(*id, &self.who)),
            AdminRequest::DeleteJob { id, .. } => done(host.delete_job_as(*id, &self.who)),
            AdminRequest::DisconnectWorker { id, .. } => {
                done(host.disconnect_worker_as(*id, &self.who))
            }
            AdminRequest::ApplySettings { patch, .. } => done(apply_settings(host, patch)),
            AdminRequest::Restart { .. } => done(restart::schedule()),
            AdminRequest::Subscribe { .. }
            | AdminRequest::Status { .. }
            | AdminRequest::FetchResult { .. } => Err(AdminError::invalid(
                "That request does not change anything.",
            )),
        }
    }

    /// What the audit line names as the target of `request`, in plain words.
    fn target_of(&self, request: &AdminRequest) -> String {
        match request {
            AdminRequest::IssueToken { role, name, .. } => {
                format!("{} code for {}", role_name(*role), clean(name))
            }
            AdminRequest::CancelToken { id, .. } => format!("pairing code {}", id.0),
            AdminRequest::Revoke { fingerprint, .. } => self.device_label(fingerprint),
            AdminRequest::Rename {
                fingerprint, name, ..
            } => format!("{} to {}", self.device_label(fingerprint), clean(name)),
            AdminRequest::CancelJob { id, .. } | AdminRequest::DeleteJob { id, .. } => {
                format!("job {}", fingerprint_short(&id.to_hex()))
            }
            AdminRequest::DisconnectWorker { id, .. } => format!("render computer {}", id.0),
            AdminRequest::ApplySettings { .. } => "settings".to_owned(),
            AdminRequest::Restart { .. } => "the coordinator".to_owned(),
            AdminRequest::Subscribe { .. }
            | AdminRequest::Status { .. }
            | AdminRequest::FetchResult { .. } => String::new(),
        }
    }

    /// A device as the audit line shows it: its name, or its short fingerprint.
    fn device_label(&self, fingerprint_hex: &str) -> String {
        let hex = fingerprint_hex.trim().to_ascii_lowercase();
        self.host
            .control
            .device_names()
            .get(&hex)
            .map_or_else(|| fingerprint_short(&hex), |name| clean(name))
    }

    /// Records a command that worked: the log line and, for every subscriber, the event.
    fn audit(&self, request: &AdminRequest, target: &str) {
        let action = request.action();
        tracing::info!("admin action: {} {action} {target}", self.who);
        if !publishes_its_own_audit(request) {
            self.host
                .control
                .hub()
                .publish(&CoordinatorEvent::AdminAction {
                    who: self.who.clone(),
                    action: action.to_owned(),
                    target: target.to_owned(),
                });
        }
    }

    /// Switches the push stream on (or replaces it).
    ///
    /// A monitor gets no log lines at all, whatever level it asks for: a log line can hold a
    /// local folder path or a device name, and a monitor sees the status without such paths
    /// (`CoordinatorStatus::for_monitor`). An administrator gets the log as asked.
    fn subscribe(&mut self, status_ms: u32, level: Option<LogLevel>) {
        let now = Instant::now();
        let level = level.filter(|_| self.access != AccessRole::Monitor);
        let (backlog, logs) = level.map_or((Vec::new(), None), |level| {
            let (backlog, receiver) =
                settings::log::subscribe_with_backlog(level.into(), LOG_BACKLOG);
            (backlog, Some(receiver))
        });
        self.push = Some(Push {
            every: Duration::from_millis(u64::from(status_ms.max(MIN_STATUS_INTERVAL_MS))),
            // The first status goes out with the next round.
            next_status: now,
            sent: None,
            level,
            backlog: backlog.into(),
            events: self.host.events(),
            logs,
            next_heartbeat: now + HEARTBEAT_INTERVAL,
        });
    }

    /// Sends what is due: logs, events, the status when it changed, the heartbeat.
    fn pump(&mut self) -> Result<(), NetError> {
        let host = self.host;
        let access = self.access;
        let mut out: Vec<AdminPush> = Vec::new();
        {
            let Some(push) = self.push.as_mut() else {
                return Ok(());
            };
            while let Some(line) = push.backlog.pop_front() {
                out.push(AdminPush::Log(line));
            }
            for _ in 0..PUSH_BURST {
                let Ok(event) = push.events.try_recv() else {
                    break;
                };
                out.push(AdminPush::Event(event));
            }
            if let Some(logs) = &push.logs {
                for _ in 0..PUSH_BURST {
                    let Ok(line) = logs.try_recv() else {
                        break;
                    };
                    if push.level.is_none_or(|min| line.level.passes(min)) {
                        out.push(AdminPush::Log(line));
                    }
                }
            }
            let now = Instant::now();
            if now >= push.next_status {
                push.next_status = now + push.every;
                let status = presented(access, status::status(host));
                let view = stable_view(&status);
                if push.sent.as_ref() != Some(&view) {
                    push.sent = Some(view);
                    out.push(AdminPush::Status(status));
                }
            }
            if now >= push.next_heartbeat {
                push.next_heartbeat = now + HEARTBEAT_INTERVAL;
                out.push(AdminPush::Heartbeat);
            }
        }
        for push in &out {
            self.send(push)?;
        }
        Ok(())
    }

    /// Streams a finished job's picture.
    fn fetch(&mut self, req_id: u64, job: JobId) -> Result<(), NetError> {
        let png = match results::load(self.host, job) {
            Ok(png) => png,
            Err(message) => return self.send(&AdminPush::err(req_id, failed(message))),
        };
        tracing::info!(
            "admin session: {} fetched the picture of job {}",
            self.who,
            fingerprint_short(&job.to_hex())
        );
        for reply in results::chunk_replies(job, &png) {
            self.send(&AdminPush::ok(req_id, reply))?;
        }
        Ok(())
    }
}

/// Applies a settings change from a remote administrator.
///
/// With a settings file loaded it is validated and saved like the window does; the whole-image
/// knobs and the jobs per device take effect at once, the rest at the next restart. A headless
/// coordinator (no settings file) has nothing to save to: a patch that sets only the knobs a
/// running coordinator can take is applied live (and answers Ok whether or not a value
/// changed), and a patch that sets anything else is refused whole, in plain words, so nothing
/// is half applied.
///
/// A saved or live change is announced as a `SettingsChanged` event. The window's backend
/// announces saved changes itself through the settings hook (and owns the restart hook), so
/// this announces only when no window is there.
fn apply_settings(host: &ServeCore, patch: &SettingsPatch) -> Result<(), String> {
    if settings::settings_file().is_none() {
        if patch.sets_restart_fields() {
            return Err(NO_SETTINGS_FILE.to_owned());
        }
        if host.apply_live_checked(patch)? {
            announce_settings_changed(host, "performance".to_owned());
        }
        return Ok(());
    }
    settings::apply_patch(patch)?;
    let _ = host.apply_live(patch);
    if !patch.is_empty() && !restart::available() {
        announce_settings_changed(host, settings::last_change_summary());
    }
    Ok(())
}

/// What a coordinator without a settings file answers to a patch it cannot apply live.
const NO_SETTINGS_FILE: &str = "This coordinator runs without a settings file, so only the whole-picture limits and the jobs per device can be changed from here. Restart it with new options for the rest.";

/// Tells every subscriber that the settings changed.
fn announce_settings_changed(host: &ServeCore, summary: String) {
    host.control
        .hub()
        .publish(&CoordinatorEvent::SettingsChanged { summary });
}

/// The status as `access` may see it: a monitor gets no local folder paths.
fn presented(access: AccessRole, status: CoordinatorStatus) -> CoordinatorStatus {
    if access == AccessRole::Monitor {
        status.for_monitor()
    } else {
        status
    }
}

/// A device fingerprint as 64 hex characters, or the validation error naming it.
fn parse_fingerprint(hex: &str) -> Result<Fingerprint, AdminError> {
    fingerprint_from_hex(hex)
        .ok_or_else(|| AdminError::invalid("A device fingerprint is 64 hexadecimal characters."))
}

/// `text` without control characters, for the audit line.
fn clean(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_net::admin::status::{CoordinatorState, DeviceStatus};

    #[test]
    fn the_stable_view_ignores_the_clock_and_nothing_else() {
        let mut status =
            CoordinatorStatus::minimal("Studio", "0.12.0", 45, CoordinatorState::Running);
        let mut device = DeviceStatus::new("Laptop", "ab".repeat(32), PeerRole::Admin);
        device.online = true;
        device.last_seen = Some(100);
        status.devices.push(device);

        let first = stable_view(&status);
        status.devices[0].last_seen = Some(101);
        assert_eq!(
            stable_view(&status),
            first,
            "a later last-seen is no change"
        );
        status.devices[0].online = false;
        assert_ne!(stable_view(&status), first, "a device going offline is");
        status.warnings.push("Encryption is off".to_owned());
        assert_ne!(stable_view(&status), first);
    }

    #[test]
    fn the_job_calls_publish_their_own_audit_event_and_the_rest_do_not() {
        let job = JobId([1; 16]);
        assert!(publishes_its_own_audit(&AdminRequest::CancelJob {
            req_id: 1,
            id: job
        }));
        assert!(publishes_its_own_audit(&AdminRequest::DeleteJob {
            req_id: 1,
            id: job
        }));
        assert!(!publishes_its_own_audit(&AdminRequest::Restart {
            req_id: 1
        }));
        assert!(!publishes_its_own_audit(&AdminRequest::Revoke {
            req_id: 1,
            fingerprint: "ab".repeat(32)
        }));
    }

    #[test]
    fn a_fingerprint_must_be_sixty_four_hex_characters() {
        assert!(parse_fingerprint(&"ab".repeat(32)).is_ok());
        assert!(parse_fingerprint("abc").is_err());
        let error = parse_fingerprint(&"zz".repeat(32)).unwrap_err();
        assert_eq!(error.code, error_codes::VALIDATION_FAILED);
    }

    #[test]
    fn names_in_the_audit_line_lose_control_characters() {
        assert_eq!(clean("Wall\r\nPC"), "WallPC");
    }
}
