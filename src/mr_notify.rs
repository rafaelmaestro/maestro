//! O que vira notificação na inbox: MR novo para revisar e resposta numa thread em que você comentou.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::mrs::Row;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    /// a primeira leitura só grava a base: abrir o app não dispara uma notificação por MR que já existia
    init: bool,
    review: BTreeSet<String>,
    /// `path!iid` → (`user_notes_count`, `updated_at`) da última leitura
    seen: BTreeMap<String, (u64, String)>,
    /// `path!iid` → maior id de nota já visto
    last_note: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Notice {
    NewMr {
        key: String,
        title: String,
        who: String,
    },
    Reply {
        key: String,
        title: String,
        who: String,
        /// id da nota nova, para o selo "nova" na thread
        note: u64,
    },
}

fn file() -> PathBuf {
    std::env::var_os("MAESTRO_MR_NOTIFY").map_or_else(
        || {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                .join(".local/share/maestro/mr-notify.json")
        },
        PathBuf::from,
    )
}

impl State {
    pub fn load() -> Self {
        std::fs::read_to_string(file())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let f = file();
        if let Some(d) = f.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(f, serde_json::to_string(self).unwrap_or_default());
    }

    /// MRs que entraram em "Para revisar" desde a última leitura.
    pub fn new_mrs(&mut self, review: &[Row]) -> Vec<Notice> {
        let fresh: Vec<Notice> = review
            .iter()
            .filter(|r| self.init && !self.review.contains(&r.key()))
            .map(|r| Notice::NewMr {
                key: r.key(),
                title: r.title.clone(),
                who: r.who.clone(),
            })
            .collect();
        self.review = review.iter().map(Row::key).collect();
        self.init = true;
        fresh
    }

    /// Só vale reler as discussões quando o MR mudou desde a última leitura.
    pub fn changed(&self, r: &Row) -> bool {
        self.seen.get(&r.key()) != Some(&(r.notes, r.updated.clone()))
    }

    /// Notas de outras pessoas, mais novas que a última vista, em discussões onde `me` escreveu.
    pub fn replies(&mut self, r: &Row, discussions: &Value, me: &str) -> Vec<Notice> {
        let key = r.key();
        let notes = |d: &Value| d["notes"].as_array().cloned().unwrap_or_default();
        let all = discussions.as_array().cloned().unwrap_or_default();
        let max = all
            .iter()
            .flat_map(notes)
            .filter_map(|n| n["id"].as_u64())
            .max()
            .unwrap_or_default();
        let last = self.last_note.get(&key).copied();
        self.seen.insert(key.clone(), (r.notes, r.updated.clone()));
        self.last_note
            .insert(key.clone(), max.max(last.unwrap_or_default()));
        let Some(last) = last else {
            return Vec::new();
        };
        let author = |n: &Value| {
            n["author"]["username"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        };
        all.iter()
            .filter(|d| notes(d).iter().any(|n| author(n) == me))
            .flat_map(notes)
            .filter(|n| n["system"] != true && author(n) != me)
            .filter(|n| n["id"].as_u64().is_some_and(|id| id > last))
            .map(|n| Notice::Reply {
                key: key.clone(),
                title: r.title.clone(),
                who: format!("@{}", author(&n)),
                note: n["id"].as_u64().unwrap_or_default(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(iid: u64, notes: u64) -> Row {
        Row {
            path: "g/bots-api".into(),
            iid,
            title: format!("MR {iid}"),
            who: "@bruno".into(),
            sha: String::new(),
            source: String::new(),
            target: String::new(),
            web_url: String::new(),
            ci: None,
            notes,
            updated: format!("t{notes}"),
            created: String::new(),
        }
    }

    fn note(id: u64, who: &str) -> Value {
        json!({"id": id, "author": {"username": who}, "system": false})
    }

    #[test]
    fn mr_novo_so_depois_da_primeira_leitura() {
        let mut s = State::default();
        assert!(s.new_mrs(&[row(1, 0)]).is_empty());
        assert!(s.new_mrs(&[row(1, 0)]).is_empty());
        let n = s.new_mrs(&[row(1, 0), row(2, 0)]);
        assert!(matches!(&n[..], [Notice::NewMr { key, .. }] if key == "g/bots-api!2"));
    }

    #[test]
    fn resposta_de_outro_numa_thread_minha() {
        let mut s = State::default();
        let r = row(1, 2);
        let d1 = json!([
            {"id": "a", "notes": [note(10, "eu"), note(11, "ana")]},
            {"id": "b", "notes": [note(12, "bruno")]}
        ]);
        assert!(
            s.replies(&r, &d1, "eu").is_empty(),
            "primeira leitura é a base"
        );
        assert!(!s.changed(&r));
        let r2 = row(1, 5);
        assert!(s.changed(&r2));
        let d2 = json!([
            {"id": "a", "notes": [note(10, "eu"), note(11, "ana"), note(20, "ana"), note(21, "eu")]},
            {"id": "b", "notes": [note(12, "bruno"), note(22, "bruno")]},
            {"id": "c", "notes": [{"id": 23, "author": {"username": "ana"}, "system": true}]}
        ]);
        let got = s.replies(&r2, &d2, "eu");
        assert_eq!(
            got.len(),
            1,
            "a thread b não é minha, nota minha e de sistema não contam"
        );
        assert!(matches!(&got[0], Notice::Reply { who, note: 20, .. } if who == "@ana"));
        assert!(s.replies(&r2, &d2, "eu").is_empty(), "não notifica de novo");
    }
}
