use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::time::{SystemTime, UNIX_EPOCH};

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

// Migration SQL constants — append only, never edit existing entries.
const MIGRATIONS: &[&str] = &[
    // v0 → v1: initial schema
    "CREATE TABLE IF NOT EXISTS topics (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        name       TEXT    NOT NULL UNIQUE,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS concepts (
        id           INTEGER PRIMARY KEY AUTOINCREMENT,
        topic_id     INTEGER NOT NULL REFERENCES topics(id),
        title        TEXT    NOT NULL,
        summary      TEXT    NOT NULL DEFAULT '',
        prerequisites TEXT   NOT NULL DEFAULT '[]',
        position     INTEGER NOT NULL DEFAULT 0,
        mastery      REAL    NOT NULL DEFAULT 0.0,
        repetitions  INTEGER NOT NULL DEFAULT 0,
        ease_factor  REAL    NOT NULL DEFAULT 2.5,
        interval_days INTEGER NOT NULL DEFAULT 1,
        next_review  INTEGER,
        UNIQUE(topic_id, title)
    );
    CREATE TABLE IF NOT EXISTS learning_sessions (
        id             INTEGER PRIMARY KEY AUTOINCREMENT,
        concept_id     INTEGER NOT NULL REFERENCES concepts(id),
        reviewed_at    INTEGER NOT NULL,
        quality        INTEGER NOT NULL,
        new_interval   INTEGER NOT NULL,
        new_ease_factor REAL   NOT NULL
    );",
];

#[derive(Clone)]
pub struct ConceptRow {
    pub id: i64,
    pub title: String,
    pub summary: String,
    pub prerequisites: Vec<String>,
    pub mastery: f64,
    pub repetitions: i64,
    pub ease_factor: f64,
    pub interval_days: i64,
    pub next_review: Option<i64>,
    #[allow(dead_code)] // used for SQL ORDER BY, stored for completeness
    pub position: i64,
}

pub struct TopicStats {
    pub name: String,
    pub total: i64,
    pub mastered: i64,
    pub in_progress: i64,
    pub not_started: i64,
    pub overdue: i64,
}

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open() -> Result<Self> {
        let path = db_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context("creating lersi data directory")?;
        }

        let conn = Connection::open(&path)
            .with_context(|| format!("opening SQLite database at {}", path.display()))?;

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .context("setting SQLite pragmas")?;

        let db = Database { conn };
        db.run_migrations().context("running schema migrations")?;
        Ok(db)
    }

    fn run_migrations(&self) -> Result<()> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);
                 INSERT INTO schema_version SELECT 0 WHERE NOT EXISTS (SELECT 1 FROM schema_version);",
            )
            .context("bootstrapping schema_version")?;

        let current: i64 = self
            .conn
            .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
            .context("reading schema version")?;

        for (i, sql) in MIGRATIONS.iter().enumerate() {
            let target = (i as i64) + 1;
            if current < target {
                self.conn
                    .execute_batch(sql)
                    .with_context(|| format!("migration {} failed", target))?;
                self.conn
                    .execute("UPDATE schema_version SET version = ?1", params![target])
                    .with_context(|| format!("updating schema_version to {}", target))?;
            }
        }

        Ok(())
    }

    pub fn upsert_topic(&self, name: &str) -> Result<i64> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO topics (name, created_at) VALUES (?1, ?2)",
                params![name, now_secs()],
            )
            .context("inserting topic")?;

        self.conn
            .query_row(
                "SELECT id FROM topics WHERE name = ?1",
                params![name],
                |r| r.get(0),
            )
            .context("fetching topic id")
    }

    /// INSERT OR IGNORE — never resets progress on existing concepts.
    pub fn upsert_concept(
        &self,
        topic_id: i64,
        title: &str,
        summary: &str,
        prerequisites: &[String],
        position: i64,
    ) -> Result<()> {
        let prereqs = serde_json::to_string(prerequisites).unwrap();
        self.conn
            .execute(
                "INSERT OR IGNORE INTO concepts
                     (topic_id, title, summary, prerequisites, position)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![topic_id, title, summary, prereqs, position],
            )
            .context("upserting concept")?;
        Ok(())
    }

    /// Mark a concept as fully mastered (prior knowledge shortcut).
    pub fn mark_mastered(&self, topic_id: i64, title: &str) -> Result<()> {
        self.conn
            .execute(
                "UPDATE concepts
                 SET mastery = 1.0, repetitions = 5, next_review = NULL
                 WHERE topic_id = ?1 AND title = ?2",
                params![topic_id, title],
            )
            .context("marking concept mastered")?;
        Ok(())
    }

    /// Return the next concept to study: overdue reviews first, then new concepts,
    /// respecting prerequisite mastery (>= 0.5).
    pub fn next_concept(&self, topic_id: i64) -> Result<Option<ConceptRow>> {
        let now = now_secs();
        let rows = self.all_concepts(topic_id)?;

        let mastery_map: std::collections::HashMap<String, f64> =
            rows.iter().map(|r| (r.title.clone(), r.mastery)).collect();

        let available = |c: &&ConceptRow| -> bool {
            c.mastery < 1.0
                && c.prerequisites
                    .iter()
                    .all(|p| mastery_map.get(p).copied().unwrap_or(0.0) >= 0.5)
        };

        // Overdue: next_review is set and in the past
        let mut overdue: Vec<&ConceptRow> = rows
            .iter()
            .filter(available)
            .filter(|c| c.next_review.map(|nr| nr <= now).unwrap_or(false))
            .collect();
        overdue.sort_by_key(|c| c.next_review.unwrap_or(i64::MIN));

        if let Some(c) = overdue.first() {
            return Ok(Some((*c).clone()));
        }

        // New: never reviewed yet
        let new_concept = rows.iter().filter(available).find(|c| c.next_review.is_none());

        Ok(new_concept.cloned())
    }

    /// Earliest next_review timestamp among not-yet-due concepts (for "come back in N days" UX).
    pub fn next_due_ts(&self, topic_id: i64) -> Result<Option<i64>> {
        let now = now_secs();
        let ts: Option<i64> = self
            .conn
            .query_row(
                "SELECT MIN(next_review) FROM concepts
                 WHERE topic_id = ?1 AND mastery < 1.0 AND next_review > ?2",
                params![topic_id, now],
                |r| r.get(0),
            )
            .context("querying next due timestamp")?;
        Ok(ts)
    }

    pub fn record_review(
        &self,
        concept_id: i64,
        quality: u8,
    ) -> Result<(ConceptRow, i64 /* new interval */)> {
        let now = now_secs();

        let concept = self
            .conn
            .query_row(
                "SELECT id, title, summary, prerequisites, mastery,
                        repetitions, ease_factor, interval_days, next_review, position
                 FROM concepts WHERE id = ?1",
                params![concept_id],
                concept_from_row,
            )
            .context("fetching concept for review")?;

        let result = crate::sm2::update(quality, concept.repetitions, concept.ease_factor, concept.interval_days);
        let next_review = now + result.interval_days * 86400;

        self.conn
            .execute(
                "UPDATE concepts
                 SET mastery = ?1, repetitions = ?2, ease_factor = ?3,
                     interval_days = ?4, next_review = ?5
                 WHERE id = ?6",
                params![
                    result.mastery,
                    result.repetitions,
                    result.ease_factor,
                    result.interval_days,
                    next_review,
                    concept_id
                ],
            )
            .context("updating concept after review")?;

        self.conn
            .execute(
                "INSERT INTO learning_sessions
                     (concept_id, reviewed_at, quality, new_interval, new_ease_factor)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    concept_id,
                    now,
                    quality,
                    result.interval_days,
                    result.ease_factor
                ],
            )
            .context("inserting learning session")?;

        let updated = self
            .conn
            .query_row(
                "SELECT id, title, summary, prerequisites, mastery,
                        repetitions, ease_factor, interval_days, next_review, position
                 FROM concepts WHERE id = ?1",
                params![concept_id],
                concept_from_row,
            )
            .context("re-fetching updated concept")?;

        Ok((updated, result.interval_days))
    }

    pub fn topic_stats(&self, topic_id: i64, topic_name: &str) -> Result<TopicStats> {
        let now = now_secs();

        let total: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM concepts WHERE topic_id = ?1",
                params![topic_id],
                |r| r.get(0),
            )
            .context("counting concepts")?;

        let mastered: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM concepts WHERE topic_id = ?1 AND mastery >= 1.0",
                params![topic_id],
                |r| r.get(0),
            )
            .context("counting mastered")?;

        let in_progress: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM concepts
                 WHERE topic_id = ?1 AND mastery > 0.0 AND mastery < 1.0",
                params![topic_id],
                |r| r.get(0),
            )
            .context("counting in_progress")?;

        let not_started: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM concepts
                 WHERE topic_id = ?1 AND repetitions = 0 AND mastery < 1.0",
                params![topic_id],
                |r| r.get(0),
            )
            .context("counting not_started")?;

        let overdue: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM concepts
                 WHERE topic_id = ?1 AND next_review IS NOT NULL AND next_review <= ?2 AND mastery < 1.0",
                params![topic_id, now],
                |r| r.get(0),
            )
            .context("counting overdue")?;

        Ok(TopicStats {
            name: topic_name.to_string(),
            total,
            mastered,
            in_progress,
            not_started,
            overdue,
        })
    }

    /// All topic (id, name) pairs.
    pub fn all_topics(&self) -> Result<Vec<(i64, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name FROM topics ORDER BY name")
            .context("preparing all_topics")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .context("querying topics")?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("collecting topics")?;
        Ok(rows)
    }

    pub fn concept_count(&self, topic_id: i64) -> Result<i64> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM concepts WHERE topic_id = ?1",
                params![topic_id],
                |r| r.get(0),
            )
            .context("counting concepts")
    }

    pub fn topic_id_by_name(&self, name: &str) -> Result<Option<i64>> {
        match self.conn.query_row(
            "SELECT id FROM topics WHERE name = ?1",
            params![name],
            |r| r.get(0),
        ) {
            Ok(id) => Ok(Some(id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e).context("fetching topic by name"),
        }
    }

    fn all_concepts(&self, topic_id: i64) -> Result<Vec<ConceptRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, title, summary, prerequisites, mastery,
                        repetitions, ease_factor, interval_days, next_review, position
                 FROM concepts WHERE topic_id = ?1
                 ORDER BY position ASC",
            )
            .context("preparing all_concepts")?;

        let rows = stmt
            .query_map(params![topic_id], concept_from_row)
            .context("querying concepts")?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("collecting concepts")?;

        Ok(rows)
    }
}

fn concept_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ConceptRow> {
    let prereqs_str: String = r.get(3)?;
    let prerequisites: Vec<String> = serde_json::from_str(&prereqs_str).unwrap_or_default();
    Ok(ConceptRow {
        id: r.get(0)?,
        title: r.get(1)?,
        summary: r.get(2)?,
        prerequisites,
        mastery: r.get(4)?,
        repetitions: r.get(5)?,
        ease_factor: r.get(6)?,
        interval_days: r.get(7)?,
        next_review: r.get(8)?,
        position: r.get(9)?,
    })
}

fn db_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("LERSI_DB_PATH") {
        return std::path::PathBuf::from(p);
    }
    dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("lersi")
        .join("lersi.db")
}
