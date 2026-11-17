//! The remote of a recording run: an environment variable names a `settings.toml` (the owner's
//! own, read once and never written) whose `[settings.remote]` the live renders of the run use
//! next to this computer. The screenshots (`ui_screens`) and the tutorial videos
//! (`tutorial_videos`) read it the same way.

use super::SettingsSeed;
use std::{
    fs,
    net::{TcpStream, ToSocketAddrs},
    path::PathBuf,
    time::Duration,
};
use toml::{Table, Value};

/// The label the seeded remote carries, so that no shot or video shows the owner's own name for
/// it.
pub const NEUTRAL_REMOTE_NAME: &str = "Render Server";
/// How long the remote may take to accept a connection before the run stays on this computer.
const REACH_TIMEOUT: Duration = Duration::from_secs(3);

/// The remote of a run: the seed text and the words of it nothing on screen may show.
pub struct RemoteSeed {
    /// The whole `settings.toml` of a shot or video that renders with the remote.
    pub seed: String,
    /// The owner's label for the remote and its host, lower case.
    pub private_words: Vec<String>,
}

impl RemoteSeed {
    /// The settings a shot or video starts from: this remote, or the existing install when
    /// `local` (it shows the remote choices themselves).
    #[must_use]
    pub fn seed_for(remote: Option<&Self>, local: bool) -> SettingsSeed {
        match remote {
            Some(remote) if !local => SettingsSeed::Raw(remote.seed.clone()),
            _ => SettingsSeed::ExistingInstall,
        }
    }

    /// Adds the remote's private words to `words`, each once.
    pub fn extend_private_words(&self, words: &mut Vec<String>) {
        for word in &self.private_words {
            if !words.contains(word) {
                words.push(word.clone());
            }
        }
    }
}

/// Reads the settings file the environment variable `variable` names: `Ok(None)` when it is
/// unset, an error sentence when the file cannot be used or the remote does not accept a
/// connection within three seconds (the run then stays on this computer). The seed is the
/// existing install (update check declined, as [`SettingsSeed::ExistingInstall`]) with the file's
/// `[settings.remote]` under [`NEUTRAL_REMOTE_NAME`] and Live Compute on Local + Remote. Nothing
/// else of the owner's settings is copied.
///
/// # Errors
///
/// Why the remote is not used, as a sentence without the address.
pub fn remote_from_env(variable: &str) -> Result<Option<RemoteSeed>, String> {
    let Some(path) = std::env::var_os(variable)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    else {
        return Ok(None);
    };
    let text =
        fs::read_to_string(&path).map_err(|e| format!("the settings file cannot be read ({e})"))?;
    let root: Table = text
        .parse()
        .map_err(|e| format!("the settings file is not valid TOML ({e})"))?;
    let mut remote = root
        .get("settings")
        .and_then(Value::as_table)
        .and_then(|settings| settings.get("remote"))
        .and_then(Value::as_table)
        .cloned()
        .ok_or("the settings file has no [settings.remote]")?;
    let connection = remote
        .get_mut("connection")
        .and_then(Value::as_table_mut)
        .ok_or("the settings file has no [settings.remote.connection]")?;
    let mut private_words = Vec::new();
    let mut keep = |text: &str| {
        let word = text.trim().to_lowercase();
        if word.chars().count() >= 3 && !private_words.contains(&word) {
            private_words.push(word);
        }
    };
    if let Some(name) = connection.get("name").and_then(Value::as_str) {
        keep(name);
    }
    let address = connection
        .get("address")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|address| !address.is_empty())
        .ok_or("the remote has no address")?
        .to_owned();
    let host = address
        .rsplit_once(':')
        .map_or(address.as_str(), |(host, _port)| host)
        .trim_matches(['[', ']']);
    keep(host);
    if !reachable(&address) {
        return Err("the remote does not answer".to_owned());
    }
    connection.insert(
        "name".to_owned(),
        Value::String(NEUTRAL_REMOTE_NAME.to_owned()),
    );

    let mut update_check = Table::new();
    update_check.insert("enabled".to_owned(), Value::Boolean(false));
    let mut settings = Table::new();
    settings.insert("update_check".to_owned(), Value::Table(update_check));
    settings.insert("remote".to_owned(), Value::Table(remote));
    settings.insert(
        "live_compute_target".to_owned(),
        Value::String("Both".to_owned()),
    );
    let mut seed_root = Table::new();
    seed_root.insert("settings".to_owned(), Value::Table(settings));
    let seed = toml::to_string(&seed_root)
        .map_err(|e| format!("the remote cannot be written to a settings file ({e})"))?;
    Ok(Some(RemoteSeed {
        seed,
        private_words,
    }))
}

/// Whether `address` (`host:port`, an IPv6 host in brackets) accepts a TCP connection within
/// [`REACH_TIMEOUT`] on any of its resolved addresses.
fn reachable(address: &str) -> bool {
    address.to_socket_addrs().is_ok_and(|mut addrs| {
        addrs.any(|addr| TcpStream::connect_timeout(&addr, REACH_TIMEOUT).is_ok())
    })
}
