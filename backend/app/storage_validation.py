"""Provider-specific configuration constraints, including legacy repair hints."""

from urllib.parse import urlparse

STORAGE_REQUIRED = {
    "local": ["base_path"],
    "ssh": ["host", "username", "base_path"],
    "sftp": ["host", "username", "base_path"],
    "webdav": ["webdav_url", "base_path"],
    "s3": ["s3_bucket", "s3_access_key", "s3_secret_key"],
    "ftp": ["host", "username", "password", "base_path"],
    "rclone": ["rclone_remote", "base_path"],
}


def storage_errors(values) -> list[str]:
    errors = []
    kind = values.get("storage_type")
    if kind not in STORAGE_REQUIRED:
        return [
            "Unknown storage type; select a supported provider to repair this record"
        ]
    for field in ["name", *STORAGE_REQUIRED[kind]]:
        value = values.get(field)
        if not isinstance(value, str) or not value.strip():
            errors.append(
                f"{field} is required for {kind}; edit this storage to repair it"
            )
    if values.get("enabled") is None:
        errors.append("enabled must be true or false")
    port = values.get("port")
    if port is not None and (
        not isinstance(port, int) or isinstance(port, bool) or not 1 <= port <= 65535
    ):
        errors.append("port must be between 1 and 65535")
    for field in ("webdav_url", "s3_endpoint_url"):
        value = values.get(field)
        if value and (
            (field == "webdav_url" and kind == "webdav")
            or (field == "s3_endpoint_url" and kind == "s3")
        ):
            try:
                parsed = urlparse(value)
                valid = parsed.scheme in ("http", "https") and bool(parsed.hostname)
                # urlparse defers invalid/non-numeric port errors until access.
                parsed.port
            except ValueError:
                valid = False
            if not valid:
                errors.append(
                    f"{field} must be an http(s) URL with a valid hostname and port"
                )
    return errors


def stored_storage_errors(storage) -> list[str]:
    return storage_errors(
        {
            column.name: getattr(storage, column.name)
            for column in storage.__table__.columns
        }
    )
