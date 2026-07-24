use super::*;
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration as StdDuration;

const DATABASE_FILE: &str = "state.sqlite3";
const LEGACY_STATE_FILE: &str = "state.json";
const LEGACY_BACKUP_FILE: &str = "state.json.migrated.bak";
const BUSY_TIMEOUT_SECS: u64 = 30;
const SCHEMA_VERSION: i64 = 1;
const ORDER_STEP: i64 = 1024;

#[derive(Debug)]
struct StoreCache {
    data: AppData,
    revision: i64,
    load_error: Option<String>,
}

#[derive(Debug)]
pub struct AppStore {
    pub(crate) root_dir: PathBuf,
    cache: Mutex<StoreCache>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct PersistedCore {
    settings: SchedulerSettings,
    task_priorities: HashMap<String, u8>,
}

impl PersistedCore {
    fn from_data(data: &AppData) -> Self {
        Self {
            settings: data.settings.clone(),
            task_priorities: data.task_priorities.clone(),
        }
    }
}

impl AppStore {
    pub fn load(root_dir: PathBuf) -> Self {
        let loaded = open_database(&root_dir)
            .and_then(|mut connection| load_normalized_for_cache(&mut connection));
        let (data, revision, load_error) = match loaded {
            Ok((data, revision)) => (data, revision, None),
            Err(error) => (AppData::default(), 0, Some(error.to_string())),
        };
        Self {
            root_dir,
            cache: Mutex::new(StoreCache {
                data,
                revision,
                load_error,
            }),
        }
    }

    pub fn snapshot(&self) -> AppData {
        self.try_snapshot().unwrap_or_else(|_| {
            let mut cache = self.cache.lock().expect("store lock");
            cache.data.lane_status = compute_lane_status(&cache.data, Utc::now());
            cache.data.clone()
        })
    }

    pub fn try_snapshot(&self) -> Result<AppData, SchedulerError> {
        let mut cache = self.cache.lock().expect("store lock");
        let loaded = open_database(&self.root_dir).and_then(|mut connection| {
            load_consistent_snapshot(&mut connection, cache.revision, cache.load_error.is_some())
        });
        let (latest, revision) = match loaded {
            Ok(loaded) => loaded,
            Err(error) => {
                cache.load_error = Some(error.to_string());
                return Err(error);
            }
        };
        if let Some(latest) = latest {
            cache.data = latest;
        }
        cache.revision = revision;
        cache.load_error = None;
        cache.data.lane_status = compute_lane_status(&cache.data, Utc::now());
        Ok(cache.data.clone())
    }

    pub fn assets_dir(&self) -> PathBuf {
        self.root_dir.join("role-media")
    }

    pub fn imagegen_dir(&self) -> PathBuf {
        self.root_dir.join("imagegen")
    }

    pub fn mutate<F, T>(&self, mutate: F) -> Result<T, SchedulerError>
    where
        F: FnOnce(&mut AppData) -> Result<T, SchedulerError>,
    {
        let mut cache = self.cache.lock().expect("store lock");
        let mut connection = open_database(&self.root_dir)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        let revision = read_revision(&transaction)?;
        let previous = if revision != cache.revision || cache.load_error.is_some() {
            Cow::Owned(load_data_raw(&transaction)?)
        } else {
            Cow::Borrowed(&cache.data)
        };
        let mut next = previous.as_ref().clone();
        normalize_loaded_app_data(&mut next);
        let result = mutate(&mut next)?;
        let changed = persist_changes(&transaction, previous.as_ref(), &next)?;
        let next_revision = if changed {
            let next_revision = revision.saturating_add(1);
            transaction
                .execute(
                    "UPDATE app_state SET revision = ?1 WHERE id = 1",
                    params![next_revision],
                )
                .map_err(sqlite_error)?;
            next_revision
        } else {
            revision
        };
        transaction.commit().map_err(sqlite_error)?;

        cache.data = next;
        cache.revision = next_revision;
        cache.load_error = None;
        Ok(result)
    }

    /// SQLite 内部单调递增版本号；前端轮询只读取这一行，不再 stat 或解析整份状态。
    pub fn state_signature(&self) -> String {
        Connection::open(self.root_dir.join(DATABASE_FILE))
            .map_err(sqlite_error)
            .and_then(|connection| {
                connection
                    .busy_timeout(StdDuration::from_secs(BUSY_TIMEOUT_SECS))
                    .map_err(sqlite_error)?;
                Ok(connection)
            })
            .and_then(|connection| read_revision(&connection))
            .map(|revision| format!("sqlite:{revision}"))
            .unwrap_or_else(|_| "sqlite:0".to_string())
    }
}

fn sqlite_error(error: rusqlite::Error) -> SchedulerError {
    SchedulerError::Io(format!("SQLite 错误：{error}"))
}

fn serialize_json<T: Serialize>(value: &T) -> Result<String, SchedulerError> {
    serde_json::to_string(value).map_err(|error| SchedulerError::Io(error.to_string()))
}

fn deserialize_json<T: DeserializeOwned>(value: String) -> Result<T, SchedulerError> {
    serde_json::from_str(&value).map_err(|error| SchedulerError::Io(error.to_string()))
}

fn open_database(root_dir: &Path) -> Result<Connection, SchedulerError> {
    fs::create_dir_all(root_dir).map_err(|error| SchedulerError::Io(error.to_string()))?;
    let mut connection = Connection::open(root_dir.join(DATABASE_FILE)).map_err(sqlite_error)?;
    connection
        .busy_timeout(StdDuration::from_secs(BUSY_TIMEOUT_SECS))
        .map_err(sqlite_error)?;
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(sqlite_error)?;
    connection
        .pragma_update(None, "synchronous", "NORMAL")
        .map_err(sqlite_error)?;
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(sqlite_error)?;
    ensure_schema(&connection)?;
    initialize_if_needed(&mut connection, root_dir)?;
    Ok(connection)
}

fn ensure_schema(connection: &Connection) -> Result<(), SchedulerError> {
    let schema_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(sqlite_error)?;
    if schema_version > SCHEMA_VERSION {
        return Err(SchedulerError::Io(format!(
            "状态数据库版本 {schema_version} 高于当前支持版本 {SCHEMA_VERSION}，请升级应用后再试"
        )));
    }
    connection
        .execute_batch(
            "
            CREATE TABLE IF NOT EXISTS app_state (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                revision INTEGER NOT NULL,
                data_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS tasks (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                id TEXT NOT NULL UNIQUE,
                position INTEGER NOT NULL,
                data_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS assets (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                id TEXT NOT NULL UNIQUE,
                position INTEGER NOT NULL,
                data_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS roles (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                id TEXT NOT NULL UNIQUE,
                position INTEGER NOT NULL,
                data_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS logs (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                id TEXT NOT NULL UNIQUE,
                position INTEGER NOT NULL,
                data_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS imagegen_history (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                id TEXT NOT NULL UNIQUE,
                position INTEGER NOT NULL,
                data_json TEXT NOT NULL
            );
            ",
        )
        .map_err(sqlite_error)?;
    if schema_version < SCHEMA_VERSION {
        connection
            .pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(sqlite_error)?;
    }
    Ok(())
}

fn initialize_if_needed(
    connection: &mut Connection,
    root_dir: &Path,
) -> Result<(), SchedulerError> {
    if read_revision_optional(connection)?.is_some() {
        return Ok(());
    }

    connection
        .pragma_update(None, "synchronous", "FULL")
        .map_err(sqlite_error)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sqlite_error)?;
    if read_revision_optional(&transaction)?.is_some() {
        transaction.commit().map_err(sqlite_error)?;
        connection
            .pragma_update(None, "synchronous", "NORMAL")
            .map_err(sqlite_error)?;
        return Ok(());
    }

    let legacy_path = root_dir.join(LEGACY_STATE_FILE);
    let legacy_exists = legacy_path.exists();
    let data = load_app_data_from_disk(root_dir)?;
    if legacy_exists {
        create_legacy_backup(root_dir)?;
    }
    insert_all(&transaction, &data)?;
    transaction
        .execute(
            "INSERT INTO app_state (id, revision, data_json) VALUES (1, 1, ?1)",
            params![serialize_json(&PersistedCore::from_data(&data))?],
        )
        .map_err(sqlite_error)?;
    transaction.commit().map_err(sqlite_error)?;
    connection
        .execute_batch("PRAGMA wal_checkpoint(FULL);")
        .map_err(sqlite_error)?;

    if legacy_exists {
        fs::remove_file(legacy_path).map_err(|error| SchedulerError::Io(error.to_string()))?;
        sync_directory(root_dir)?;
    }
    connection
        .pragma_update(None, "synchronous", "NORMAL")
        .map_err(sqlite_error)?;
    Ok(())
}

fn create_legacy_backup(root_dir: &Path) -> Result<(), SchedulerError> {
    let source = root_dir.join(LEGACY_STATE_FILE);
    let default_backup = root_dir.join(LEGACY_BACKUP_FILE);
    let backup = if default_backup.exists() {
        root_dir.join(format!(
            "state.json.migrated.{}.bak",
            Uuid::new_v4().simple()
        ))
    } else {
        default_backup
    };
    let temporary = root_dir.join(format!(
        "{LEGACY_BACKUP_FILE}.tmp.{}.{}",
        std::process::id(),
        Uuid::new_v4().simple()
    ));
    fs::copy(&source, &temporary).map_err(|error| SchedulerError::Io(error.to_string()))?;
    File::open(&temporary)
        .and_then(|file| file.sync_all())
        .map_err(|error| SchedulerError::Io(error.to_string()))?;
    if let Err(error) = fs::rename(&temporary, &backup) {
        let _ = fs::remove_file(&temporary);
        return Err(SchedulerError::Io(error.to_string()));
    }
    sync_directory(root_dir)?;
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), SchedulerError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| SchedulerError::Io(error.to_string()))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), SchedulerError> {
    Ok(())
}

fn read_revision_optional(connection: &Connection) -> Result<Option<i64>, SchedulerError> {
    connection
        .query_row("SELECT revision FROM app_state WHERE id = 1", [], |row| {
            row.get(0)
        })
        .optional()
        .map_err(sqlite_error)
}

fn read_revision(connection: &Connection) -> Result<i64, SchedulerError> {
    read_revision_optional(connection)?
        .ok_or_else(|| SchedulerError::Io("SQLite 状态尚未初始化".to_string()))
}

fn load_normalized_for_cache(
    connection: &mut Connection,
) -> Result<(AppData, i64), SchedulerError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sqlite_error)?;
    let revision = read_revision(&transaction)?;
    let raw = load_data_raw(&transaction)?;
    let mut normalized = raw.clone();
    normalize_loaded_app_data(&mut normalized);
    let changed = persist_changes(&transaction, &raw, &normalized)?;
    let revision = if changed {
        let next_revision = revision.saturating_add(1);
        transaction
            .execute(
                "UPDATE app_state SET revision = ?1 WHERE id = 1",
                params![next_revision],
            )
            .map_err(sqlite_error)?;
        next_revision
    } else {
        revision
    };
    transaction.commit().map_err(sqlite_error)?;
    Ok((normalized, revision))
}

fn load_consistent_snapshot(
    connection: &mut Connection,
    cached_revision: i64,
    force_reload: bool,
) -> Result<(Option<AppData>, i64), SchedulerError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(sqlite_error)?;
    let revision = read_revision(&transaction)?;
    let data = if force_reload || revision != cached_revision {
        let mut data = load_data_raw(&transaction)?;
        normalize_loaded_app_data(&mut data);
        Some(data)
    } else {
        None
    };
    transaction.commit().map_err(sqlite_error)?;
    Ok((data, revision))
}

fn load_data_raw(connection: &Connection) -> Result<AppData, SchedulerError> {
    let core: PersistedCore = connection
        .query_row("SELECT data_json FROM app_state WHERE id = 1", [], |row| {
            row.get::<_, String>(0)
        })
        .map_err(sqlite_error)
        .and_then(deserialize_json)?;
    let data = AppData {
        settings: core.settings,
        assets: load_rows(connection, "assets")?,
        roles: load_rows(connection, "roles")?,
        tasks: load_rows(connection, "tasks")?,
        task_priorities: core.task_priorities,
        logs: load_rows(connection, "logs")?,
        imagegen_history: load_rows(connection, "imagegen_history")?,
        asset_hash_index: HashMap::new(),
        lane_status: vec![],
    };
    Ok(data)
}

fn load_rows<T: DeserializeOwned>(
    connection: &Connection,
    table: &str,
) -> Result<Vec<T>, SchedulerError> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT data_json FROM {table} ORDER BY position ASC, seq ASC"
        ))
        .map_err(sqlite_error)?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(sqlite_error)?;
    let mut values = Vec::new();
    for row in rows {
        values.push(deserialize_json(row.map_err(sqlite_error)?)?);
    }
    Ok(values)
}

fn insert_all(transaction: &Transaction<'_>, data: &AppData) -> Result<(), SchedulerError> {
    insert_rows(transaction, "tasks", data.tasks.iter(), |item| &item.id)?;
    insert_rows(transaction, "assets", data.assets.iter(), |item| &item.id)?;
    insert_rows(transaction, "roles", data.roles.iter(), |item| &item.id)?;
    insert_rows(transaction, "logs", data.logs.iter(), |item| &item.id)?;
    insert_rows(
        transaction,
        "imagegen_history",
        data.imagegen_history.iter(),
        |item| &item.id,
    )?;
    Ok(())
}

fn insert_rows<'a, T, I, F>(
    transaction: &Transaction<'_>,
    table: &str,
    items: I,
    id_of: F,
) -> Result<(), SchedulerError>
where
    T: Serialize + 'a,
    I: IntoIterator<Item = &'a T>,
    F: Fn(&T) -> &str,
{
    let mut statement = transaction
        .prepare(&format!(
            "INSERT INTO {table} (id, position, data_json) VALUES (?1, ?2, ?3)"
        ))
        .map_err(sqlite_error)?;
    for (position, item) in items.into_iter().enumerate() {
        let position = i64::try_from(position)
            .ok()
            .and_then(|value| value.checked_mul(ORDER_STEP))
            .ok_or_else(|| SchedulerError::Io("实体排序位置超出范围".to_string()))?;
        statement
            .execute(params![id_of(item), position, serialize_json(item)?])
            .map_err(sqlite_error)?;
    }
    Ok(())
}

fn persist_changes(
    transaction: &Transaction<'_>,
    previous: &AppData,
    next: &AppData,
) -> Result<bool, SchedulerError> {
    let mut changed = sync_rows(transaction, "tasks", &previous.tasks, &next.tasks, |item| {
        &item.id
    })?;
    changed |= sync_rows(
        transaction,
        "assets",
        &previous.assets,
        &next.assets,
        |item| &item.id,
    )?;
    changed |= sync_rows(transaction, "roles", &previous.roles, &next.roles, |item| {
        &item.id
    })?;
    changed |= sync_rows(transaction, "logs", &previous.logs, &next.logs, |item| {
        &item.id
    })?;
    changed |= sync_rows(
        transaction,
        "imagegen_history",
        &previous.imagegen_history,
        &next.imagegen_history,
        |item| &item.id,
    )?;
    let previous_core = PersistedCore::from_data(previous);
    let next_core = PersistedCore::from_data(next);
    if previous_core != next_core {
        transaction
            .execute(
                "UPDATE app_state SET data_json = ?1 WHERE id = 1",
                params![serialize_json(&next_core)?],
            )
            .map_err(sqlite_error)?;
        changed = true;
    }
    Ok(changed)
}

fn sync_rows<T, F>(
    transaction: &Transaction<'_>,
    table: &str,
    previous: &[T],
    next: &[T],
    id_of: F,
) -> Result<bool, SchedulerError>
where
    T: Serialize + PartialEq,
    F: Fn(&T) -> &str,
{
    let previous_by_id: HashMap<&str, &T> =
        previous.iter().map(|item| (id_of(item), item)).collect();
    let next_ids: HashSet<&str> = next.iter().map(&id_of).collect();
    let stored_positions = load_stored_positions(transaction, table)?;
    let stored_position_by_id: HashMap<&str, i64> = stored_positions
        .iter()
        .map(|(id, position)| (id.as_str(), *position))
        .collect();
    let desired_positions = plan_positions(
        &stored_positions,
        &stored_position_by_id,
        &next_ids,
        next,
        &id_of,
    )?;
    let mut changed = false;
    let mut delete = transaction
        .prepare(&format!("DELETE FROM {table} WHERE id = ?1"))
        .map_err(sqlite_error)?;
    for item in previous {
        let id = id_of(item);
        if !next_ids.contains(id) {
            delete.execute(params![id]).map_err(sqlite_error)?;
            changed = true;
        }
    }

    let mut update_data = transaction
        .prepare(&format!("UPDATE {table} SET data_json = ?2 WHERE id = ?1"))
        .map_err(sqlite_error)?;
    let mut update_position = transaction
        .prepare(&format!("UPDATE {table} SET position = ?2 WHERE id = ?1"))
        .map_err(sqlite_error)?;
    for item in next {
        let id = id_of(item);
        match previous_by_id.get(id) {
            Some(previous_item) => {
                if *previous_item != item {
                    update_data
                        .execute(params![id, serialize_json(item)?])
                        .map_err(sqlite_error)?;
                    changed = true;
                }
                let stored_position = stored_position_by_id.get(id).copied();
                let desired_position = desired_positions[id];
                if stored_position != Some(desired_position) {
                    update_position
                        .execute(params![id, desired_position])
                        .map_err(sqlite_error)?;
                    changed = true;
                }
            }
            None => {}
        }
    }

    let mut insert = transaction
        .prepare(&format!(
            "INSERT INTO {table} (id, position, data_json) VALUES (?1, ?2, ?3)"
        ))
        .map_err(sqlite_error)?;
    for item in next {
        let id = id_of(item);
        if !previous_by_id.contains_key(id) {
            insert
                .execute(params![id, desired_positions[id], serialize_json(item)?])
                .map_err(sqlite_error)?;
            changed = true;
        }
    }
    Ok(changed)
}

fn load_stored_positions(
    transaction: &Transaction<'_>,
    table: &str,
) -> Result<Vec<(String, i64)>, SchedulerError> {
    let mut statement = transaction
        .prepare(&format!(
            "SELECT id, position FROM {table} ORDER BY position ASC, seq ASC"
        ))
        .map_err(sqlite_error)?;
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(sqlite_error)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(sqlite_error)
}

fn plan_positions<'a, T, F>(
    stored: &[(String, i64)],
    stored_by_id: &HashMap<&str, i64>,
    next_ids: &HashSet<&str>,
    next: &'a [T],
    id_of: &F,
) -> Result<HashMap<&'a str, i64>, SchedulerError>
where
    F: Fn(&T) -> &str,
{
    let next_existing = next
        .iter()
        .filter_map(|item| {
            let id = id_of(item);
            stored_by_id.contains_key(id).then_some(id)
        })
        .collect::<Vec<_>>();
    let stored_remaining = stored
        .iter()
        .filter_map(|(id, _)| next_ids.contains(id.as_str()).then_some(id.as_str()))
        .collect::<Vec<_>>();
    if next_existing != stored_remaining {
        return sequential_positions(next, id_of);
    }

    let mut desired = next
        .iter()
        .filter_map(|item| {
            let id = id_of(item);
            stored_by_id.get(id).map(|position| (id, *position))
        })
        .collect::<HashMap<_, _>>();
    let mut index = 0;
    while index < next.len() {
        if desired.contains_key(id_of(&next[index])) {
            index += 1;
            continue;
        }
        let start = index;
        while index < next.len() && !desired.contains_key(id_of(&next[index])) {
            index += 1;
        }
        let end = index;
        let count = end - start;
        let left = start
            .checked_sub(1)
            .and_then(|left_index| desired.get(id_of(&next[left_index])).copied());
        let right = (end < next.len())
            .then(|| desired.get(id_of(&next[end])).copied())
            .flatten();
        let Some(positions) = positions_between(left, right, count) else {
            return sequential_positions(next, id_of);
        };
        for (offset, position) in positions.into_iter().enumerate() {
            desired.insert(id_of(&next[start + offset]), position);
        }
    }
    Ok(desired)
}

fn sequential_positions<'a, T, F>(
    items: &'a [T],
    id_of: &F,
) -> Result<HashMap<&'a str, i64>, SchedulerError>
where
    F: Fn(&T) -> &str,
{
    let positions = sequential_position_values(items.len())?;
    Ok(items
        .iter()
        .zip(positions)
        .map(|(item, position)| (id_of(item), position))
        .collect())
}

fn sequential_position_values(count: usize) -> Result<Vec<i64>, SchedulerError> {
    (0..count)
        .map(|index| {
            i64::try_from(index)
                .ok()
                .and_then(|value| value.checked_mul(ORDER_STEP))
                .ok_or_else(|| SchedulerError::Io("实体排序位置超出范围".to_string()))
        })
        .collect()
}

fn positions_between(left: Option<i64>, right: Option<i64>, count: usize) -> Option<Vec<i64>> {
    let count_i64 = i64::try_from(count).ok()?;
    match (left, right) {
        (None, None) => sequential_position_values(count).ok(),
        (Some(left), None) => (1..=count_i64)
            .map(|offset| left.checked_add(ORDER_STEP.checked_mul(offset)?))
            .collect(),
        (None, Some(right)) => (0..count_i64)
            .map(|offset| {
                let distance = ORDER_STEP.checked_mul(count_i64.checked_sub(offset)?)?;
                right.checked_sub(distance)
            })
            .collect(),
        (Some(left), Some(right)) => {
            let gap = right.checked_sub(left)?;
            if gap <= count_i64 {
                return None;
            }
            let divisor = count_i64.checked_add(1)?;
            (1..=count_i64)
                .map(|offset| {
                    let scaled = i128::from(gap) * i128::from(offset) / i128::from(divisor);
                    i64::try_from(i128::from(left) + scaled).ok()
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_task(title: &str) -> ScheduledTask {
        ScheduledTask::from(TaskDraft {
            title: title.to_string(),
            prompt: format!("prompt {title}"),
            image_asset_ids: vec![],
            audio_asset_ids: vec![],
            role_ids: vec![],
            manual_mention_ids: vec![],
            auto_match_roles: false,
            params: VideoParams::default(),
            scheduled_at: None,
            temp_image_asset_ids: vec![],
            temp_image_paths: vec![],
            prompt_doc: None,
        })
    }

    #[test]
    fn multi_table_read_and_revision_share_one_sqlite_snapshot() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = AppStore::load(temp.path().to_path_buf());
        store
            .mutate(|data| {
                data.tasks.push(test_task("before"));
                Ok(())
            })
            .expect("seed task");

        let mut reader = open_database(temp.path()).expect("open reader");
        let read_transaction = reader
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .expect("begin read transaction");
        let read_revision_before = read_revision(&read_transaction).expect("read revision");

        let writer = AppStore::load(temp.path().to_path_buf());
        writer
            .mutate(|data| {
                data.tasks[0].title = "after".to_string();
                Ok(())
            })
            .expect("concurrent writer");

        let data = load_data_raw(&read_transaction).expect("read stable snapshot");
        let read_revision_after = read_revision(&read_transaction).expect("read revision again");
        read_transaction.commit().expect("commit read transaction");

        assert_eq!(data.tasks[0].title, "before");
        assert_eq!(read_revision_after, read_revision_before);
        assert_eq!(writer.snapshot().tasks[0].title, "after");
    }
}
