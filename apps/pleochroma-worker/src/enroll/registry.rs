//! The pending-entry registry: mint, hash, constant-time compare, TTL sweep and cap. The
//! security rationale is in the `crate::enroll` module doc.

use crate::pki;
use pleochroma_net::{messages::PeerRole, tls::Fingerprint, token};
use std::{
    fmt,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use super::{MAX_PENDING, TOKEN_TTL_SECS};

/// Why issuing a pending enrollment failed (see [`EnrollRegistry::issue`]).
///
/// The `Issue` arm of [`super::connection::handle_enroll_connection`] sends the
/// [`Display`](fmt::Display) text to the peer as the `reason` of
/// [`pleochroma_net::enroll::EnrollResponse::IssueRefused`]. Other callers match on the variant.
#[derive(Debug)]
pub enum EnrollIssueError {
    /// Minting the client-certificate bundle failed -- see
    /// [`pki::issue_client_in_memory`] and [`pki::PkiError`].
    Pki(Box<pki::PkiError>),
    /// The system CSPRNG ([`random_bytes`]) failed to produce randomness -- extremely
    /// rare.
    Csprng,
    /// [`MAX_PENDING`] enrollments are already pending.
    TooManyPending,
    /// Enrollment for this role is not available here: a monitor or admin certificate needs
    /// remote administration to be switched on (see [`EnrollRegistry::register_allowlist`]).
    UnsupportedRole(PeerRole),
}

impl fmt::Display for EnrollIssueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pki(e) => write!(f, "{e}"),
            Self::UnsupportedRole(_) => write!(
                f,
                "enrollment for monitor and admin certificates needs remote administration to be \
                 switched on (not supported here)"
            ),
            Self::Csprng => write!(f, "system CSPRNG failed to produce randomness"),
            Self::TooManyPending => write!(
                f,
                "{MAX_PENDING} enrollments are already pending -- claim or wait for one to expire before issuing \
                 another"
            ),
        }
    }
}

impl std::error::Error for EnrollIssueError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Pki(e) => Some(e.as_ref()),
            Self::Csprng | Self::TooManyPending | Self::UnsupportedRole(_) => None,
        }
    }
}

impl From<pki::PkiError> for EnrollIssueError {
    fn from(e: pki::PkiError) -> Self {
        Self::Pki(Box::new(e))
    }
}

// Wire protocol: `pleochroma_net::enroll`, shared with `apps/pleochroma-cut`.

/// A freshly minted client-certificate bundle held in memory for one pending enrollment.
/// `client_key_pem` is wrapped in [`Zeroizing`] so the key bytes are wiped on drop.
pub(super) struct EnrollBundle {
    pub(super) ca_pem: String,
    pub(super) client_cert_pem: String,
    pub(super) client_key_pem: Zeroizing<String>,
    pub(super) client_fingerprint: Fingerprint,
}

impl From<pki::InMemoryClientBundle> for EnrollBundle {
    fn from(b: pki::InMemoryClientBundle) -> Self {
        Self {
            ca_pem: b.ca_pem,
            client_cert_pem: b.client_cert_pem,
            client_key_pem: Zeroizing::new(b.client_key_pem),
            client_fingerprint: b.client_fingerprint,
        }
    }
}

/// Names one pending enrollment, so it can be cancelled with [`EnrollRegistry::remove`]
/// and matched to the [`ClaimEvent`] it later produces.
///
/// A plain counter value, unrelated to the secret: it is safe to show, log and keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TokenId(pub u64);

/// What [`EnrollRegistry::issue_for_role`] returns: the encoded token, and the id that
/// cancels it. `Debug` hides the token.
pub struct IssuedEnrollment {
    /// Cancels this enrollment with [`EnrollRegistry::remove`].
    pub id: TokenId,
    /// The encoded `GW1-...` token. A bearer secret: never log or print it.
    pub token: String,
    /// The role the claimed certificate gets.
    pub role: PeerRole,
    /// The token's lifetime in seconds, counted from the moment it was issued.
    pub expires_in_secs: u64,
}

impl fmt::Debug for IssuedEnrollment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IssuedEnrollment")
            .field("id", &self.id)
            .field("token", &format_args!("<redacted>"))
            .field("role", &self.role)
            .field("expires_in_secs", &self.expires_in_secs)
            .finish_non_exhaustive()
    }
}

/// One successful claim, sent to every [`EnrollRegistry::subscribe`] receiver so a screen
/// can say "Paired from <address>". Carries no secret: the fingerprint is public.
#[derive(Debug, Clone)]
pub struct ClaimEvent {
    /// The token that was claimed.
    pub id: TokenId,
    /// The role of the certificate that was handed out.
    pub role: PeerRole,
    /// The certificate's allowlist label (its Common Name).
    pub name: String,
    /// SHA-256 fingerprint of the issued client certificate (what the allowlist holds).
    pub fingerprint: Fingerprint,
    /// The address the claim came from. `0.0.0.0:0` if the socket could not report one.
    pub peer: SocketAddr,
}

pub(super) struct PendingEnrollment {
    id: TokenId,
    role: PeerRole,
    /// `SHA-256(secret)`, never the secret itself.
    secret_hash: Zeroizing<[u8; 32]>,
    expires_at: Instant,
    name: String,
    bundle: EnrollBundle,
}

/// The result of a successful [`EnrollRegistry::claim`]: everything
/// [`handle_enroll_connection`] needs to both reply to the claiming client and append to
/// `allowlist.txt`.
pub(super) struct ClaimedEnrollment {
    pub(super) id: TokenId,
    pub(super) role: PeerRole,
    pub(super) name: String,
    pub(super) bundle: EnrollBundle,
}

/// All unclaimed, unexpired enrollments of one enrollment listener of this `serve`.
///
/// One registry per listener, so a token only ever claims where it was issued. It lives as
/// long as one `serve` process: a restart drops every pending token, by design.
///
/// Each pending token has its own role, chosen when it is issued
/// ([`Self::issue_for_role`]); the registry's own role is only what the plain
/// [`Self::issue`] uses. A claim sends a [`ClaimEvent`] to every [`Self::subscribe`] receiver.
/// There is no `Debug`: the table holds hashes and key material.
pub struct EnrollRegistry {
    pub(super) pending: Mutex<Vec<PendingEnrollment>>,
    role: PeerRole,
    /// The secret an `Issue` request must present (`<pki_dir>/issue.secret`). `None`
    /// refuses every `Issue`: a registry nobody configured a secret for issues nothing.
    operator_secret: Option<Zeroizing<[u8; token::SECRET_LEN]>>,
    next_id: AtomicU64,
    subscribers: Mutex<Vec<mpsc::Sender<ClaimEvent>>>,
    /// Allowlists for roles other than [`Self::role`], see [`Self::register_allowlist`].
    allowlists: Mutex<Vec<(PeerRole, PathBuf)>>,
}

impl Default for EnrollRegistry {
    fn default() -> Self {
        Self::for_role(PeerRole::Viewer)
    }
}

impl EnrollRegistry {
    /// A VIEWER enrollment registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An enrollment registry minting certificates of `role`.
    #[must_use]
    pub const fn for_role(role: PeerRole) -> Self {
        Self {
            pending: Mutex::new(Vec::new()),
            role,
            operator_secret: None,
            next_id: AtomicU64::new(1),
            subscribers: Mutex::new(Vec::new()),
            allowlists: Mutex::new(Vec::new()),
        }
    }

    /// A receiver that gets one [`ClaimEvent`] for every enrollment claimed from now on.
    ///
    /// Each call makes a new, independent channel; dropping the receiver unsubscribes it.
    /// Events are sent after the claimed fingerprint was appended to the allowlist, right
    /// before the certificate goes back to the claiming device.
    #[must_use]
    pub fn subscribe(&self) -> mpsc::Receiver<ClaimEvent> {
        let (tx, rx) = mpsc::channel();
        self.lock_subscribers().push(tx);
        rx
    }

    /// Cancels the pending enrollment `id`, wiping its key material. `true` if it was still
    /// pending; `false` if it was already claimed, expired or removed.
    pub fn remove(&self, id: TokenId) -> bool {
        let mut pending = self.lock_pending();
        let before = pending.len();
        pending.retain(|p| p.id != id);
        pending.len() != before
    }

    /// Tells a claim of `role` to append its fingerprint to `path`, for roles other than the
    /// registry's own (whose path the connection handler is given directly).
    ///
    /// A claim of such a role with no registered path is refused rather than handed an unlisted
    /// certificate.
    pub fn register_allowlist(&self, role: PeerRole, path: PathBuf) {
        let mut lists = self
            .allowlists
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        lists.retain(|(r, _)| *r != role);
        lists.push((role, path));
    }

    /// The path registered for `role` by [`Self::register_allowlist`].
    pub(super) fn allowlist_for(&self, role: PeerRole) -> Option<PathBuf> {
        let lists = self
            .allowlists
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        lists
            .iter()
            .find(|(r, _)| *r == role)
            .map(|(_, p)| p.clone())
    }

    /// Sends `event` to every live subscriber and forgets the ones that went away.
    pub(super) fn notify_claimed(&self, event: &ClaimEvent) {
        self.lock_subscribers()
            .retain(|tx| tx.send(event.clone()).is_ok());
    }

    fn lock_subscribers(&self) -> MutexGuard<'_, Vec<mpsc::Sender<ClaimEvent>>> {
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// This registry, authorising `Issue` requests that present `secret`.
    #[must_use]
    pub fn with_operator_secret(mut self, secret: Zeroizing<[u8; token::SECRET_LEN]>) -> Self {
        self.operator_secret = Some(secret);
        self
    }

    /// Whether `presented` is the operator secret, compared in constant time. `false`
    /// when no operator secret is configured.
    #[must_use]
    pub fn operator_secret_matches(&self, presented: &[u8; token::SECRET_LEN]) -> bool {
        self.operator_secret
            .as_ref()
            .is_some_and(|secret| bool::from(secret.ct_eq(presented)))
    }

    /// The pending table, tolerating a poisoned lock: a panic in one enrollment
    /// connection must not disable enrollment for every other.
    fn lock_pending(&self) -> MutexGuard<'_, Vec<PendingEnrollment>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The certificate role this registry mints.
    #[must_use]
    pub const fn role(&self) -> PeerRole {
        self.role
    }

    /// Mints a client-certificate bundle of this registry's role for `name`.
    ///
    /// `name` is the bare label (a worker certificate's Common Name gets the `worker:` prefix).
    /// The bundle is registered as pending under a fresh 256-bit CSPRNG secret. Returns the encoded token and its TTL in
    /// seconds.
    ///
    /// # Errors
    ///
    /// See [`EnrollIssueError`]'s variants: too many enrollments already pending,
    /// certificate minting failed, or the CSPRNG failed (extremely rare).
    pub fn issue(&self, pki_dir: &Path, name: &str) -> Result<(String, u64), EnrollIssueError> {
        self.issue_with_ttl(pki_dir, name, Duration::from_secs(TOKEN_TTL_SECS))
    }

    /// The TTL-parameterized implementation behind [`Self::issue`], so tests can use a short
    /// TTL. Every real caller uses [`TOKEN_TTL_SECS`].
    ///
    /// `name` is sanitized with [`pleochroma_net::tls::sanitize_allowlist_label`] first. The
    /// certificate's Common Name and the allowlist line appended on claim then carry the same
    /// string, and a `\r`, `\n` or `#` in an operator-supplied `--name` cannot inject a stray
    /// allowlist line.
    pub(super) fn issue_with_ttl(
        &self,
        pki_dir: &Path,
        name: &str,
        ttl: Duration,
    ) -> Result<(String, u64), EnrollIssueError> {
        let IssuedEnrollment {
            token,
            expires_in_secs,
            ..
        } = self.issue_role_with_ttl(pki_dir, name, self.role, ttl)?;
        Ok((token, expires_in_secs))
    }

    /// Like [`Self::issue`], for a certificate of `role` instead of the registry's own: the role
    /// belongs to this one token.
    ///
    /// `name` is the bare label (a worker certificate's Common Name gets the `worker:` prefix).
    ///
    /// # Errors
    ///
    /// As [`Self::issue`]; also [`EnrollIssueError::UnsupportedRole`] for the monitor and
    /// admin roles while no allowlist is registered for them with
    /// [`Self::register_allowlist`] (remote administration is off): a claim of such a token
    /// could not be listed, so none is issued.
    pub fn issue_for_role(
        &self,
        pki_dir: &Path,
        name: &str,
        role: PeerRole,
    ) -> Result<IssuedEnrollment, EnrollIssueError> {
        self.issue_role_with_ttl(pki_dir, name, role, Duration::from_secs(TOKEN_TTL_SECS))
    }

    /// The implementation behind [`Self::issue_for_role`] and [`Self::issue_with_ttl`].
    pub(super) fn issue_role_with_ttl(
        &self,
        pki_dir: &Path,
        name: &str,
        role: PeerRole,
        ttl: Duration,
    ) -> Result<IssuedEnrollment, EnrollIssueError> {
        // Fail closed: a monitor or admin certificate is minted only where its claim can be
        // listed, which is exactly when remote administration registered its allowlist.
        if matches!(role, PeerRole::Monitor | PeerRole::Admin)
            && role != self.role
            && self.allowlist_for(role).is_none()
        {
            return Err(EnrollIssueError::UnsupportedRole(role));
        }
        let label = pleochroma_net::tls::sanitize_allowlist_label(name);
        let bundle = pki::issue_client_in_memory(pki_dir, &label, role)?;
        // The allowlist label is the Common Name (`worker:<name>` for a worker).
        let name = bundle.common_name.clone();

        let mut secret = [0u8; token::SECRET_LEN];
        random_bytes(&mut secret)?;
        let secret_hash = sha256(&secret);
        let encoded = token::encode(&secret, &bundle.ca_fingerprint);
        secret.zeroize_local();

        let mut pending = self.lock_pending();
        // Expired entries do not count against the cap.
        let now = Instant::now();
        pending.retain(|p| p.expires_at > now);
        if pending.len() >= MAX_PENDING {
            return Err(EnrollIssueError::TooManyPending);
        }
        let id = TokenId(self.next_id.fetch_add(1, Ordering::Relaxed));
        pending.push(PendingEnrollment {
            id,
            role,
            secret_hash: Zeroizing::new(secret_hash),
            expires_at: Instant::now() + ttl,
            name,
            bundle: EnrollBundle::from(bundle),
        });
        drop(pending);

        Ok(IssuedEnrollment {
            id,
            token: encoded,
            role,
            expires_in_secs: ttl.as_secs(),
        })
    }

    /// Attempts to claim the pending enrollment whose secret hashes to `secret`'s hash.
    ///
    /// First sweeps and drops every entry already past `expires_at`. Then checks the
    /// candidate's SHA-256 against every *remaining* entry using
    /// [`subtle::ConstantTimeEq::ct_eq`], never short-circuiting on the first match, so
    /// total comparison work doesn't leak which entry (if any) matched via timing. On a
    /// match, that entry is removed before being returned -- single use.
    ///
    /// Returns `None` for "expired", "wrong secret", and "no such token" alike -- the
    /// caller collapses all of these to the same [`EnrollResponse::ClaimFailed`].
    pub(super) fn claim(&self, secret: &[u8; token::SECRET_LEN]) -> Option<ClaimedEnrollment> {
        let candidate_hash = sha256(secret);
        let now = Instant::now();

        let mut pending = self.lock_pending();
        pending.retain(|p| p.expires_at > now); // expired entries dropped (and zeroized) here

        let mut matched_index = None;
        for (i, p) in pending.iter().enumerate() {
            let is_match: bool = candidate_hash.ct_eq(&*p.secret_hash).into();
            if is_match {
                matched_index = Some(i);
                // No `break`: every remaining entry is still compared (timing safety).
            }
        }

        let entry = pending.remove(matched_index?);
        drop(pending);
        Some(ClaimedEnrollment {
            id: entry.id,
            role: entry.role,
            name: entry.name,
            bundle: entry.bundle,
        })
    }
}

/// Fills `dest` with cryptographically secure random bytes via [`ring::rand::SystemRandom`].
///
/// # Errors
///
/// [`EnrollIssueError::Csprng`] in the (extremely rare, effectively "the OS RNG is
/// unavailable") case `ring` itself reports failure.
pub(super) fn random_bytes(dest: &mut [u8]) -> Result<(), EnrollIssueError> {
    use ring::rand::SecureRandom;
    ring::rand::SystemRandom::new()
        .fill(dest)
        .map_err(|_| EnrollIssueError::Csprng)
}

fn sha256(data: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(data).into()
}

/// Small local extension so a stack-allocated `[u8; N]` secret can be explicitly
/// zeroized in place without promoting it to a heap-allocated `Zeroizing<Vec<u8>>` just
/// for that one call.
pub(super) trait ZeroizeLocal {
    fn zeroize_local(&mut self);
}

impl<const N: usize> ZeroizeLocal for [u8; N] {
    fn zeroize_local(&mut self) {
        use zeroize::Zeroize;
        self.zeroize();
    }
}
