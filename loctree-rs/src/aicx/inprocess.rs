use super::{AicxIntent, ProjectScope};
use std::sync::mpsc;
use std::time::Duration;

#[derive(Debug)]
pub enum AicxInProcessError {
    TimedOut,
    Other(anyhow::Error),
}

#[derive(Debug)]
pub struct AicxInProcessClient {
    client: aicx::api::Aicx,
}

impl AicxInProcessClient {
    pub fn from_env() -> Result<Self, anyhow::Error> {
        let base = aicx::api::Aicx::from_env()?;
        let store_root = base.store_root().join("store");
        Ok(Self {
            client: aicx::api::Aicx::with_store_root(store_root),
        })
    }

    pub fn intents(
        &self,
        scope: &ProjectScope,
        window_hours: u64,
        limit: usize,
        cap: Option<Duration>,
    ) -> Result<Vec<AicxIntent>, AicxInProcessError> {
        let Some(cap) = cap else {
            return self
                .intents_unbounded(scope, window_hours, limit)
                .map_err(AicxInProcessError::Other);
        };

        let scope = scope.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = Self::from_env()
                .and_then(|client| client.intents_unbounded(&scope, window_hours, limit));
            let _ = tx.send(result);
        });

        match rx.recv_timeout(cap) {
            Ok(Ok(rows)) => Ok(rows),
            Ok(Err(error)) => Err(AicxInProcessError::Other(error)),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(AicxInProcessError::TimedOut),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(AicxInProcessError::Other(
                anyhow::anyhow!("in-process AICX worker disconnected"),
            )),
        }
    }

    fn intents_unbounded(
        &self,
        scope: &ProjectScope,
        window_hours: u64,
        limit: usize,
    ) -> Result<Vec<AicxIntent>, anyhow::Error> {
        let mut config = aicx::intents::IntentsConfig {
            project: scope.primary().to_string(),
            hours: window_hours,
            strict: false,
            min_confidence: None,
            kind_filter: None,
            frame_kind: Some(aicx::timeline::FrameKind::UserMsg),
        };
        let projects = scope.projects();
        let extraction = if projects.len() > 1 {
            let projects = projects.to_vec();
            self.client
                .extract_intents_for_projects(&config, &projects)?
        } else {
            if config.project.is_empty() {
                config.project = String::new();
            }
            self.client.extract_intents(&config)?
        };

        Ok(extraction
            .records
            .into_iter()
            .take(limit)
            .map(intent_record_to_loctree)
            .collect())
    }
}

fn intent_record_to_loctree(record: aicx::intents::IntentRecord) -> AicxIntent {
    AicxIntent {
        kind: intent_kind_label(record.kind).to_string(),
        text: record.summary,
        agent: record.agent,
        date: record.date,
        timestamp: record.timestamp,
        session_id: record.session_id,
        project: record.project,
        source_chunk_path: record.source_chunk,
        frame_kind: Some("user_msg".to_string()),
        oracle_status: None,
    }
}

fn intent_kind_label(kind: aicx::intents::IntentKind) -> &'static str {
    match kind {
        aicx::intents::IntentKind::Decision => "decision",
        aicx::intents::IntentKind::Intent => "intent",
        aicx::intents::IntentKind::Outcome => "outcome",
        aicx::intents::IntentKind::Task => "task",
    }
}
