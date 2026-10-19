//! Short-lived identity lists freeze order, never runtime data.
use std::{
    collections::{HashMap, HashSet},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use uuid::Uuid;

use super::types::{DashboardFilters, DashboardStatusFilter};

const IDLE_TTL: Duration = Duration::from_secs(300);
const MAX_SNAPSHOTS: usize = 128;
const MAX_IDENTITIES: usize = 100_000;

struct Snapshot {
    scope: String,
    fingerprint: String,
    ids: Vec<String>,
    filters: DashboardFilters,
    touched: Instant,
}

fn cache() -> &'static Mutex<HashMap<Uuid, Snapshot>> {
    static CACHE: OnceLock<Mutex<HashMap<Uuid, Snapshot>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub struct Selection {
    pub snapshot_id: Option<Uuid>,
    pub status: &'static str,
    pub ids: Vec<String>,
    pub total: usize,
    pub has_updates: bool,
}

// The selector binds access scope, query identity, fallback and page separately.
#[allow(clippy::too_many_arguments)]
pub fn select(
    scope: &str,
    fingerprint: String,
    requested: Option<Uuid>,
    desired: &[String],
    fallback: &[String],
    page: u32,
    page_size: u32,
    filters: &DashboardFilters,
) -> Result<Selection, &'static str> {
    let now = Instant::now();
    let current_ids = (filters.status == DashboardStatusFilter::Current)
        .then(|| desired.iter().map(String::as_str).collect::<HashSet<_>>());
    let fallback_ids = || {
        fallback
            .iter()
            .filter(|id| {
                current_ids
                    .as_ref()
                    .is_none_or(|ids| ids.contains(id.as_str()))
            })
            .cloned()
            .collect()
    };
    let mut cache = cache().lock().unwrap_or_else(|e| e.into_inner());
    cache.retain(|_, s| now.duration_since(s.touched) < IDLE_TTL);
    if let Some(id) = requested {
        if let Some(snapshot) = cache.get_mut(&id) {
            if snapshot.scope != scope || snapshot.fingerprint != fingerprint {
                return Err("Snapshot does not belong to this query");
            }
            snapshot.touched = now;
            // Ongoing work disappears when it ends, while surviving rows keep
            // their relative order. New work still waits for explicit refresh.
            if let Some(ids) = &current_ids {
                snapshot.ids.retain(|id| ids.contains(id.as_str()));
            }
            let offset = page as usize * page_size as usize;
            return Ok(Selection {
                snapshot_id: Some(id),
                status: "ready",
                total: snapshot.ids.len(),
                ids: snapshot
                    .ids
                    .iter()
                    .skip(offset)
                    .take(page_size as usize)
                    .cloned()
                    .collect(),
                has_updates: snapshot.ids != desired,
            });
        }
        return Ok(Selection {
            snapshot_id: None,
            status: "expired",
            ids: fallback_ids(),
            total: desired.len(),
            has_updates: true,
        });
    }
    if cache.len() >= MAX_SNAPSHOTS
        || cache.values().map(|s| s.ids.len()).sum::<usize>() + desired.len() > MAX_IDENTITIES
    {
        return Ok(Selection {
            snapshot_id: None,
            status: "capacity",
            ids: fallback_ids(),
            total: desired.len(),
            has_updates: true,
        });
    }
    let id = Uuid::new_v4();
    let offset = page as usize * page_size as usize;
    let selection = Selection {
        snapshot_id: Some(id),
        status: "ready",
        total: desired.len(),
        ids: desired
            .iter()
            .skip(offset)
            .take(page_size as usize)
            .cloned()
            .collect(),
        has_updates: false,
    };
    cache.insert(
        id,
        Snapshot {
            scope: scope.into(),
            fingerprint,
            ids: desired.to_vec(),
            filters: filters.clone(),
            touched: now,
        },
    );
    Ok(selection)
}

pub fn filters(scope: &str, id: Uuid) -> Option<DashboardFilters> {
    let cache = cache().lock().unwrap_or_else(|e| e.into_inner());
    let snapshot = cache.get(&id)?;
    (snapshot.scope == scope && snapshot.touched.elapsed() < IDLE_TTL)
        .then(|| snapshot.filters.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordering_is_frozen_and_scoped() {
        let scope = Uuid::new_v4().to_string();
        let initial = vec!["a".into(), "b".into(), "c".into()];
        let first = select(
            &scope,
            "query".into(),
            None,
            &initial,
            &[],
            0,
            2,
            &DashboardFilters::default(),
        )
        .unwrap();
        let changed = vec!["x".into(), "c".into(), "a".into(), "b".into()];
        let second = select(
            &scope,
            "query".into(),
            first.snapshot_id,
            &changed,
            &[],
            1,
            2,
            &DashboardFilters::default(),
        )
        .unwrap();
        assert_eq!(second.ids, vec!["c"]);
        assert!(second.has_updates);
        assert!(
            select(
                "another-host",
                "query".into(),
                first.snapshot_id,
                &changed,
                &[],
                0,
                2,
                &DashboardFilters::default()
            )
            .is_err()
        );
        let expired = select(
            &scope,
            "query".into(),
            Some(Uuid::new_v4()),
            &changed,
            &["a".into()],
            0,
            2,
            &DashboardFilters::default(),
        )
        .unwrap();
        assert_eq!(expired.status, "expired");
        assert_eq!(expired.ids, vec!["a"]);
    }

    #[test]
    fn current_removes_finished_rows_without_inserting_or_reordering_work() {
        let scope = Uuid::new_v4().to_string();
        let filters = DashboardFilters::default();
        let first = select(
            &scope,
            "current".into(),
            None,
            &["a".into(), "b".into(), "c".into()],
            &[],
            0,
            20,
            &filters,
        )
        .unwrap();
        let desired = vec!["c".into(), "d".into(), "a".into()];
        let next = select(
            &scope,
            "current".into(),
            first.snapshot_id,
            &desired,
            &[],
            0,
            20,
            &filters,
        )
        .unwrap();
        assert_eq!(next.snapshot_id, first.snapshot_id);
        assert_eq!(next.ids, vec!["a", "c"]);
        assert_eq!(next.total, 2);
        assert!(next.has_updates);
        let expired = select(
            &scope,
            "current".into(),
            Some(Uuid::new_v4()),
            &desired,
            &["b".into(), "c".into()],
            0,
            20,
            &filters,
        )
        .unwrap();
        assert_eq!(expired.ids, vec!["c"]);
        let finished = select(
            &scope,
            "current".into(),
            first.snapshot_id,
            &[],
            &next.ids,
            0,
            20,
            &filters,
        )
        .unwrap();
        assert!(finished.ids.is_empty());
        assert_eq!(finished.total, 0);
        assert!(!finished.has_updates);
    }
}
