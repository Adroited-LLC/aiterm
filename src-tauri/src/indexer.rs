//! Full-text session search, modeled on claudeman's tantivy indexer
//! (~/Projects/claudeman/src/indexer.rs): index session titles, project dirs,
//! and the actual user/assistant message text; search returns ranked ids.

use crate::sessions::{ClaudeProvider, Session};
use serde::Serialize;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use tantivy::collector::TopDocs;
use tantivy::query::{QueryParser, TermQuery};
use tantivy::schema::{
    IndexRecordOption, Schema, TextFieldIndexing, TextOptions, Value, INDEXED, STORED, STRING,
};
use tantivy::{doc, Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term};

struct SessionIndex {
    index: Index,
    reader: IndexReader,
    schema: Schema,
    /// None when another aiterm instance holds the tantivy writer lock —
    /// searches still work read-only; reindexing is skipped.
    writer: Mutex<Option<IndexWriter>>,
}

fn build_schema() -> Schema {
    let mut b = Schema::builder();
    b.add_text_field("session_id", STRING | STORED);
    b.add_u64_field("file_mtime", INDEXED | STORED);
    b.add_text_field("name", tantivy::schema::TEXT);
    b.add_text_field("project_dir", tantivy::schema::TEXT);
    let indexed_only = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer("default")
            .set_index_option(tantivy::schema::IndexRecordOption::WithFreqsAndPositions),
    );
    b.add_text_field("user_messages", indexed_only.clone());
    b.add_text_field("assistant_messages", indexed_only);
    b.build()
}

fn index_dir() -> Option<std::path::PathBuf> {
    Some(dirs::cache_dir()?.join("aiterm/session-index"))
}

fn global() -> Option<&'static SessionIndex> {
    static IDX: OnceLock<Option<SessionIndex>> = OnceLock::new();
    IDX.get_or_init(|| {
        let dir = index_dir()?;
        std::fs::create_dir_all(&dir).ok()?;
        let schema = build_schema();
        let index = Index::open_in_dir(&dir)
            .or_else(|_| Index::create_in_dir(&dir, schema.clone()))
            .ok()?;
        let writer = index.writer(30_000_000).ok();
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .ok()?;
        Some(SessionIndex {
            index,
            reader,
            schema,
            writer: Mutex::new(writer),
        })
    })
    .as_ref()
}

impl SessionIndex {
    fn stored_mtime(&self, session_id: &str) -> Option<u64> {
        let searcher = self.reader.searcher();
        let id_field = self.schema.get_field("session_id").ok()?;
        let mtime_field = self.schema.get_field("file_mtime").ok()?;
        let term = Term::from_field_text(id_field, session_id);
        let query = TermQuery::new(term, IndexRecordOption::Basic);
        let hits = searcher
            .search(&query, &TopDocs::with_limit(1).order_by_score())
            .ok()?;
        let (_, addr) = hits.first()?;
        let doc: TantivyDocument = searcher.doc(*addr).ok()?;
        doc.get_first(mtime_field)?.as_u64()
    }

    fn has_writer(&self) -> bool {
        self.writer.lock().map(|w| w.is_some()).unwrap_or(false)
    }

    fn upsert(&self, s: &Session, mtime: u64, user_text: &str, assistant_text: &str) {
        let Ok(guard) = self.writer.lock() else { return };
        let Some(writer) = guard.as_ref() else { return };
        let id_field = self.schema.get_field("session_id").unwrap();
        writer.delete_term(Term::from_field_text(id_field, &s.id));
        let _ = writer.add_document(doc!(
            id_field => s.id.clone(),
            self.schema.get_field("file_mtime").unwrap() => mtime,
            self.schema.get_field("name").unwrap() => s.title.clone(),
            self.schema.get_field("project_dir").unwrap() => s.project_path.clone(),
            self.schema.get_field("user_messages").unwrap() => user_text,
            self.schema.get_field("assistant_messages").unwrap() => assistant_text,
        ));
    }

    fn commit(&self) {
        if let Ok(mut guard) = self.writer.lock() {
            if let Some(writer) = guard.as_mut() {
                let _ = writer.commit();
            }
        }
        let _ = self.reader.reload();
    }

    fn search_ids(&self, query_str: &str) -> Vec<String> {
        let searcher = self.reader.searcher();
        let fields = ["name", "user_messages", "assistant_messages", "project_dir"]
            .iter()
            .filter_map(|f| self.schema.get_field(f).ok())
            .collect();
        let parser = QueryParser::for_index(&self.index, fields);
        let (query, _errs) = parser.parse_query_lenient(query_str);
        let Ok(hits) = searcher.search(&query, &TopDocs::with_limit(100).order_by_score()) else {
            return vec![];
        };
        let id_field = self.schema.get_field("session_id").unwrap();
        hits.iter()
            .filter_map(|(_, addr)| {
                let doc: TantivyDocument = searcher.doc(*addr).ok()?;
                Some(doc.get_first(id_field)?.as_str()?.to_string())
            })
            .collect()
    }
}

/// Extract searchable user/assistant text from a session transcript, capped
/// so huge sessions don't bloat the index.
fn extract_texts(path: &Path) -> (String, String) {
    const CAP: usize = 200_000;
    let mut user = String::new();
    let mut assistant = String::new();
    let Ok(file) = File::open(path) else {
        return (user, assistant);
    };
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if user.len() >= CAP && assistant.len() >= CAP {
            break;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let (bucket, cap_left) = match v.get("type").and_then(|t| t.as_str()) {
            Some("user") => (&mut user, CAP),
            Some("assistant") => (&mut assistant, CAP),
            _ => continue,
        };
        let Some(content) = v.pointer("/message/content") else {
            continue;
        };
        match content {
            serde_json::Value::String(s) => {
                if bucket.len() < cap_left {
                    bucket.push_str(s);
                    bucket.push('\n');
                }
            }
            serde_json::Value::Array(blocks) => {
                for block in blocks {
                    if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                        if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                            if bucket.len() < cap_left {
                                bucket.push_str(t);
                                bucket.push('\n');
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    (user, assistant)
}

#[derive(Serialize)]
pub struct ReindexResult {
    pub indexed: usize,
    pub total: usize,
}

/// Bring the index up to date with the session store (mtime-based, so only
/// new/changed transcripts get re-read).
#[tauri::command]
pub fn reindex_sessions() -> ReindexResult {
    let sessions = ClaudeProvider.scan_with_paths();
    let total = sessions.len();
    let Some(idx) = global() else {
        return ReindexResult { indexed: 0, total };
    };
    if !idx.has_writer() {
        // Another instance owns the index writer; it will do the reindexing.
        return ReindexResult { indexed: 0, total };
    }
    let mut indexed = 0;
    for (s, path) in &sessions {
        if idx.stored_mtime(&s.id) == Some(s.last_active) {
            continue;
        }
        let (user_text, assistant_text) = extract_texts(path);
        idx.upsert(s, s.last_active, &user_text, &assistant_text);
        indexed += 1;
    }
    if indexed > 0 {
        idx.commit();
    }
    ReindexResult { indexed, total }
}

/// Ranked full-text search across titles, project dirs, and message text.
#[tauri::command]
pub fn search_sessions(query: String) -> Vec<Session> {
    let Some(idx) = global() else { return vec![] };
    let by_id: std::collections::HashMap<String, Session> = ClaudeProvider
        .scan_with_paths()
        .into_iter()
        .map(|(s, _)| (s.id.clone(), s))
        .collect();
    idx.search_ids(&query)
        .into_iter()
        .filter_map(|id| by_id.get(&id).cloned())
        .collect()
}
