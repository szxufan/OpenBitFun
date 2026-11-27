use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Clone, Default, Deserialize, Serialize)]
pub(super) struct Settings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub grants: Vec<Grant>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Grant {
    pub id: Uuid,
    pub name: String,
    pub workspace_ids: BTreeSet<String>,
    pub origin: Option<String>,
    pub token_hash: String,
}

impl Grant {
    pub(super) fn accepts(&self, token: &str, origin: Option<&str>) -> bool {
        self.token_hash == digest(token.as_bytes())
            && origin.is_none_or(|origin| self.origin.as_deref() == Some(origin))
    }
}

pub(super) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(super) fn turn_id(grant: Uuid, key: &str) -> Uuid {
    let hash = Sha256::new()
        .chain_update(grant.as_bytes())
        .chain_update(key)
        .finalize();
    Uuid::from_bytes(hash[..16].try_into().expect("SHA256 has at least 16 bytes"))
}

/// Journal admission before handing work to the Runtime. An ambiguous handoff is
/// observable by its original IDs but is never submitted a second time.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Receipt {
    pub session_id: Uuid,
    pub turn_id: Uuid,
    pub workspace_id: String,
    pub fingerprint: String,
    pub admission: String,
    pub error: Option<String>,
    pub instance: Uuid,
}

pub(super) struct Store {
    root: PathBuf,
    _lock: fs::File,
}

impl Store {
    pub(super) fn open(root: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(root.join("host.lock"))
            .map_err(|e| e.to_string())?;
        lock.try_lock()
            .map_err(|e| format!("Cannot acquire desktop controller storage: {e}"))?;
        Ok(Self { root, _lock: lock })
    }

    pub(super) fn settings(&self) -> Result<Settings, String> {
        Ok(read(&self.root.join("settings.json"))?.unwrap_or_default())
    }

    pub(super) fn save_settings(&self, settings: &Settings) -> Result<(), String> {
        write(&self.root.join("settings.json"), settings)
    }

    fn receipt_path(&self, grant: Uuid, turn: Uuid) -> PathBuf {
        self.root
            .join(grant.to_string())
            .join(format!("{turn}.json"))
    }

    pub(super) fn receipt(&self, grant: Uuid, turn: Uuid) -> Result<Option<Receipt>, String> {
        read(&self.receipt_path(grant, turn))
    }

    pub(super) fn save_receipt(&self, grant: Uuid, receipt: &Receipt) -> Result<(), String> {
        write(&self.receipt_path(grant, receipt.turn_id), receipt)
    }
}

fn read<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    match fs::File::open(path) {
        Ok(file) => serde_json::from_reader(file)
            .map(Some)
            .map_err(|e| e.to_string()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

fn write(path: &Path, data: &impl Serialize) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or("Missing controller storage directory")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut file, data).map_err(|e| e.to_string())?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    file.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_control_grants_and_admission_survive_restart_and_stay_isolated() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path().to_owned()).unwrap();
        assert!(!store.settings().unwrap().enabled);
        let id = Uuid::new_v4();
        let grant = Grant {
            id,
            name: "Board".into(),
            workspace_ids: BTreeSet::from(["project".into()]),
            origin: Some("https://board.example".into()),
            token_hash: digest(b"one-time-token"),
        };
        assert!(grant.accepts("one-time-token", None));
        assert!(grant.accepts("one-time-token", Some("https://board.example")));
        assert!(!grant.accepts("one-time-token", Some("https://unrelated.example")));
        assert!(!grant.accepts("different-token", None));
        store
            .save_settings(&Settings {
                enabled: true,
                port: None,
                grants: vec![grant],
            })
            .unwrap();
        assert!(!fs::read_to_string(root.path().join("settings.json"))
            .unwrap()
            .contains("one-time-token"));
        let turn = turn_id(id, "issue-17");
        store
            .save_receipt(
                id,
                &Receipt {
                    session_id: turn,
                    turn_id: turn,
                    workspace_id: "project".into(),
                    fingerprint: "intent".into(),
                    admission: "outcome_unknown".into(),
                    error: None,
                    instance: Uuid::new_v4(),
                },
            )
            .unwrap();
        assert!(Store::open(root.path().to_owned()).is_err());
        drop(store);
        let reopened = Store::open(root.path().to_owned()).unwrap();
        assert!(reopened.settings().unwrap().enabled);
        assert_eq!(
            reopened
                .receipt(id, turn_id(id, "issue-17"))
                .unwrap()
                .unwrap()
                .turn_id,
            turn
        );
        let other = Uuid::new_v4();
        assert_ne!(turn_id(other, "issue-17"), turn);
        assert!(reopened.receipt(other, turn).unwrap().is_none());
    }

    #[test]
    fn external_control_settings_tolerate_legacy_shape_and_preserve_corrupt_records() {
        let settings: Settings =
            serde_json::from_str(r#"{"enabled":false,"future":true}"#).unwrap();
        let restored: Settings =
            serde_json::from_value(serde_json::to_value(&settings).unwrap()).unwrap();
        assert!(restored.grants.is_empty() && restored.port.is_none() && !restored.enabled);
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path().to_owned()).unwrap();
        let path = root.path().join("settings.json");
        fs::write(&path, "incomplete").unwrap();
        assert!(store.settings().is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "incomplete");
    }
}
