//! Pairing this viewer with a coordinator from a pairing link.
//!
//! The "Paste pairing link" part of the Remote Coordinator dialog. [`read_pairing_link`]
//! checks the pasted text and returns a [`PairOffer`] the dialog shows for confirmation
//! ("Pair with Studio PC?"); nothing connects before the user confirms. [`pair`] then redeems
//! the code over every address of the link at once, keeps the first coordinator that passes
//! the certificate-authority pin the code carries, and writes the certificate bundle the same
//! way the token flow does ([`super::enroll::write_bundle`]).
//!
//! [`pair`] is blocking (real connections): `gui::remote` runs it on a background thread.
//! Nothing here keeps the code after it was used, and no message names any part of it.

use super::enroll::{bundle_dir_for, write_bundle};
use pleochroma_net::{
    enroll::ClaimAnyError,
    messages::PeerRole,
    pairing::{PairingLink, PairingLinkError},
};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

/// A pairing link this viewer is willing to act on: a viewer code that parsed cleanly.
///
/// `Debug` is redacted like the link's, and the code is wiped on drop.
pub struct PairOffer {
    link: PairingLink,
}

impl std::fmt::Debug for PairOffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairOffer")
            .field("name", &self.link.name)
            .field("addrs", &self.link.addrs)
            .field("enroll_port", &self.link.enroll_port)
            .finish_non_exhaustive()
    }
}

impl PairOffer {
    /// The computer's name as the link spells it; "your computer" when it has none.
    #[must_use]
    pub fn name(&self) -> String {
        let name = self.link.name.trim();
        if name.is_empty() {
            "your computer".to_owned()
        } else {
            name.to_owned()
        }
    }

    /// The rendering addresses the link carries, as `host:port`, main network first.
    #[must_use]
    pub fn addresses(&self) -> Vec<String> {
        self.link.addrs.iter().map(ToString::to_string).collect()
    }

    /// One line for the confirm step: the first address and how many more there are.
    #[must_use]
    pub fn address_summary(&self) -> String {
        let all = self.addresses();
        match all.split_first() {
            None => String::new(),
            Some((first, [])) => first.clone(),
            Some((first, rest)) => format!("{first} (+{} more)", rest.len()),
        }
    }

    /// The port of the coordinator's enrollment listener.
    #[must_use]
    pub const fn enroll_port(&self) -> u16 {
        self.link.enroll_port
    }

    /// The enrollment listener's addresses: each link address with the enrollment port.
    fn enrollment_addresses(&self) -> Vec<SocketAddr> {
        self.link
            .addrs
            .iter()
            .map(|addr| SocketAddr::new(addr.ip(), self.link.enroll_port))
            .collect()
    }
}

/// Reads pasted text as a pairing link for this viewer.
///
/// Only a viewer code is accepted. A render computer's code and the Monitor and Admin codes
/// are refused in plain words, so the viewer never takes a role it cannot use.
///
/// # Errors
///
/// A plain sentence for the user when the text is not a pairing link, is damaged or too new,
/// or carries a code for something other than a viewer. It never repeats the link.
pub fn read_pairing_link(text: &str) -> Result<PairOffer, String> {
    let link = PairingLink::parse(text.trim()).map_err(|e| link_error_words(e).to_owned())?;
    match link.role {
        PeerRole::Viewer => Ok(PairOffer { link }),
        PeerRole::Worker => Err("This code is for a render computer. Use Help another \
             coordinator on that computer."
            .to_owned()),
        PeerRole::Monitor | PeerRole::Admin => Err("This code is for managing a computer, \
             which this app cannot do. On the computer, choose Connect a computer."
            .to_owned()),
    }
}

/// The plain words for a link that could not be read. None of them contains any of the link.
#[must_use]
pub const fn link_error_words(error: PairingLinkError) -> &'static str {
    match error {
        PairingLinkError::NotAPairingLink => {
            "This is not a Pleochroma pairing link. On the computer, choose Connect a \
             computer."
        }
        PairingLinkError::TooLong => "This pairing link is too long to be real.",
        PairingLinkError::NeedsNewerVersion => {
            "This link needs a newer Pleochroma. Update the app, then try again."
        }
        PairingLinkError::MissingPart | PairingLinkError::RepeatedPart => {
            "This pairing link is incomplete or damaged. Show the code on the computer again."
        }
        PairingLinkError::UnknownRole => {
            "This pairing link names an access level this app does not know."
        }
        PairingLinkError::BadName => "This pairing link has a computer name this app cannot use.",
        PairingLinkError::BadAddress | PairingLinkError::NotIpv4 => {
            "This pairing link has an address this app cannot use."
        }
        PairingLinkError::NoAddress => "This pairing link has no address to connect to.",
        PairingLinkError::TooManyAddresses => "This pairing link has too many addresses.",
        PairingLinkError::BadPort => "This pairing link has a port this app cannot use.",
        PairingLinkError::MissingToken => "This pairing link has no code in it.",
        PairingLinkError::BadToken | PairingLinkError::BadEncoding => {
            "This pairing link is damaged. Show the code on the computer again."
        }
    }
}

/// The plain words for a failed claim. None of them contains the code.
#[must_use]
pub fn claim_error_words(error: &ClaimAnyError) -> String {
    match error {
        ClaimAnyError::NoAddresses => "The pairing link has no address to connect to. Show the \
             code on the computer again."
            .to_owned(),
        ClaimAnyError::InvalidCode => "That is not a valid pairing code. On the computer, \
             choose Connect a computer."
            .to_owned(),
        ClaimAnyError::AllTimedOut => {
            "The computer did not answer. Are both computers on the same network?".to_owned()
        }
        ClaimAnyError::Refused => "The computer refused the connection. Is Pleochroma running \
             there, with Connect a computer open?"
            .to_owned(),
        ClaimAnyError::Unreachable => {
            "This computer cannot reach the other one. Are both on the same network?".to_owned()
        }
        ClaimAnyError::WrongCoordinator => "A different computer answered than the one this \
             code belongs to, so pairing was stopped."
            .to_owned(),
        ClaimAnyError::CodeExpiredOrUsed => "This code expired or was already used. If you \
             didn't use it, someone else may have paired \u{2014} check Devices on the \
             coordinator."
            .to_owned(),
        ClaimAnyError::Failed(reason) => format!("Pairing failed: {reason}"),
    }
}

/// What the settings remember about a coordinator this viewer paired with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paired {
    /// The coordinator's name.
    pub name: String,
    /// Its rendering addresses (`host:port`); the one that answered the claim is first.
    pub addresses: Vec<String>,
    /// The port of its enrollment listener.
    pub enroll_port: u16,
    /// The folder the certificate bundle was written to.
    pub cert_dir: PathBuf,
}

/// Redeems a confirmed offer over every address of its link at once and writes the bundle
/// under `settings_dir`.
///
/// Blocking. Nothing is written unless the claim succeeded, so a failed pairing leaves an
/// earlier bundle of the same name alone.
///
/// # Errors
///
/// A plain sentence ([`claim_error_words`]) for a refused or unreachable computer or a code
/// that expired or was already used, or a message naming the file that could not be written.
pub fn pair(offer: &PairOffer, settings_dir: &Path) -> Result<Paired, String> {
    let outcome =
        pleochroma_net::enroll::claim_any(offer.link.token(), &offer.enrollment_addresses())
            .map_err(|e| claim_error_words(&e))?;
    let cert_dir = write_bundle(
        &outcome.bundle,
        &bundle_dir_for(settings_dir, &offer.name()),
    )?;
    Ok(Paired {
        name: offer.name(),
        addresses: answered_first(offer.addresses(), outcome.addr),
        enroll_port: offer.enroll_port(),
        cert_dir,
    })
}

/// `addresses` with the one whose host answered the claim moved to the front (the rest keep
/// their order), so the next connection tries it first.
#[must_use]
pub fn answered_first(mut addresses: Vec<String>, answered: SocketAddr) -> Vec<String> {
    addresses.sort_by_key(|text| {
        !text
            .parse::<SocketAddr>()
            .is_ok_and(|addr| addr.ip() == answered.ip())
    });
    addresses
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "GW1-AbCdEf-0123456789";

    fn link_text(role: PeerRole, name: &str, addrs: &[&str]) -> String {
        PairingLink::new(
            role,
            name,
            addrs.iter().map(|a| a.parse().expect("address")).collect(),
            7879,
            TOKEN,
        )
        .build()
        .expect("a valid link")
    }

    #[test]
    fn a_viewer_link_becomes_an_offer_the_confirm_step_can_describe() {
        let text = link_text(
            PeerRole::Viewer,
            "Studio PC",
            &["192.168.178.20:7878", "10.0.0.5:7878"],
        );
        // Pasted text often carries a trailing newline or spaces.
        let offer = read_pairing_link(&format!("  {text}\n")).expect("a viewer link");
        assert_eq!(offer.name(), "Studio PC");
        assert_eq!(offer.addresses(), ["192.168.178.20:7878", "10.0.0.5:7878"]);
        assert_eq!(offer.address_summary(), "192.168.178.20:7878 (+1 more)");
        assert_eq!(offer.enroll_port(), 7879);

        let one = read_pairing_link(&link_text(PeerRole::Viewer, "", &["10.0.0.5:7878"]))
            .expect("a link without a name");
        assert_eq!(one.name(), "your computer");
        assert_eq!(one.address_summary(), "10.0.0.5:7878");
        // Enrollment listens on each link host at the enrollment port.
        assert_eq!(one.enrollment_addresses()[0].to_string(), "10.0.0.5:7879");
    }

    #[test]
    fn the_offer_never_shows_the_code() {
        let offer = read_pairing_link(&link_text(PeerRole::Viewer, "Studio", &["10.0.0.5:7878"]))
            .expect("a viewer link");
        assert!(!format!("{offer:?}").contains(TOKEN));
    }

    #[test]
    fn a_render_computer_code_says_where_it_belongs() {
        let text = link_text(PeerRole::Worker, "Render box", &["10.0.0.5:7878"]);
        let error = read_pairing_link(&text).expect_err("a worker code is not for a viewer");
        assert_eq!(
            error,
            "This code is for a render computer. Use Help another coordinator on that computer."
        );
        for role in [PeerRole::Monitor, PeerRole::Admin] {
            let text = link_text(role, "Studio", &["10.0.0.5:7878"]);
            let error = read_pairing_link(&text).expect_err("a viewer only pairs as a viewer");
            assert!(error.contains("cannot do"), "{error}");
            assert!(!error.contains(TOKEN));
        }
    }

    #[test]
    fn damaged_links_get_plain_words_that_hold_none_of_the_link() {
        for text in [
            "",
            "hello",
            "https://example.com/pair?t=GW1-x",
            "pleochroma://pair?v=1",
            "pleochroma://pair?v=9&r=viewer&a=10.0.0.5:7878&e=7879&t=GW1-AbCdEf-0123456789",
        ] {
            let error = read_pairing_link(text).expect_err("not a usable link");
            assert!(!error.is_empty());
            assert!(!error.contains("GW1-"), "{error}");
            assert!(!error.contains("pleochroma://"), "{error}");
        }
    }

    #[test]
    fn every_claim_error_has_its_own_plain_sentence_without_the_code() {
        let errors = [
            ClaimAnyError::NoAddresses,
            ClaimAnyError::InvalidCode,
            ClaimAnyError::AllTimedOut,
            ClaimAnyError::Refused,
            ClaimAnyError::Unreachable,
            ClaimAnyError::WrongCoordinator,
            ClaimAnyError::CodeExpiredOrUsed,
            ClaimAnyError::Failed("the connection broke".to_owned()),
        ];
        let words: Vec<String> = errors.iter().map(claim_error_words).collect();
        for (i, a) in words.iter().enumerate() {
            assert!(!a.is_empty());
            assert!(!a.contains("GW1-"), "{a}");
            for b in &words[i + 1..] {
                assert_ne!(a, b);
            }
        }
        assert_eq!(
            claim_error_words(&ClaimAnyError::CodeExpiredOrUsed),
            "This code expired or was already used. If you didn't use it, someone else may \
             have paired \u{2014} check Devices on the coordinator."
        );
    }

    #[test]
    fn the_address_that_answered_goes_first_and_the_rest_keep_their_order() {
        let list = vec![
            "192.168.1.20:7878".to_owned(),
            "10.0.0.5:7878".to_owned(),
            "172.16.0.9:7878".to_owned(),
        ];
        let answered: SocketAddr = "172.16.0.9:7879".parse().expect("address");
        assert_eq!(
            answered_first(list.clone(), answered),
            ["172.16.0.9:7878", "192.168.1.20:7878", "10.0.0.5:7878"]
        );
        let first: SocketAddr = "192.168.1.20:7879".parse().expect("address");
        assert_eq!(answered_first(list.clone(), first), list);
    }
}
