use bitcoin_hashes::{sha256, Hash};
use lazy_static::lazy_static;
use secp256k1::{schnorr, Secp256k1, VerifyOnly, XOnlyPublicKey};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::str::FromStr;
use tide::log;

pub struct InvalidEventError;

#[derive(Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub pubkey: String,
    pub created_at: u64,
    pub kind: u64,
    pub tags: Vec<Vec<String>>,
    pub content: String,
    pub sig: String,
}

lazy_static! {
    pub static ref SECP: Secp256k1<VerifyOnly> = Secp256k1::verification_only();
}

impl Event {
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

    pub fn to_json(&self) -> serde_json::Value {
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
}

#[derive(Serialize, Deserialize)]
pub struct Filter {
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
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
