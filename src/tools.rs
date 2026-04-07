use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::db::Database;

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

// ─── Input types ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ConceptInput {
    title: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    prerequisites: Vec<String>,
}

#[derive(Deserialize)]
struct ConceptGraph {
    concepts: Vec<ConceptInput>,
}

// ─── Tool list ───────────────────────────────────────────────────────────────

pub fn list() -> Value {
    json!([
        {
            "name": "learn__start_topic",
            "description": "Initialize a learning topic with a generated curriculum (ConceptGraph). \
                            Call this at the start of a new learning session for a topic. \
                            Concepts that already exist are preserved (no progress reset). \
                            Pass prior_knowledge to skip concepts the user already knows.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "topic": {
                        "type": "string",
                        "description": "Name of the learning topic (e.g. 'Rust Programming')"
                    },
                    "concept_graph": {
                        "type": "object",
                        "description": "Ordered curriculum graph you generated",
                        "properties": {
                            "concepts": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "title":         { "type": "string" },
                                        "summary":       { "type": "string" },
                                        "prerequisites": { "type": "array", "items": { "type": "string" } }
                                    },
                                    "required": ["title"]
                                }
                            }
                        },
                        "required": ["concepts"]
                    },
                    "prior_knowledge": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Concept titles the user already knows (marked mastered, skipped in reviews)"
                    }
                },
                "required": ["topic", "concept_graph"]
            }
        },
        {
            "name": "learn__next_concept",
            "description": "Get the next concept for the user to study. \
                            Returns overdue reviews first, then new concepts in curriculum order, \
                            respecting prerequisites. \
                            Returns {status:'all_done'} when everything is mastered, \
                            or {status:'no_due'} when nothing is due today.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "topic": { "type": "string", "description": "Topic name" }
                },
                "required": ["topic"]
            }
        },
        {
            "name": "learn__record_review",
            "description": "Record the outcome of a concept review using SM-2 quality scores. \
                            Call after teaching/quizzing the user on a concept. \
                            Quality: 0=complete blackout, 1=wrong, 2=wrong but familiar, \
                            3=correct with difficulty, 4=correct with hesitation, 5=perfect recall.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "concept_id": {
                        "type": "integer",
                        "description": "Concept ID returned by learn__next_concept"
                    },
                    "quality": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": 5,
                        "description": "SM-2 quality score (0–5)"
                    }
                },
                "required": ["concept_id", "quality"]
            }
        },
        {
            "name": "learn__status",
            "description": "Get learning progress for one or all topics. \
                            Shows mastered/in-progress/not-started counts and overdue reviews.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "topic": {
                        "type": "string",
                        "description": "Topic name (omit to get all topics)"
                    }
                }
            }
        }
    ])
}

// ─── Dispatch ────────────────────────────────────────────────────────────────

pub fn call(db: &Database, name: &str, args: &Value) -> Result<Value> {
    let result = match name {
        "learn__start_topic"  => start_topic(db, args),
        "learn__next_concept" => next_concept(db, args),
        "learn__record_review" => record_review(db, args),
        "learn__status"       => status(db, args),
        _ => bail!("unknown tool: {}", name),
    };

    match result {
        Ok(v) => Ok(json!({ "content": [{ "type": "text", "text": v.to_string() }] })),
        Err(e) => Ok(json!({
            "content": [{
                "type": "text",
                "text": json!({
                    "error": "internal_error",
                    "message": e.to_string()
                }).to_string()
            }],
            "isError": true
        })),
    }
}

// ─── Handlers ────────────────────────────────────────────────────────────────

fn start_topic(db: &Database, args: &Value) -> Result<Value> {
    let topic = args["topic"]
        .as_str()
        .context("missing required field: topic")?;

    let graph: ConceptGraph = serde_json::from_value(
        args["concept_graph"].clone(),
    )
    .context("invalid concept_graph")?;

    let prior: Vec<String> = args
        .get("prior_knowledge")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    let topic_id = db.upsert_topic(topic).context("upserting topic")?;

    let mut new_count = 0i64;
    let mut skipped = 0i64;

    // We track new vs existing by comparing total count before and after each
    // INSERT OR IGNORE. Slightly chatty but correct for small-to-medium graphs.
    for (i, concept) in graph.concepts.iter().enumerate() {
        let before: i64 = db.concept_count(topic_id).context("counting before upsert")?;

        db.upsert_concept(
            topic_id,
            &concept.title,
            &concept.summary,
            &concept.prerequisites,
            i as i64,
        )
        .context("upserting concept")?;

        let after: i64 = db.concept_count(topic_id).context("counting after upsert")?;

        if after > before {
            new_count += 1;
        } else {
            skipped += 1;
        }
    }

    // Mark prior knowledge as mastered
    let mut marked = 0i64;
    for title in &prior {
        db.mark_mastered(topic_id, title)
            .context("marking prior knowledge")?;
        marked += 1;
    }

    Ok(json!({
        "topic": topic,
        "total_concepts": graph.concepts.len(),
        "new_concepts": new_count,
        "existing_concepts": skipped,
        "prior_knowledge_marked": marked,
        "message": format!(
            "Ready! {} concepts loaded ({} new, {} existing). \
             Call learn__next_concept to get started.",
            graph.concepts.len(), new_count, skipped
        )
    }))
}

fn next_concept(db: &Database, args: &Value) -> Result<Value> {
    let topic = args["topic"]
        .as_str()
        .context("missing required field: topic")?;

    let topic_id = db
        .topic_id_by_name(topic)
        .context("looking up topic")?
        .with_context(|| format!("topic '{}' not found — call learn__start_topic first", topic))?;

    match db.next_concept(topic_id).context("getting next concept")? {
        Some(c) => {
            let now = now_secs();
            let is_new = c.next_review.is_none();
            let overdue_days = c
                .next_review
                .map(|nr| ((now - nr) / 86400).max(0))
                .unwrap_or(0);

            Ok(json!({
                "status": "concept",
                "concept": {
                    "id": c.id,
                    "title": c.title,
                    "summary": c.summary,
                    "is_new": is_new,
                    "mastery": c.mastery,
                    "repetitions": c.repetitions,
                    "overdue_by_days": overdue_days
                }
            }))
        }
        None => {
            // Check if everything is mastered or just nothing due today
            let stats = db.topic_stats(topic_id, topic).context("getting topic stats")?;

            if stats.not_started == 0 && stats.in_progress == 0 {
                return Ok(json!({
                    "status": "all_done",
                    "message": format!(
                        "All {} concepts for '{}' are mastered. Great work!",
                        stats.mastered, topic
                    )
                }));
            }

            let next_ts = db.next_due_ts(topic_id).context("getting next due timestamp")?;
            let days_until = next_ts
                .map(|ts| ((ts - now_secs()) / 86400 + 1).max(1))
                .unwrap_or(1);

            Ok(json!({
                "status": "no_due",
                "next_review_in_days": days_until,
                "message": format!(
                    "Nothing due today for '{}'. Come back in ~{} day(s) for your next review.",
                    topic, days_until
                ),
                "stats": {
                    "mastered": stats.mastered,
                    "in_progress": stats.in_progress,
                    "not_started": stats.not_started,
                    "total": stats.total
                }
            }))
        }
    }
}

fn record_review(db: &Database, args: &Value) -> Result<Value> {
    let concept_id = args["concept_id"]
        .as_i64()
        .context("missing required field: concept_id")?;

    let quality = args["quality"]
        .as_u64()
        .context("missing required field: quality")? as u8;

    if quality > 5 {
        bail!("quality must be 0–5, got {}", quality);
    }

    let (updated, interval_days) = db
        .record_review(concept_id, quality)
        .context("recording review")?;

    let passed = quality >= 3;
    let message = if !passed {
        format!(
            "Needs more work. '{}' will come up again tomorrow.",
            updated.title
        )
    } else if updated.mastery >= 1.0 {
        format!("'{}' is mastered! No further reviews needed.", updated.title)
    } else {
        format!(
            "Good{}! '{}' scheduled for review in {} day(s). Mastery: {:.0}%.",
            if quality == 5 { " recall" } else { "" },
            updated.title,
            interval_days,
            updated.mastery * 100.0
        )
    };

    Ok(json!({
        "concept_id": concept_id,
        "title": updated.title,
        "quality": quality,
        "passed": passed,
        "mastery": updated.mastery,
        "repetitions": updated.repetitions,
        "next_review_in_days": interval_days,
        "message": message
    }))
}

fn status(db: &Database, args: &Value) -> Result<Value> {
    let filter_topic = args.get("topic").and_then(|v| v.as_str());

    let topics = db.all_topics().context("fetching topics")?;

    if topics.is_empty() {
        return Ok(json!({
            "topics": [],
            "message": "No topics yet. Call learn__start_topic to begin."
        }));
    }

    let mut results = Vec::new();
    for (id, name) in &topics {
        if let Some(filter) = filter_topic {
            if name != filter {
                continue;
            }
        }
        let stats = db.topic_stats(*id, name).context("computing topic stats")?;
        let progress_pct = if stats.total > 0 {
            (stats.mastered * 100) / stats.total
        } else {
            0
        };
        results.push(json!({
            "topic": stats.name,
            "total": stats.total,
            "mastered": stats.mastered,
            "in_progress": stats.in_progress,
            "not_started": stats.not_started,
            "overdue_reviews": stats.overdue,
            "progress_percent": progress_pct
        }));
    }

    if results.is_empty() {
        if let Some(t) = filter_topic {
            bail!("topic '{}' not found", t);
        }
    }

    Ok(json!({ "topics": results }))
}
