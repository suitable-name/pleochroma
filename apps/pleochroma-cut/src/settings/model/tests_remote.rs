//! Tests for the single-endpoint settings model and its transfer preferences (full data
//! or final picture only): the legacy multi-worker migration and the transfer-choice
//! defaults.

use super::{
    AppSettings, ExportTransfer, LiveTransfer, RemoteEndpoint, SettingsFile, WorkerSettings,
};

/// A settings file whose legacy worker list holds two entries.
const OLD_FILE_WITH_TWO_WORKERS: &str = r#"
[settings]
exposure = 1.3
live_compute_target = "RemoteOnly"

[[settings.remote_workers]]
name = "Office"
address = "office.lan:7878"
cert_dir = "C:/certs/office"
cadence_ms = 250

[[settings.remote_workers]]
name = "Cloud"
address = "cloud.example:7878"
cert_dir = "C:/certs/cloud"
"#;

#[test]
fn an_old_file_with_two_workers_keeps_the_first_as_the_remote_and_drops_the_second() {
    let mut file: SettingsFile = toml::from_str(OLD_FILE_WITH_TWO_WORKERS).expect("parse");
    assert_eq!(file.settings.legacy_remote_workers.len(), 2);
    assert_eq!(file.settings.remote, None);

    let migration = file
        .settings
        .migrate_legacy_remote_workers()
        .expect("there was a list to migrate");
    assert_eq!(migration.kept.as_deref(), Some("office.lan:7878"));
    assert_eq!(migration.dropped, vec!["cloud.example:7878".to_string()]);

    let remote = file.settings.remote.as_ref().expect("first entry kept");
    assert_eq!(remote.connection.name, "Office");
    assert_eq!(remote.connection.cert_dir, "C:/certs/office");
    assert_eq!(remote.connection.cadence_ms, 250);
    assert_eq!(remote.export_transfer, ExportTransfer::FullData);
    assert_eq!(remote.live_transfer, LiveTransfer::FullData);
    assert_eq!(file.settings.legacy_remote_workers.len(), 0);
    // Everything else in the file is untouched.
    assert!((file.settings.exposure - 1.3).abs() < 1e-6);

    // Idempotent, and the retired key is never written back.
    assert_eq!(file.settings.migrate_legacy_remote_workers(), None);
    let rewritten = toml::to_string_pretty(&file).expect("serialize");
    assert!(!rewritten.contains("remote_workers"), "{rewritten}");
    let reparsed: SettingsFile = toml::from_str(&rewritten).expect("reparse");
    assert_eq!(reparsed.settings.remote, file.settings.remote);
}

/// A file that somehow carries both an endpoint and a legacy list keeps the endpoint
/// and drops the whole list (logged), never overwriting the newer setting.
#[test]
fn a_configured_endpoint_wins_over_a_leftover_legacy_list() {
    let mut settings = AppSettings {
        remote: Some(RemoteEndpoint::new(WorkerSettings {
            address: "coordinator:7878".to_string(),
            ..WorkerSettings::default()
        })),
        legacy_remote_workers: vec![WorkerSettings {
            address: "old:7878".to_string(),
            ..WorkerSettings::default()
        }],
        ..AppSettings::default()
    };
    let migration = settings.migrate_legacy_remote_workers().expect("migrated");
    assert_eq!(migration.kept, None);
    assert_eq!(migration.dropped, vec!["old:7878".to_string()]);
    assert_eq!(
        settings.remote_worker().map(|w| w.address),
        Some("coordinator:7878".to_string())
    );
}

#[test]
fn nothing_to_migrate_reports_none() {
    assert_eq!(AppSettings::default().migrate_legacy_remote_workers(), None);
}

/// Both transfer preferences default to full data, the only transfer a plain worker supports;
/// the pill indices round-trip.
#[test]
fn transfer_choices_default_to_full_data_and_their_indices_round_trip() {
    let endpoint = RemoteEndpoint::default();
    assert_eq!(endpoint.export_transfer, ExportTransfer::FullData);
    assert_eq!(endpoint.live_transfer, LiveTransfer::FullData);
    for t in [ExportTransfer::FullData, ExportTransfer::FinalPicture] {
        assert_eq!(ExportTransfer::from_index(t.index()), t);
    }
    for t in [LiveTransfer::FullData, LiveTransfer::FinalPicture] {
        assert_eq!(LiveTransfer::from_index(t.index()), t);
    }
    assert_eq!(ExportTransfer::from_index(-1), ExportTransfer::FullData);
    assert_eq!(LiveTransfer::from_index(7), LiveTransfer::FullData);
}

/// A settings file written by 0.11.1: one `host:port`, no address list.
const FILE_FROM_0_11_1: &str = r#"
[settings.remote.connection]
name = "Studio PC"
address = "192.168.1.20:7878"
cert_dir = "C:/certs/studio"
"#;

/// The 0.11.1 single address loads as a one-element list. Nothing is rewritten on load: the
/// stored fields stay empty, and saving the file again adds no new key.
#[test]
fn a_single_address_from_0_11_1_loads_as_a_one_element_list() {
    let file: SettingsFile = toml::from_str(FILE_FROM_0_11_1).expect("parse");
    let remote = file.settings.remote.as_ref().expect("remote parsed");
    assert!(remote.addresses.is_empty());
    assert_eq!(remote.enroll_port, 0);
    assert!(
        !remote.fixed_name,
        "an old endpoint keeps verifying the dialed name"
    );
    assert_eq!(
        remote.endpoint_addresses(),
        vec!["192.168.1.20:7878".to_string()]
    );

    let rewritten = toml::to_string_pretty(&file).expect("serialize");
    assert!(!rewritten.contains("addresses"), "{rewritten}");
    assert!(!rewritten.contains("enroll_port"), "{rewritten}");
    assert!(!rewritten.contains("fixed_name"), "{rewritten}");
    let reparsed: SettingsFile = toml::from_str(&rewritten).expect("reparse");
    assert_eq!(reparsed, file);
}

/// An endpoint with nothing saved has no address, and blank list entries are skipped.
#[test]
fn the_address_list_skips_blanks_and_prefers_the_saved_list() {
    assert!(RemoteEndpoint::default().endpoint_addresses().is_empty());
    let endpoint = RemoteEndpoint {
        connection: WorkerSettings {
            address: "old:7878".to_string(),
            ..WorkerSettings::default()
        },
        addresses: vec![
            " 10.0.0.5:7878 ".to_string(),
            String::new(),
            "192.168.1.20:7878".to_string(),
        ],
        ..RemoteEndpoint::default()
    };
    assert_eq!(
        endpoint.endpoint_addresses(),
        vec!["10.0.0.5:7878".to_string(), "192.168.1.20:7878".to_string()]
    );
}

/// A paired endpoint keeps its address order, marks itself fixed-name, and survives a
/// settings round trip; the stream preferences it had before stay.
#[test]
fn a_paired_endpoint_round_trips_with_its_address_list() {
    let before = RemoteEndpoint {
        export_transfer: ExportTransfer::FinalPicture,
        ..RemoteEndpoint::new(WorkerSettings {
            name: "Garage".to_string(),
            address: "172.16.0.9:7878".to_string(),
            cert_dir: "C:/certs/garage".to_string(),
            cadence_ms: 250,
            ..WorkerSettings::default()
        })
    };
    let paired = before.paired(
        "Studio PC",
        vec!["192.168.1.20:7878".to_string(), "10.0.0.5:7878".to_string()],
        7879,
        "C:/certs/studio-pc",
    );
    assert_eq!(paired.connection.name, "Studio PC");
    assert_eq!(paired.connection.address, "192.168.1.20:7878");
    assert_eq!(paired.connection.cert_dir, "C:/certs/studio-pc");
    assert_eq!(paired.connection.cadence_ms, 250);
    assert_eq!(paired.export_transfer, ExportTransfer::FinalPicture);
    assert_eq!(paired.enroll_port, 7879);
    assert!(paired.fixed_name);
    assert_eq!(
        paired.endpoint_addresses(),
        vec!["192.168.1.20:7878".to_string(), "10.0.0.5:7878".to_string()]
    );

    let mut file = SettingsFile::default();
    file.settings.remote = Some(paired);
    let text = toml::to_string_pretty(&file).expect("serialize");
    let reparsed: SettingsFile = toml::from_str(&text).expect("reparse");
    assert_eq!(reparsed, file);
}

/// Rediscovery replaces only the addresses: the certificates, the name and the verification
/// mode stay.
#[test]
fn rediscovery_replaces_the_addresses_and_nothing_else() {
    let paired = RemoteEndpoint::default().paired(
        "Studio PC",
        vec!["192.168.1.20:7878".to_string()],
        7879,
        "C:/certs/studio-pc",
    );
    let moved = paired.moved_to(vec!["192.168.1.40:7878".to_string()]);
    assert_eq!(moved.connection.address, "192.168.1.40:7878");
    assert_eq!(
        moved.endpoint_addresses(),
        vec!["192.168.1.40:7878".to_string()]
    );
    assert_eq!(moved.connection.name, "Studio PC");
    assert_eq!(moved.connection.cert_dir, "C:/certs/studio-pc");
    assert_eq!(moved.enroll_port, 7879);
    assert!(moved.fixed_name);
    assert_eq!(moved.display_name(), "Studio PC");
    assert_eq!(
        RemoteEndpoint::default()
            .moved_to(vec!["a:1".to_string()])
            .display_name(),
        "a:1"
    );
}
