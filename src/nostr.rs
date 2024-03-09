use bitcoin_hashes::{sha256, Hash};
use chrono::TimeZone;
use chrono::{DateTime, NaiveDateTime, Utc};
use lazy_static::lazy_static;
use secp256k1::{schnorr, Secp256k1, VerifyOnly, XOnlyPublicKey};
use serde::{Deserialize, Serialize};
use serde_json::json;
use serde_json::Value as JsonValue;
use serde_yaml::Value as YamlValue;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::str;
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tide::log;
use yaml_front_matter::{Document, YamlFrontMatter};

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
pub const EVENT_KIND_AUTH: i64 = 27235;
pub const EVENT_KIND_LONG_FORM: i64 = 30023;
pub const EVENT_KIND_LONG_FORM_DRAFT: i64 = 30024;

lazy_static! {
    pub static ref SECP: Secp256k1<VerifyOnly> = Secp256k1::verification_only();
}

impl Event {
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

    pub fn get_long_form_tag(&self, tag: &str) -> Option<String> {
        if self.kind != EVENT_KIND_LONG_FORM && self.kind != EVENT_KIND_LONG_FORM_DRAFT {
            return None;
        }

        self.get_tags_hash().get(tag).cloned()
    }

    pub fn get_long_form_slug(&self) -> Option<String> {
        self.get_long_form_tag("d")
    }

    pub fn get_long_form_summary(&self) -> Option<String> {
        self.get_long_form_tag("summary")
    }

    pub fn get_long_form_published_at(&self) -> Option<NaiveDateTime> {
        let ts = self
            .get_long_form_tag("published_at")?
            .parse::<i64>()
            .unwrap();

        NaiveDateTime::from_timestamp_opt(ts, 0)
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

    pub fn write(&self, fd: &mut fs::File) -> std::io::Result<()> {
        // TODO: receive a path, so it can create all intermediate directories!

        writeln!(fd, "---")?;
        writeln!(fd, "id: {}", self.id)?;
        writeln!(fd, "pubkey: {}", self.pubkey)?;
        writeln!(fd, "created_at: {}", self.created_at)?;
        writeln!(fd, "kind: {}", self.kind)?;
        writeln!(fd, "tags:")?;
        for tag in self.tags.clone() {
            for (i, t) in tag.iter().enumerate() {
                if i == 0 {
                    writeln!(fd, "- - {}", t)?;
                } else {
                    writeln!(fd, "  - \"{}\"", t)?;
                }
            }
        }
        writeln!(fd, "sig: {}", self.sig)?;
        writeln!(fd, "---")?;
        write!(fd, "{}", self.content)?;

        Ok(())
    }
}

fn get_metadata_tags(document: &Document<HashMap<String, YamlValue>>) -> Option<Vec<Vec<String>>> {
    let mut tags: Vec<Vec<String>> = vec![];
    if let Some(seq) = document.metadata.get("tags")?.as_sequence() {
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

pub fn read_event(path: &str) -> Option<Event> {
    if let Ok(content) = fs::read_to_string(path) {
        if let Ok(document) = YamlFrontMatter::parse::<HashMap<String, YamlValue>>(&content) {
            return Some(Event {
                id: document.metadata.get("id")?.as_str()?.to_owned(),
                pubkey: document.metadata.get("pubkey")?.as_str()?.to_owned(),
                created_at: document.metadata.get("created_at")?.as_i64()?,
                kind: document.metadata.get("kind")?.as_i64()?,
                tags: get_metadata_tags(&document)?,
                content: document.content.to_owned(),
                sig: document.metadata.get("sig")?.as_str()?.to_owned(),
            });
        }
    }

    None
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
