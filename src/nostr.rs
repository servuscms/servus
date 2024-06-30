use bitcoin_hashes::{sha256, Hash};
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use lazy_static::lazy_static;
use secp256k1::{schnorr, Secp256k1, VerifyOnly, XOnlyPublicKey};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use serde_yaml::Value as YamlValue;
use std::{
    collections::HashMap,
    ffi::OsStr,
    fs,
    fs::{File, OpenOptions},
    io::Write,
    path::Path,
    str::FromStr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tide::log;

pub struct InvalidEventError;

#[derive(Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub pubkey: String,
    pub created_at: i64,
    pub kind: i64,
    pub tags: Vec<Vec<String>>,
    pub content: String,
    pub sig: String,
}

pub const EVENT_KIND_NOTE: i64 = 1;
pub const EVENT_KIND_DELETE: i64 = 5;
pub const EVENT_KIND_BLOSSOM: i64 = 24242;
pub const EVENT_KIND_AUTH: i64 = 27235;
pub const EVENT_KIND_LONG_FORM: i64 = 30023;
pub const EVENT_KIND_LONG_FORM_DRAFT: i64 = 30024;
pub const EVENT_KIND_CUSTOM_DATA: i64 = 30078;

lazy_static! {
    pub static ref SECP: Secp256k1<VerifyOnly> = Secp256k1::verification_only();
}

impl Event {
    pub fn is_long_form(&self) -> bool {
        self.kind == EVENT_KIND_LONG_FORM || self.kind == EVENT_KIND_LONG_FORM_DRAFT
    }

    pub fn get_date(&self) -> NaiveDateTime {
        if self.is_long_form() {
            if let Some(published_at) = self.get_long_form_published_at() {
                return published_at;
            }
        }

        self.get_created_at_date().naive_utc()
    }

    pub fn get_created_at_date(&self) -> DateTime<Utc> {
        Utc.timestamp_opt(self.created_at, 0).unwrap()
    }

    pub fn get_tags_hash(&self) -> HashMap<String, String> {
        let mut tags: HashMap<String, String> = HashMap::new();
        for t in &self.tags {
            tags.insert(t[0].to_owned(), t[1].to_owned());
        }
        tags
    }

    fn get_tag(&self, tag: &str) -> Option<String> {
        self.get_tags_hash().get(tag).cloned()
    }

    pub fn get_d_tag(&self) -> Option<String> {
        self.get_tag("d")
    }

    pub fn get_long_form_summary(&self) -> Option<String> {
        if self.kind != EVENT_KIND_LONG_FORM && self.kind != EVENT_KIND_LONG_FORM_DRAFT {
            return None;
        }

        self.get_tag("summary")
    }

    pub fn get_long_form_published_at(&self) -> Option<NaiveDateTime> {
        if self.kind != EVENT_KIND_LONG_FORM && self.kind != EVENT_KIND_LONG_FORM_DRAFT {
            return None;
        }

        let ts = self.get_tag("published_at")?.parse::<i64>().unwrap();

        DateTime::from_timestamp(ts, 0).map(|d| d.naive_utc())
    }

    pub fn validate_sig(&self) -> Result<(), InvalidEventError> {
        let canonical = self.to_canonical();
        log::debug!("Event in canonical format: {}", &canonical);

        let hash = sha256::Hash::hash(canonical.as_bytes());
        let hex_hash = format!("{:x}", hash);
        log::debug!("Event id: {}", self.id);
        log::debug!("Computed event id: {}", hash);

        if self.id != hex_hash {
            return Err(InvalidEventError);
        }

        if let Ok(msg) = secp256k1::Message::from_slice(hash.as_ref()) {
            if let Ok(pubkey) = XOnlyPublicKey::from_str(&self.pubkey) {
                let sig = schnorr::Signature::from_str(&self.sig).unwrap();
                if SECP.verify_schnorr(&sig, &msg, &pubkey).is_err() {
                    log::debug!("Failed to verify signature!");
                    Err(InvalidEventError)
                } else {
                    Ok(())
                }
            } else {
                Err(InvalidEventError)
            }
        } else {
            Err(InvalidEventError)
        }
    }

    pub fn get_nip98_pubkey(&self, url: &str, method: &str) -> Option<String> {
        if self.validate_sig().is_err() {
            return None;
        }

        if self.kind != EVENT_KIND_AUTH || !self.content.is_empty() {
            return None;
        }

        let now = SystemTime::now();
        let one_min = Duration::from_secs(60);
        let created_at = UNIX_EPOCH + Duration::from_secs(self.created_at as u64);
        if created_at < now && created_at.elapsed().unwrap() > one_min {
            return None;
        }
        if created_at > now && created_at > now.checked_add(one_min).unwrap() {
            return None;
        }

        let tags = self.get_tags_hash();
        if tags.get("u")? != url {
            return None;
        }
        if tags.get("method")? != method {
            return None;
        }

        Some(self.pubkey.to_owned())
    }

    pub fn get_blossom_pubkey(&self, method: &str) -> Option<String> {
        if self.validate_sig().is_err() {
            return None;
        }

        if self.kind != EVENT_KIND_BLOSSOM {
            return None;
        }

        let now = SystemTime::now();
        let one_min = Duration::from_secs(60);
        let created_at = UNIX_EPOCH + Duration::from_secs(self.created_at as u64);
        if created_at > now.checked_add(one_min).unwrap() {
            return None;
        }

        let tags = self.get_tags_hash();
        if tags.get("t")? != method {
            return None;
        }
        let expiration = tags.get("expiration")?;
        let expiration = UNIX_EPOCH + Duration::from_secs(expiration.parse::<u64>().unwrap());
        if expiration < now {
            return None;
        }

        Some(self.pubkey.to_owned())
    }

    pub fn to_json(&self) -> JsonValue {
        json!({
            "id": self.id,
            "pubkey": self.pubkey,
            "created_at": self.created_at,
            "kind": self.kind,
            "tags": self.tags,
            "content": self.content,
            "sig": self.sig,
        })
    }

    fn to_canonical(&self) -> String {
        let c = json!([
            0,
            self.pubkey.to_owned(),
            self.created_at,
            self.kind,
            self.tags,
            self.content.to_owned(),
        ]);

        serde_json::to_string(&c).unwrap()
    }

    pub fn write(&self, filename: &str) -> std::io::Result<u64> {
        let path = Path::new(&filename);
        fs::create_dir_all(path.ancestors().nth(1).unwrap()).unwrap();
        let extension = path.extension().and_then(OsStr::to_str).unwrap();
        let mut file;
        let index;
        if extension == "mmd" {
            index = path.metadata()?.len();
            file = OpenOptions::new().append(true).open(path).unwrap();
            if index != 0 {
                writeln!(file)?;
                writeln!(file)?;
                writeln!(file)?;
            }
        } else {
            index = 0_u64;
            file = File::create(path).unwrap();
        }

        writeln!(file, "---")?;
        writeln!(file, "id: {}", self.id)?;
        writeln!(file, "pubkey: {}", self.pubkey)?;
        writeln!(file, "created_at: {}", self.created_at)?;
        writeln!(file, "kind: {}", self.kind)?;
        writeln!(file, "tags:")?;
        for tag in self.tags.clone() {
            for (i, t) in tag.iter().enumerate() {
                if i == 0 {
                    writeln!(file, "- - {}", t)?;
                } else {
                    writeln!(file, "  - \"{}\"", t)?;
                }
            }
        }
        writeln!(file, "sig: {}", self.sig)?;
        writeln!(file, "---")?;
        write!(file, "{}", self.content)?;

        Ok(index)
    }
}

fn get_metadata_tags(metadata: &HashMap<String, YamlValue>) -> Option<Vec<Vec<String>>> {
    let mut tags: Vec<Vec<String>> = vec![];
    if let Some(seq) = metadata.get("tags")?.as_sequence() {
        for tag in seq {
            let mut tag_vec: Vec<String> = vec![];
            for t in tag.as_sequence().unwrap() {
                tag_vec.push(t.as_str().unwrap().to_owned());
            }
            tags.push(tag_vec);
        }
    }

    Some(tags)
}

pub fn parse_event(front_matter: &HashMap<String, YamlValue>, content: &str) -> Option<Event> {
    Some(Event {
        id: front_matter.get("id")?.as_str()?.to_owned(),
        pubkey: front_matter.get("pubkey")?.as_str()?.to_owned(),
        created_at: front_matter.get("created_at")?.as_i64()?,
        kind: front_matter.get("kind")?.as_i64()?,
        tags: get_metadata_tags(front_matter)?,
        sig: front_matter.get("sig")?.as_str()?.to_owned(),
        content: content.trim_end_matches('\n').to_owned(),
    })
}

#[derive(Serialize, Deserialize)]
pub struct Filter {
    #[serde(flatten)]
    pub extra: HashMap<String, JsonValue>,
}

#[derive(Serialize, Deserialize)]
pub struct EventCmd {
    pub cmd: String,
    pub event: Event,
}

#[derive(Serialize, Deserialize)]
pub struct ReqCmd {
    pub cmd: String,
    pub subscription_id: String,
    pub filter: Filter,
}

#[derive(Serialize, Deserialize)]
pub struct CloseCmd {
    pub cmd: String,
    pub subscription_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(untagged)]
pub enum Message {
    Event(EventCmd),
    Req(ReqCmd),
    Close(CloseCmd),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_event() {
        let id = "0ff0c8f57ddea79cb9f12c574b5056b712d584b9fe55118149ea4b343d3f89a7";
        let pubkey = "f982dbf2a0a4a484c98c5cbb8b83a1ecaf6589cb2652e19381158b5646fe23d6";
        let created_at = 1710006173;
        let sig = "39944d4aa9bdba0b6739d6ee126ae84cdbacb90e9b4412ff44bf91c1948525c07ef022c5941921c25154d08b2a43bd3c8f4e5181b905eaaef18957d89d01f598";
        let content = "qwerty";

        let front_matter = format!(
            "id: {}\npubkey: {}\ncreated_at: {}\nkind: 1\ntags:\nsig: {}\n",
            id, pubkey, created_at, sig
        );
        let event = parse_event(&serde_yaml::from_str(&front_matter).unwrap(), content).unwrap();

        assert_eq!(event.id, id);
        assert_eq!(event.pubkey, pubkey);
        assert_eq!(event.kind, 1);
        assert_eq!(event.sig, sig);
        assert_eq!(event.content, content);

        // front matter without id
        let front_matter = format!(
            "id:\npubkey: {}\ncreated_at: {}\nkind: 1\ntags:\nsig: {}\n",
            pubkey, created_at, sig
        );
        let no_event = parse_event(&serde_yaml::from_str(&front_matter).unwrap(), content);

        assert!(no_event.is_none());
    }
}
