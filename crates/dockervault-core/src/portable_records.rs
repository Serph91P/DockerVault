//! Strict portable column DTOs. These are not a validated recovery state: the
//! service must still check manifests, references, paths, cron and credential custody.
use crate::{
    model::{BackupStatus, BackupType},
    timestamp::Timestamp,
};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

pub const TABLE_ORDER: [&str; 9] = [
    "retention_policies",
    "schedules",
    "remote_storages",
    "encryption_config",
    "app_settings",
    "backup_targets",
    "backups",
    "backup_schedules",
    "backup_storage_syncs",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordError;
impl std::fmt::Display for RecordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Invalid or incompatible recovery state")
    }
}
impl std::error::Error for RecordError {}
fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(d)
}
fn text<'de, D: Deserializer<'de>, const MAX: usize>(d: D) -> Result<String, D::Error> {
    let s = String::deserialize(d)?;
    if s.chars().count() > MAX {
        return Err(serde::de::Error::custom("Invalid field length"));
    }
    Ok(s)
}
fn nullable_text<'de, D: Deserializer<'de>, const MAX: usize>(
    d: D,
) -> Result<Option<String>, D::Error> {
    let s = Option::<String>::deserialize(d)?;
    if s.as_ref().is_some_and(|s| s.chars().count() > MAX) {
        return Err(serde::de::Error::custom("Invalid field length"));
    }
    Ok(s)
}
fn id<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    let value = i64::deserialize(d)?;
    if value <= 0 {
        return Err(serde::de::Error::custom("Invalid record ID"));
    }
    Ok(value)
}
fn rows<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Vec<T>, D::Error> {
    let rows = Vec::<T>::deserialize(d)?;
    if rows.len() > 100000 {
        return Err(serde::de::Error::custom("Too many records"));
    }
    Ok(rows)
}

// No Debug implementation: these records can contain decrypted portable credentials.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableTables {
    #[serde(deserialize_with = "rows")]
    pub retention_policies: Vec<RetentionPolicy>,
    #[serde(deserialize_with = "rows")]
    pub schedules: Vec<Schedule>,
    #[serde(deserialize_with = "rows")]
    pub remote_storages: Vec<RemoteStorage>,
    #[serde(deserialize_with = "rows")]
    pub encryption_config: Vec<EncryptionConfig>,
    #[serde(deserialize_with = "rows")]
    pub app_settings: Vec<AppSetting>,
    #[serde(deserialize_with = "rows")]
    pub backup_targets: Vec<BackupTarget>,
    #[serde(deserialize_with = "rows")]
    pub backups: Vec<Backup>,
    #[serde(deserialize_with = "rows")]
    pub backup_schedules: Vec<BackupSchedule>,
    #[serde(deserialize_with = "rows")]
    pub backup_storage_syncs: Vec<BackupStorageSync>,
}
impl PortableTables {
    pub fn parse(value: Value) -> Result<Self, RecordError> {
        serde_json::from_value(value).map_err(|_| RecordError)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetentionPolicy {
    #[serde(deserialize_with = "id")]
    pub id: i64,
    #[serde(deserialize_with = "text::<_,255>")]
    pub name: String,
    #[serde(deserialize_with = "nullable")]
    pub description: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub keep_last: Option<i64>,
    #[serde(deserialize_with = "nullable")]
    pub keep_daily: Option<i64>,
    #[serde(deserialize_with = "nullable")]
    pub keep_weekly: Option<i64>,
    #[serde(deserialize_with = "nullable")]
    pub keep_monthly: Option<i64>,
    #[serde(deserialize_with = "nullable")]
    pub keep_yearly: Option<i64>,
    #[serde(deserialize_with = "nullable")]
    pub max_age_days: Option<i64>,
    #[serde(deserialize_with = "nullable")]
    pub created_at: Option<Timestamp>,
    #[serde(deserialize_with = "nullable")]
    pub updated_at: Option<Timestamp>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Schedule {
    #[serde(deserialize_with = "id")]
    pub id: i64,
    #[serde(deserialize_with = "text::<_,255>")]
    pub name: String,
    #[serde(deserialize_with = "text::<_,100>")]
    pub cron_expression: String,
    #[serde(deserialize_with = "nullable")]
    pub description: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub enabled: Option<bool>,
    #[serde(deserialize_with = "nullable")]
    pub created_at: Option<Timestamp>,
    #[serde(deserialize_with = "nullable")]
    pub updated_at: Option<Timestamp>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteStorage {
    #[serde(deserialize_with = "id")]
    pub id: i64,
    #[serde(deserialize_with = "text::<_,255>")]
    pub name: String,
    #[serde(deserialize_with = "text::<_,50>")]
    pub storage_type: String,
    #[serde(deserialize_with = "nullable")]
    pub enabled: Option<bool>,
    #[serde(deserialize_with = "nullable_text::<_,255>")]
    pub host: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub port: Option<i64>,
    #[serde(deserialize_with = "nullable_text::<_,255>")]
    pub username: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub password: Option<String>,
    #[serde(deserialize_with = "nullable_text::<_,1024>")]
    pub base_path: Option<String>,
    #[serde(deserialize_with = "nullable_text::<_,1024>")]
    pub ssh_key_path: Option<String>,
    #[serde(deserialize_with = "nullable_text::<_,255>")]
    pub s3_bucket: Option<String>,
    #[serde(deserialize_with = "nullable_text::<_,50>")]
    pub s3_region: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub s3_access_key: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub s3_secret_key: Option<String>,
    #[serde(deserialize_with = "nullable_text::<_,1024>")]
    pub s3_endpoint_url: Option<String>,
    #[serde(deserialize_with = "nullable_text::<_,1024>")]
    pub webdav_url: Option<String>,
    #[serde(deserialize_with = "nullable_text::<_,255>")]
    pub rclone_remote: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub created_at: Option<Timestamp>,
    #[serde(deserialize_with = "nullable")]
    pub updated_at: Option<Timestamp>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EncryptionConfig {
    #[serde(deserialize_with = "id")]
    pub id: i64,
    pub public_key: String,
    #[serde(deserialize_with = "nullable")]
    pub key_created_at: Option<Timestamp>,
    #[serde(deserialize_with = "nullable")]
    pub encryption_enabled: Option<bool>,
    #[serde(deserialize_with = "nullable")]
    pub setup_completed: Option<bool>,
    #[serde(deserialize_with = "nullable")]
    pub created_at: Option<Timestamp>,
    #[serde(deserialize_with = "nullable")]
    pub updated_at: Option<Timestamp>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppSetting {
    #[serde(deserialize_with = "id")]
    pub id: i64,
    #[serde(deserialize_with = "text::<_,100>")]
    pub key: String,
    #[serde(deserialize_with = "nullable")]
    pub value: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub updated_at: Option<Timestamp>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupTarget {
    #[serde(deserialize_with = "id")]
    pub id: i64,
    #[serde(deserialize_with = "text::<_,255>")]
    pub name: String,
    #[serde(deserialize_with = "text::<_,50>")]
    pub target_type: String,
    #[serde(deserialize_with = "nullable_text::<_,255>")]
    pub container_id: Option<String>,
    #[serde(deserialize_with = "nullable_text::<_,255>")]
    pub container_name: Option<String>,
    #[serde(deserialize_with = "nullable_text::<_,255>")]
    pub volume_name: Option<String>,
    #[serde(deserialize_with = "nullable_text::<_,1024>")]
    pub host_path: Option<String>,
    #[serde(deserialize_with = "nullable_text::<_,255>")]
    pub stack_name: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub schedule_id: Option<i64>,
    #[serde(deserialize_with = "nullable_text::<_,100>")]
    pub schedule_cron: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub enabled: Option<bool>,
    #[serde(deserialize_with = "nullable")]
    pub removed_at: Option<Timestamp>,
    #[serde(deserialize_with = "nullable")]
    pub retention_policy_id: Option<i64>,
    pub dependencies: Value,
    pub selected_volumes: Value,
    pub include_paths: Value,
    pub exclude_paths: Value,
    pub per_volume_rules: Value,
    #[serde(deserialize_with = "nullable")]
    pub pre_backup_command: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub post_backup_command: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub stop_container: Option<bool>,
    #[serde(deserialize_with = "nullable")]
    pub compression_enabled: Option<bool>,
    #[serde(deserialize_with = "nullable")]
    pub sync_to_remote: Option<bool>,
    pub remote_storage_ids: Value,
    #[serde(deserialize_with = "nullable")]
    pub delete_local_after_sync: Option<bool>,
    #[serde(deserialize_with = "nullable")]
    pub created_at: Option<Timestamp>,
    #[serde(deserialize_with = "nullable")]
    pub updated_at: Option<Timestamp>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Backup {
    #[serde(deserialize_with = "id")]
    pub id: i64,
    pub target_id: i64,
    #[serde(deserialize_with = "nullable")]
    pub backup_type: Option<BackupType>,
    #[serde(deserialize_with = "nullable")]
    pub status: Option<BackupStatus>,
    #[serde(deserialize_with = "nullable_text::<_,1024>")]
    pub file_path: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub file_size: Option<i64>,
    #[serde(deserialize_with = "nullable_text::<_,64>")]
    pub checksum: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub encrypted: Option<bool>,
    #[serde(deserialize_with = "nullable_text::<_,1024>")]
    pub encryption_key_path: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub started_at: Option<Timestamp>,
    #[serde(deserialize_with = "nullable")]
    pub completed_at: Option<Timestamp>,
    #[serde(deserialize_with = "nullable")]
    pub duration_seconds: Option<i64>,
    #[serde(deserialize_with = "nullable")]
    pub error_message: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub retry_count: Option<i64>,
    pub backup_metadata: Value,
    #[serde(deserialize_with = "nullable")]
    pub created_at: Option<Timestamp>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupSchedule {
    #[serde(deserialize_with = "id")]
    pub id: i64,
    pub target_id: i64,
    #[serde(deserialize_with = "text::<_,100>")]
    pub cron_expression: String,
    #[serde(deserialize_with = "nullable")]
    pub next_run: Option<Timestamp>,
    #[serde(deserialize_with = "nullable")]
    pub last_run: Option<Timestamp>,
    #[serde(deserialize_with = "nullable")]
    pub enabled: Option<bool>,
    #[serde(deserialize_with = "nullable")]
    pub created_at: Option<Timestamp>,
    #[serde(deserialize_with = "nullable")]
    pub updated_at: Option<Timestamp>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupStorageSync {
    #[serde(deserialize_with = "id")]
    pub id: i64,
    pub backup_id: i64,
    pub storage_id: i64,
    #[serde(deserialize_with = "nullable_text::<_,1024>")]
    pub remote_path: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub synced_at: Option<Timestamp>,
    #[serde(deserialize_with = "nullable_text::<_,50>")]
    pub sync_status: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub error_message: Option<String>,
}
