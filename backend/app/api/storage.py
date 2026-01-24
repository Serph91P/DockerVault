"""Remote Storage API endpoints"""

from fastapi import APIRouter, HTTPException, Depends
from pydantic import BaseModel
from typing import Optional, List, Dict, Any
from enum import Enum

from ..remote_storage import (
    StorageType, StorageConfig, storage_manager,
    RemoteStorageManager
)
from ..database import get_db, RemoteStorage as RemoteStorageModel
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy import select

router = APIRouter(prefix="/storage", tags=["Remote Storage"])


class StorageCreate(BaseModel):
    """Create a new remote storage configuration"""
    name: str
    storage_type: StorageType
    enabled: bool = True
    
    # Connection settings
    host: Optional[str] = None
    port: Optional[int] = None
    username: Optional[str] = None
    password: Optional[str] = None
    
    # Path settings
    base_path: str = "/backups"
    
    # SSH/SFTP specific
    ssh_key_path: Optional[str] = None
    
    # S3 specific
    s3_bucket: Optional[str] = None
    s3_region: Optional[str] = None
    s3_access_key: Optional[str] = None
    s3_secret_key: Optional[str] = None
    s3_endpoint_url: Optional[str] = None
    
    # WebDAV specific
    webdav_url: Optional[str] = None
    
    # Rclone specific
    rclone_remote: Optional[str] = None


class StorageUpdate(BaseModel):
    """Update remote storage configuration"""
    name: Optional[str] = None
    enabled: Optional[bool] = None
    host: Optional[str] = None
    port: Optional[int] = None
    username: Optional[str] = None
    password: Optional[str] = None
    base_path: Optional[str] = None


class StorageResponse(BaseModel):
    """Remote storage response"""
    id: int
    name: str
    storage_type: str
    enabled: bool
    host: Optional[str]
    base_path: str
    created_at: str
    
    class Config:
        from_attributes = True


class StorageTestResult(BaseModel):
    """Result of connection test"""
    success: bool
    message: str


@router.get("", response_model=List[StorageResponse])
async def list_storage(db: AsyncSession = Depends(get_db)):
    """List all configured remote storage backends"""
    result = await db.execute(select(RemoteStorageModel))
    storages = result.scalars().all()
    return [StorageResponse(
        id=s.id,
        name=s.name,
        storage_type=s.storage_type,
        enabled=s.enabled,
        host=s.host,
        base_path=s.base_path,
        created_at=s.created_at.isoformat()
    ) for s in storages]


@router.get("/types")
async def list_storage_types():
    """List available storage types with their required fields"""
    return {
        "local": {
            "name": "Local/Network Path",
            "description": "Local filesystem or mounted network storage (NFS, SMB)",
            "required": ["base_path"],
            "optional": []
        },
        "ssh": {
            "name": "SSH/SFTP",
            "description": "Remote server via SSH with rsync",
            "required": ["host", "username", "base_path"],
            "optional": ["port", "ssh_key_path", "password"]
        },
        "webdav": {
            "name": "WebDAV",
            "description": "WebDAV compatible storage (Nextcloud, ownCloud, etc.)",
            "required": ["webdav_url", "base_path"],
            "optional": ["username", "password"]
        },
        "s3": {
            "name": "S3 Compatible",
            "description": "AWS S3, MinIO, Backblaze B2, Wasabi, etc.",
            "required": ["s3_bucket", "s3_access_key", "s3_secret_key"],
            "optional": ["s3_region", "s3_endpoint_url", "base_path"]
        },
        "ftp": {
            "name": "FTP/FTPS",
            "description": "FTP or FTPS server",
            "required": ["host", "username", "password", "base_path"],
            "optional": ["port"]
        },
        "rclone": {
            "name": "Rclone",
            "description": "Any rclone-supported backend (40+ providers)",
            "required": ["rclone_remote", "base_path"],
            "optional": []
        }
    }


@router.post("", response_model=StorageResponse)
async def create_storage(
    data: StorageCreate,
    db: AsyncSession = Depends(get_db)
):
    """Create a new remote storage configuration"""
    storage = RemoteStorageModel(
        name=data.name,
        storage_type=data.storage_type.value,
        enabled=data.enabled,
        host=data.host,
        port=data.port,
        username=data.username,
        password=data.password,
        base_path=data.base_path,
        ssh_key_path=data.ssh_key_path,
        s3_bucket=data.s3_bucket,
        s3_region=data.s3_region,
        s3_access_key=data.s3_access_key,
        s3_secret_key=data.s3_secret_key,
        s3_endpoint_url=data.s3_endpoint_url,
        webdav_url=data.webdav_url,
        rclone_remote=data.rclone_remote
    )
    
    db.add(storage)
    await db.commit()
    await db.refresh(storage)
    
    # Register with manager
    config = _db_to_config(storage)
    storage_manager.add_storage(config)
    
    return StorageResponse(
        id=storage.id,
        name=storage.name,
        storage_type=storage.storage_type,
        enabled=storage.enabled,
        host=storage.host,
        base_path=storage.base_path,
        created_at=storage.created_at.isoformat()
    )


@router.get("/{storage_id}", response_model=StorageResponse)
async def get_storage(storage_id: int, db: AsyncSession = Depends(get_db)):
    """Get a remote storage configuration"""
    storage = await db.get(RemoteStorageModel, storage_id)
    if not storage:
        raise HTTPException(status_code=404, detail="Storage not found")
    
    return StorageResponse(
        id=storage.id,
        name=storage.name,
        storage_type=storage.storage_type,
        enabled=storage.enabled,
        host=storage.host,
        base_path=storage.base_path,
        created_at=storage.created_at.isoformat()
    )


@router.put("/{storage_id}", response_model=StorageResponse)
async def update_storage(
    storage_id: int,
    data: StorageUpdate,
    db: AsyncSession = Depends(get_db)
):
    """Update a remote storage configuration"""
    storage = await db.get(RemoteStorageModel, storage_id)
    if not storage:
        raise HTTPException(status_code=404, detail="Storage not found")
    
    for field, value in data.model_dump(exclude_unset=True).items():
        setattr(storage, field, value)
    
    await db.commit()
    await db.refresh(storage)
    
    # Update manager
    storage_manager.remove_storage(storage_id)
    config = _db_to_config(storage)
    storage_manager.add_storage(config)
    
    return StorageResponse(
        id=storage.id,
        name=storage.name,
        storage_type=storage.storage_type,
        enabled=storage.enabled,
        host=storage.host,
        base_path=storage.base_path,
        created_at=storage.created_at.isoformat()
    )


@router.delete("/{storage_id}")
async def delete_storage(storage_id: int, db: AsyncSession = Depends(get_db)):
    """Delete a remote storage configuration"""
    storage = await db.get(RemoteStorageModel, storage_id)
    if not storage:
        raise HTTPException(status_code=404, detail="Storage not found")
    
    await db.delete(storage)
    await db.commit()
    
    storage_manager.remove_storage(storage_id)
    
    return {"message": "Storage deleted"}


@router.post("/{storage_id}/test", response_model=StorageTestResult)
async def test_storage(storage_id: int, db: AsyncSession = Depends(get_db)):
    """Test connection to remote storage"""
    storage = await db.get(RemoteStorageModel, storage_id)
    if not storage:
        raise HTTPException(status_code=404, detail="Storage not found")
    
    # Ensure backend is registered
    backend = storage_manager.get_backend(storage_id)
    if not backend:
        config = _db_to_config(storage)
        backend = storage_manager.add_storage(config)
    
    result = await backend.test_connection()
    return StorageTestResult(**result)


@router.get("/{storage_id}/files")
async def list_files(
    storage_id: int,
    path: str = "",
    db: AsyncSession = Depends(get_db)
):
    """List files in remote storage"""
    storage = await db.get(RemoteStorageModel, storage_id)
    if not storage:
        raise HTTPException(status_code=404, detail="Storage not found")
    
    backend = storage_manager.get_backend(storage_id)
    if not backend:
        config = _db_to_config(storage)
        backend = storage_manager.add_storage(config)
    
    files = await backend.list_files(path)
    return {"files": files, "path": path}


@router.post("/{storage_id}/sync/{backup_id}")
async def sync_backup_to_storage(
    storage_id: int,
    backup_id: int,
    db: AsyncSession = Depends(get_db)
):
    """Manually sync a specific backup to remote storage"""
    from ..database import Backup, BackupTarget
    from pathlib import Path
    
    storage = await db.get(RemoteStorageModel, storage_id)
    if not storage:
        raise HTTPException(status_code=404, detail="Storage not found")
    
    backup = await db.get(Backup, backup_id)
    if not backup:
        raise HTTPException(status_code=404, detail="Backup not found")
    
    if not backup.file_path:
        raise HTTPException(status_code=400, detail="Backup has no file")
    
    target = await db.get(BackupTarget, backup.target_id)
    
    backend = storage_manager.get_backend(storage_id)
    if not backend:
        config = _db_to_config(storage)
        backend = storage_manager.add_storage(config)
    
    local_path = Path(backup.file_path)
    remote_path = f"{target.name}/{local_path.name}"
    
    success = await backend.upload(local_path, remote_path)
    
    if success:
        return {"message": f"Backup synced to {storage.name}", "remote_path": remote_path}
    else:
        raise HTTPException(status_code=500, detail="Sync failed")


def _db_to_config(storage: RemoteStorageModel) -> StorageConfig:
    """Convert database model to StorageConfig"""
    return StorageConfig(
        id=storage.id,
        name=storage.name,
        storage_type=StorageType(storage.storage_type),
        enabled=storage.enabled,
        host=storage.host,
        port=storage.port,
        username=storage.username,
        password=storage.password,
        base_path=storage.base_path,
        ssh_key_path=storage.ssh_key_path,
        s3_bucket=storage.s3_bucket,
        s3_region=storage.s3_region,
        s3_access_key=storage.s3_access_key,
        s3_secret_key=storage.s3_secret_key,
        s3_endpoint_url=storage.s3_endpoint_url,
        webdav_url=storage.webdav_url,
        rclone_remote=storage.rclone_remote
    )
