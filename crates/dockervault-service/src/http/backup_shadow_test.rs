use super::{ArchiveReader, File, FsPath, MemberKind, Read, Seek, io, json, with_archive_data};
use crate::db::Database;
use crate::http::backups::{FROM, SELECT, copies, record};
use serde_json::Value;
use std::io::SeekFrom;
use std::path::Component;
use zeroize::Zeroizing;

#[tokio::test]
#[ignore = "requires the read-only synthetic Python package snapshot from native CI"]
async fn copied_python_catalog_and_archive() {
    assert!(!FsPath::new("/var/run/docker.sock").exists());
    let root = FsPath::new("/shadow");
    let db = Database::read_only(root.join("data/backup.db"))
        .await
        .unwrap();
    assert!(db.migration().is_none());
    let mut conn = db.pool().acquire().await.unwrap();
    let mut query = sqlx::QueryBuilder::new(format!("{SELECT}{FROM}"));
    let rows = query.build().fetch_all(&mut *conn).await.unwrap();
    assert_eq!(rows.len(), 1);
    let mut syncs = copies(&mut conn, &rows).await.unwrap().into_values();
    let catalog = record(
        &rows[0],
        syncs.next().unwrap(),
        false,
        FsPath::new("/backups"),
    )
    .unwrap();
    drop(conn);
    db.close().await;

    let [copy] = catalog["remote_sync_details"]
        .as_array()
        .unwrap()
        .as_slice()
    else {
        panic!("expected one retained LOCAL copy");
    };
    let key = copy["remote_path"].as_str().unwrap();
    assert!(key.ends_with(".tar.gz.enc"));
    assert!(
        FsPath::new(key)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
    );
    let archive = root.join("archives").join(key);
    let name = archive.to_str().unwrap();
    let identity = Zeroizing::new(std::fs::read_to_string(root.join("identity")).unwrap());
    let mut source = File::open(&archive).unwrap();
    let sidecar = File::open(super::key_sidecar_path(name)).unwrap();
    let members = with_archive_data(
        &mut source,
        name,
        FsPath::new("/tmp"),
        Some((sidecar, identity.trim())),
        None,
        &|| false,
        |tar, _| {
            let entries = ArchiveReader::new(&mut *tar)?.collect::<io::Result<Vec<_>>>()?;
            let mut members = Vec::new();
            for member in entries {
                let is_dir = member.kind == MemberKind::Directory;
                let bytes = if is_dir {
                    None
                } else {
                    assert_eq!(member.kind, MemberKind::File);
                    assert!(member.size <= 4096, "not the tiny synthetic fixture");
                    let mut bytes = vec![0; member.size as usize];
                    tar.seek(SeekFrom::Start(member.data_offset))?;
                    tar.read_exact(&mut bytes)?;
                    Some(bytes)
                };
                members.push(json!([
                    String::from_utf8(member.name).unwrap(),
                    is_dir,
                    member.size,
                    bytes
                ]));
            }
            Ok(members)
        },
    )
    .unwrap();
    let fields = [
        "id",
        "target_id",
        "target_name",
        "backup_type",
        "status",
        "file_path",
        "file_size",
        "checksum",
        "encrypted",
        "remote_synced",
    ];
    let fields: Vec<&Value> = fields.iter().map(|&field| &catalog[field]).collect();
    println!(
        "shadow={}",
        json!({"catalog": fields, "copy": [&copy["storage_id"], &copy["status"], &copy["remote_path"]], "members": members})
    );
}
