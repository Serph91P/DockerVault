use axum::Json;
use serde_json::{Value, json};

pub(super) async fn types() -> Json<Value> {
    Json(json!({
        "local": {
            "name": "Local/Network Path",
            "description": "Local filesystem or mounted network storage (NFS, SMB)",
            "required": ["base_path"],
            "optional": [],
        },
        "ssh": {
            "name": "SSH/SFTP",
            "description": "Remote server via SSH with rsync",
            "required": ["host", "username", "base_path"],
            "optional": ["port", "ssh_key_path", "password"],
        },
        "webdav": {
            "name": "WebDAV",
            "description": "WebDAV compatible storage (Nextcloud, ownCloud, etc.)",
            "required": ["webdav_url", "base_path"],
            "optional": ["username", "password"],
        },
        "s3": {
            "name": "S3 Compatible",
            "description": "AWS S3, MinIO, Backblaze B2, Wasabi, etc.",
            "required": ["s3_bucket", "s3_access_key", "s3_secret_key"],
            "optional": ["s3_region", "s3_endpoint_url", "base_path"],
        },
        "ftp": {
            "name": "FTP (unencrypted)",
            "description": "Plain FTP server; use SSH/SFTP for secure transport. FTPS is not supported.",
            "required": ["host", "username", "password", "base_path"],
            "optional": ["port"],
        },
        "rclone": {
            "name": "Rclone",
            "description": "Any rclone-supported backend (40+ providers)",
            "required": ["rclone_remote", "base_path"],
            "optional": [],
        },
    }))
}
