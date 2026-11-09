"""Typed rclone dependency closure inside the encrypted recovery payload.

Native provider metadata is an option inventory, NOT a safety classification.
SSH private keys and command/ambient-auth dependencies are explicitly excluded.
"""

import asyncio
import base64
import contextlib
import configparser
import fcntl
import http.client
import csv
import functools
import hashlib
import io
import json
import os
import re
import secrets
import shutil
import socket
import stat
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import NoReturn

from app.credential_encryption import decrypt_value, encrypt_value
from app.rclone_paths import NAME as _NAME, parse as _parse_reference, environment_name

MAX_MATERIAL_BYTES = 1024 * 1024
MAX_REMOTES = 256
_WRAPPERS = {"alias", "crypt", "chunker", "compress", "hasher", "archive", "cache"}
_FILE_OPTIONS = {
    "drive": {"service_account_file"},
    "google cloud storage": {"service_account_file"},
    "s3": {"shared_credentials_file"},
    "box": {"box_config_file"},
    "azureblob": {"client_certificate_path", "service_principal_file"},
    "azurefiles": {"client_certificate_path", "service_principal_file"},
    "oracleobjectstorage": {"config_file", "sse_customer_key_file"},
    "sftp": {"known_hosts_file", "pubkey_file"},
}
_SSH_PRIVATE = {"key_pem", "key_file", "key_file_pass", "key_use_agent"}


class RcloneRecoveryError(ValueError):
    """Safe error without native output, credential values or source paths."""


def _invalid() -> NoReturn:
    raise RcloneRecoveryError(
        "Invalid or unavailable portable rclone configuration"
    ) from None


def _environment():
    return {
        "PATH": os.defpath + ":/usr/local/bin",
        "HOME": "/nonexistent",
        "LANG": "C.UTF-8",
    }


def _binary():
    binary = shutil.which("rclone")
    if not binary:
        _invalid()
    return binary


@functools.lru_cache(maxsize=1)
def provider_options():
    try:
        result = subprocess.run(
            [
                _binary(),
                "--config",
                os.devnull,
                "--ask-password=false",
                "config",
                "providers",
            ],
            env=_environment(),
            stdin=subprocess.DEVNULL,
            capture_output=True,
            timeout=15,
        )
        if result.returncode or len(result.stdout) > 8 * 1024 * 1024:
            _invalid()
        providers = json.loads(result.stdout)
        return {
            provider["Name"]: {option["Name"] for option in provider["Options"]}
            for provider in providers
        }
    except (OSError, ValueError, KeyError, subprocess.TimeoutExpired):
        _invalid()


def _read_material(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        info = os.fstat(descriptor)
        if not stat.S_ISREG(info.st_mode) or info.st_size > MAX_MATERIAL_BYTES:
            _invalid()
        with os.fdopen(descriptor, "rb", closefd=False) as stream:
            data = stream.read(MAX_MATERIAL_BYTES + 1)
        if not data or len(data) > MAX_MATERIAL_BYTES:
            _invalid()
        return data
    finally:
        os.close(descriptor)


def _space_list(value):
    # rclone1.72.1 fs/config_list.go: encoding/csv with delimiter space, not shell quoting.
    if "\n" in value or "\r" in value:
        _invalid()
    # Python's CSV parser accepts bare quotes that Go rejects; enforce Go's field grammar.
    if not re.fullmatch(
        r'(?:"(?:[^\"]|\"\")*"|[^\" ]*)(?: (?:"(?:[^\"]|\"\")*"|[^\" ]*))*', value
    ):
        _invalid()
    try:
        return next(csv.reader(io.StringIO(value), delimiter=" ", strict=True))
    except (csv.Error, StopIteration):
        _invalid()


def _normalize_reference(value, config):
    name, overrides, path = _parse_reference(value)
    if ".." in path.split("/") or "\x00" in path or "\\" in path:
        _invalid()
    if name is None:
        return str(Path(path).absolute()), None
    if name.startswith(":") or overrides:
        if name.startswith(":"):
            node = {"type": name[1:]}
        else:
            node = config.get(name)
            if type(node) is not dict:
                _invalid()
            node = dict(node)
        if "type" in overrides:
            _invalid()
        node.update(overrides)
        name = (
            "__dv_inline_"
            + hashlib.sha256(
                json.dumps(node, sort_keys=True, separators=(",", ":")).encode()
            ).hexdigest()
        )
        if name in config and config[name] != node:
            _invalid()
        config[name] = node
    return name + ":" + path, name


def _dependencies(remote, config):
    kind, dependencies = remote["type"], []
    if kind in _WRAPPERS:
        reference, name = _normalize_reference(remote.get("remote", ""), config)
        remote["remote"] = reference
        if name is not None:
            dependencies.append(name)
    elif kind in {"union", "combine"}:
        normalized = []
        for upstream in _space_list(remote.get("upstreams", "")):
            mount, suffix = "", ""
            if kind == "combine":
                mount, separator, upstream = upstream.partition("=")
                if not separator or not mount or "/" in mount or mount in {".", ".."}:
                    _invalid()
                mount += "="
            else:
                for flag in (":writeback", ":ro", ":nc"):
                    if upstream.endswith(flag):
                        upstream, suffix = upstream[: -len(flag)], flag
                        break
            reference, name = _normalize_reference(upstream, config)
            normalized.append(mount + reference + suffix)
            if name is not None:
                dependencies.append(name)
        output = io.StringIO()
        csv.writer(output, delimiter=" ", lineterminator="\n").writerow(normalized)
        remote["upstreams"] = output.getvalue().rstrip("\n")
    return dependencies


def _record(data, files, dependencies=None):
    key = hashlib.sha256(data).hexdigest()
    files[key] = {
        "kind": "provider_credential",
        "content_b64": base64.b64encode(data).decode(),
    }
    if dependencies:
        files[key]["dependencies"] = sorted(set(dependencies))
    return key


def _profile_material(data, path, files, provider, profile):
    source = configparser.ConfigParser(interpolation=None)
    source.optionxform = lambda optionstr: optionstr
    source.read_string(data.decode())
    destination = configparser.ConfigParser(interpolation=None)
    destination.optionxform = lambda optionstr: optionstr
    visited, dependencies = set(), []

    def visit(name):
        if name in visited or len(visited) >= 32 or name not in source:
            _invalid()
        visited.add(name)
        values = dict(source[name])
        if provider == "s3" and values.get("source_profile"):
            visit(values["source_profile"])
        if provider == "oracleobjectstorage":
            for field in ("key_file", "security_token_file"):
                if values.get(field):
                    reference = Path(values[field]).expanduser()
                    if not reference.is_absolute():
                        reference = Path(path).parent / reference
                    child = _read_material(reference)
                    if b"OPENSSH PRIVATE KEY" in child:
                        _invalid()
                    identifier = _record(child, files)
                    dependencies.append(identifier)
                    values[field] = "dv-recovery-file:" + identifier
        if name == source.default_section:
            destination.read_dict({source.default_section: values})
        else:
            destination[name] = values

    visit(profile)
    if not dependencies and set(source.sections()) == visited and not source.defaults():
        return data, dependencies
    output = io.StringIO()
    destination.write(output)
    return output.getvalue().encode(), dependencies


def _load_record(key, source_files, files, active=None):
    active = set() if active is None else active
    if key in active or len(active) > 32 or len(files) >= MAX_REMOTES:
        _invalid()
    if key in files:
        return
    record = source_files.get(key)
    if (
        type(record) is not dict
        or set(record)
        not in ({"kind", "content_b64"}, {"kind", "content_b64", "dependencies"})
        or record["kind"] != "provider_credential"
    ):
        _invalid()
    data = base64.b64decode(record["content_b64"], validate=True)
    if (
        not data
        or len(data) > MAX_MATERIAL_BYTES
        or base64.b64encode(data).decode() != record["content_b64"]
        or hashlib.sha256(data).hexdigest() != key
    ):
        _invalid()
    dependencies = record.get("dependencies", [])
    if (
        type(dependencies) is not list
        or len(dependencies) > 32
        or len(dependencies) != len(set(dependencies))
    ):
        _invalid()
    active.add(key)
    for dependency in dependencies:
        if (
            type(dependency) is not str
            or ("dv-recovery-file:" + dependency).encode() not in data
        ):
            _invalid()
        _load_record(dependency, source_files, files, active)
    active.remove(key)
    files[key] = record


def _material(value, files, source_files, *, provider=None, field=None, profile=None):
    if source_files is None:
        if type(value) is not str:
            _invalid()
        path = Path(value).expanduser()
        data = _read_material(path)
        dependencies = []
        if (provider, field) in {
            ("s3", "shared_credentials_file"),
            ("oracleobjectstorage", "config_file"),
        }:
            data, dependencies = _profile_material(
                data,
                path,
                files,
                provider,
                profile or ("default" if provider == "s3" else "DEFAULT"),
            )
        key = _record(data, files, dependencies)
    else:
        if type(value) is not dict or set(value) != {"file_ref"}:
            _invalid()
        key = value["file_ref"]
        _load_record(key, source_files, files)
    return {"file_ref": key}


def _external_command(kind, field):
    return field == "ssh" or (
        field.endswith("_command")
        and not (kind == "sftp" and field in {"md5sum_command", "sha1sum_command"})
    )


def package_rclone(
    config,
    storages,
    options=None,
    *,
    source_files=None,
    source_requirements=None,
    source_commands=None,
):
    """Resolve only referenced roots; unavailable source requirements remain explicit."""
    options = provider_options() if options is None else options
    if type(config) is not dict or len(config) > MAX_REMOTES:
        _invalid()
    config = dict(config)
    result = {"schema_version": 1, "roots": [], "remotes": {}, "files": {}}
    for storage in storages:
        name = storage["rclone_remote"]
        root = {"storage_id": storage["id"], "remote_name": name, "state": "portable"}
        collected, files, active = {}, {}, set()
        external_identity = False
        commands = list((source_commands or {}).get(storage["id"], []))
        for command in commands:
            if type(command) is not dict or set(command) != {"remote", "field"}:
                _invalid()
            kind = config.get(command["remote"], {}).get("type")
            if (
                kind not in options
                or command["field"] not in options[kind]
                or not _external_command(kind, command["field"])
            ):
                _invalid()
        requirements = list((source_requirements or {}).get(storage["id"], []))
        for requirement in requirements:
            if type(requirement) is not dict or set(requirement) != {
                "remote",
                "key_path",
                "agent",
            }:
                _invalid()
            if (
                config.get(requirement["remote"], {}).get("type") != "sftp"
                or type(requirement["agent"]) is not bool
            ):
                _invalid()
            key_path = requirement["key_path"]
            if key_path is not None and (
                type(key_path) is not str
                or not key_path.startswith("/")
                or ".." in key_path.split("/")
                or "\x00" in key_path
            ):
                _invalid()

        def visit(name, depth=0):
            nonlocal external_identity
            if (
                type(name) is not str
                or not _NAME.fullmatch(name)
                or depth > 32
                or name in active
            ):
                _invalid()
            if name in collected:
                return
            remote = config.get(name)
            if type(remote) is not dict or len(remote) > 256:
                _invalid()
            remote = dict(remote)
            kind = remote.get("type")
            if type(kind) is not str or kind not in options:
                _invalid()
            if kind == "oracleobjectstorage":
                provider = remote.get("provider", "env_auth")
                if provider == "user_principal_auth" and not remote.get("config_file"):
                    default = Path("~/.oci/config").expanduser()
                    if source_files is None and default.is_file():
                        remote["config_file"] = str(default)
                    else:
                        external_identity = True
                elif provider in {
                    "env_auth",
                    "instance_principal_auth",
                    "workload_identity_auth",
                    "resource_principal_auth",
                }:
                    external_identity = True
            if kind == "sftp" and any(
                remote.get(field) not in (None, "", "false", "0")
                for field in _SSH_PRIVATE
            ):
                requirements.append(
                    {
                        "remote": name,
                        "key_path": remote.get("key_file") or None,
                        "agent": remote.get("key_use_agent", "false").lower()
                        not in {"false", "0", ""},
                    }
                )
                remote = {
                    key: value
                    for key, value in remote.items()
                    if key not in _SSH_PRIVATE
                }
            if remote.get("env_auth", "false").lower() not in {"false", "0", ""}:
                explicit_file = any(
                    remote.get(field) for field in _FILE_OPTIONS.get(kind, set())
                )
                explicit_inline = kind in {
                    "drive",
                    "google cloud storage",
                } and remote.get("service_account_credentials")
                if not explicit_file and not explicit_inline:
                    external_identity = True
            # Executable and ambient credential sources are not embedded programs/identities.
            for field, value in list(remote.items()):
                if field != "type" and field not in options[kind]:
                    _invalid()
                if type(value) is not str and field not in _FILE_OPTIONS.get(
                    kind, set()
                ):
                    _invalid()
                if type(value) is str and len(value) > MAX_MATERIAL_BYTES:
                    _invalid()
                if value and _external_command(kind, field):
                    commands.append({"remote": name, "field": field})
                    del remote[field]

            active.add(name)
            for dependency in _dependencies(remote, config):
                visit(dependency, depth + 1)
            copy = dict(remote)
            for field in _FILE_OPTIONS.get(kind, set()):
                if copy.get(field):
                    copy[field] = _material(
                        copy[field],
                        files,
                        source_files,
                        provider=kind,
                        field=field,
                        profile=remote.get("profile")
                        if kind == "s3"
                        else remote.get("config_profile"),
                    )
            collected[name] = copy
            active.remove(name)

        try:
            visit(name)
        except (
            RcloneRecoveryError,
            configparser.Error,
            OSError,
            ValueError,
            TypeError,
            KeyError,
            AttributeError,
        ):
            root["state"] = "credentials_required"
            root["reason"] = "external_or_unsafe_dependency"
        else:
            if requirements:
                root["state"] = "credentials_required"
                root["reason"] = "ssh_private_key_required"
                root["ssh_requirements"] = sorted(
                    requirements, key=lambda item: item["remote"]
                )
            if external_identity or commands:
                root["state"] = "credentials_required"
                root["reason"] = "external_credentials_required"
                if external_identity:
                    root["external_identity_required"] = True
                if commands:
                    root["external_commands"] = sorted(
                        commands, key=lambda item: (item["remote"], item["field"])
                    )
            result["remotes"].update(collected)
            result["files"].update(files)
        result["roots"].append(root)
    return result


def validate_rclone(payload, storages, options=None):
    """Import never reads host files or accepts hidden/unreferenced configuration."""
    try:
        if type(payload) is not dict or set(payload) != {
            "schema_version",
            "roots",
            "remotes",
            "files",
        }:
            _invalid()
        if type(payload["schema_version"]) is not int or payload["schema_version"] != 1:
            _invalid()
        roots = [storage for storage in storages if storage["storage_type"] == "rclone"]
        if len(payload["roots"]) != len(roots) or len(payload["files"]) > MAX_REMOTES:
            _invalid()
        expected = package_rclone(
            payload["remotes"],
            roots,
            options,
            source_files=payload["files"],
            source_requirements={
                root["storage_id"]: root.get("ssh_requirements", [])
                for root in payload["roots"]
            },
            source_commands={
                root["storage_id"]: root.get("external_commands", [])
                for root in payload["roots"]
            },
        )
        # An incomplete root may carry only its known redacted requirement, not runnable fragments.
        for actual, computed in zip(payload["roots"], expected["roots"], strict=True):
            if (
                actual.get("state") == "credentials_required"
                and computed["state"] == "credentials_required"
            ):
                if actual != computed:
                    _invalid()
            elif actual != computed or computed["state"] != "portable":
                _invalid()
        if expected != payload:
            _invalid()
    except (ValueError, TypeError, KeyError, AttributeError):
        _invalid()


class _UnixConnection(http.client.HTTPConnection):
    def __init__(self, path):
        super().__init__("localhost", timeout=5)
        self.path = path

    def connect(self):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(self.timeout)
        self.sock.connect(self.path)


def _rpc(path, endpoint, data):
    connection = _UnixConnection(path)
    try:
        connection.request(
            "POST",
            "/" + endpoint,
            json.dumps(data).encode(),
            {"Content-Type": "application/json"},
        )
        response = connection.getresponse()
        body = response.read(8 * 1024 * 1024 + 1)
        if response.status != 200 or len(body) > 8 * 1024 * 1024:
            _invalid()
        return json.loads(body)
    finally:
        connection.close()


def _common(config, unlock):
    environment = _environment()
    environment["DV_RECOVERY_RCLONE_UNLOCK"] = unlock
    return [
        _binary(),
        "--config",
        str(config),
        "--ask-password=false",
        "--log-level",
        "ERROR",
        "--password-command",
        "/usr/bin/printenv DV_RECOVERY_RCLONE_UNLOCK",
    ], environment


def _config_command(config, unlock, args):
    common, environment = _common(config, unlock)
    try:
        result = subprocess.run(
            _owner_bound(common + args),
            env=environment,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            timeout=30,
        )
        if result.returncode or len(result.stdout) > 8 * 1024 * 1024:
            _invalid()
        return result.stdout
    except (OSError, subprocess.TimeoutExpired):
        _invalid()


def _owner_bound(command):
    return [
        sys.executable,
        "-I",
        str(Path(__file__).with_name("recovery_child.py")),
        str(os.getpid()),
        *command,
    ]


@contextlib.contextmanager
def _rpc_address(directory):
    descriptor = os.open(directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        # Both addresses stay short even when the private generation path is long.
        yield f"/proc/self/fd/{descriptor}/rc"
    finally:
        (directory / "rc").unlink(missing_ok=True)
        os.close(descriptor)


def _create_managed(payload, directory):
    directory = Path(directory)
    directory.mkdir(mode=0o700, parents=False, exist_ok=False)
    config = directory / "rclone.conf"
    unlock = secrets.token_urlsafe(48)
    daemon = None
    try:
        # Encrypt the EMPTY native config before supplying any credentials.
        _config_command(config, unlock, ["config", "touch"])
        _config_command(config, unlock, ["config", "encryption", "set"])
        refs = {}
        overrides = {}
        common, environment = _common(config, unlock)
        with _rpc_address(directory) as address:
            daemon = subprocess.Popen(
                _owner_bound(
                    common + ["rcd", "--rc-addr", "unix://rc", "--rc-no-auth"]
                ),
                cwd=directory,
                env=environment,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            deadline = time.monotonic() + 10
            while True:
                try:
                    _rpc(address, "rc/noop", {})
                    break
                except OSError:
                    if daemon.poll() is not None or time.monotonic() >= deadline:
                        _invalid()
                    time.sleep(0.02)
            try:
                for name, remote in payload["remotes"].items():
                    parameters = dict(remote)
                    for field in _FILE_OPTIONS.get(remote["type"], set()):
                        value = parameters.get(field)
                        if isinstance(value, dict):
                            refs.setdefault(name, {})[field] = value["file_ref"]
                            parameters[field] = "dv-recovery-file:" + value["file_ref"]
                    # Bootstrap as memory, then set the validated type in the same native
                    # config call: avoids ALL provider onboarding/network state machines.
                    # Native config/create deliberately discards every config_* field,
                    # including OCI config_file/config_profile; retain these encrypted.
                    overrides[name] = {
                        field: value
                        for field, value in parameters.items()
                        if field.startswith("config_")
                    }
                    _rpc(
                        address,
                        "config/create",
                        {
                            "name": name,
                            "type": "memory",
                            "parameters": parameters,
                            "opt": {
                                "nonInteractive": True,
                                "noOutput": True,
                                "noObscure": True,
                            },
                        },
                    )
            finally:
                daemon.terminate()
                try:
                    daemon.communicate(timeout=5)
                except subprocess.TimeoutExpired:
                    daemon.kill()
                    daemon.communicate(timeout=5)
                daemon = None
        _config_command(config, unlock, ["config", "encryption", "check"])
        os.chmod(config, 0o600)
        with config.open("rb") as stream:
            os.fsync(stream.fileno())
        descriptor = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
        return {
            "directory": str(directory),
            "unlock": encrypt_value(unlock),
            "file_refs": refs,
            "cache_remotes": [
                name
                for name, fields in payload["remotes"].items()
                if fields["type"] == "cache"
            ],
            "files": encrypt_value(json.dumps(payload["files"])),
            "overrides": encrypt_value(json.dumps(overrides)),
            "roots": payload["roots"],
        }
    except BaseException:
        if daemon is not None:
            daemon.kill()
            daemon.communicate(timeout=5)
        shutil.rmtree(directory)
        raise


async def create_managed_config(payload, directory):
    """Produce a generation-bound encrypted native config, not an active installation."""
    from app.remote_storage import _await_io_settled

    return await _await_io_settled(
        asyncio.to_thread(_create_managed, payload, directory)
    )


@contextlib.contextmanager
def _config_lock(state):
    path = Path(state["directory"])
    if path.is_symlink() or not path.is_dir() or (path / "rclone.conf").is_symlink():
        _invalid()
    descriptor = os.open(path / ".lock", os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        yield path / "rclone.conf", decrypt_value(state["unlock"])
    finally:
        os.close(descriptor)


def _managed_snapshot(state, storages):
    with _config_lock(state) as (config, unlock):
        remotes = json.loads(_config_command(config, unlock, ["config", "dump"]))
        for name, fields in json.loads(
            decrypt_value(state.get("overrides")) or "{}"
        ).items():
            if name not in remotes:
                _invalid()
            for field, value in fields.items():
                if field in remotes[name] and remotes[name][field] != value:
                    _invalid()
                remotes[name][field] = value
        for name, fields in state["file_refs"].items():
            if name not in remotes:
                _invalid()
            for field, reference in fields.items():
                if remotes[name].get(field) != "dv-recovery-file:" + reference:
                    _invalid()
                remotes[name][field] = {"file_ref": reference}
        files = json.loads(decrypt_value(state["files"]))
        payload = package_rclone(
            remotes,
            storages,
            source_files=files,
            source_requirements={
                root["storage_id"]: root.get("ssh_requirements", [])
                for root in state["roots"]
            },
            source_commands={
                root["storage_id"]: root.get("external_commands", [])
                for root in state["roots"]
            },
        )
        validate_rclone(payload, storages)
        return payload


async def export_managed_config(state, storages):
    from app.remote_storage import _await_io_settled

    return await _await_io_settled(
        asyncio.to_thread(_managed_snapshot, state, storages)
    )


async def managed_state(session):
    from sqlalchemy import select
    from app.database import AppSettings

    value = (
        await session.execute(
            select(AppSettings.value).where(AppSettings.key == "recovery_rclone")
        )
    ).scalar_one_or_none()
    return json.loads(value) if value else None


def _external_snapshot(storages):
    from app.config import settings

    path = Path(settings.RCLONE_CONFIG)
    if not path.is_file() or path.is_symlink() or path.stat().st_size > 8 * 1024 * 1024:
        return package_rclone({}, storages)
    try:
        config = json.loads(
            _config_command(
                path, settings.RCLONE_CONFIG_PASS.get_secret_value(), ["config", "dump"]
            )
        )
        return package_rclone(config, storages)
    except RcloneRecoveryError:
        return package_rclone({}, storages)


async def export_rclone(session, storages):
    from app.remote_storage import _await_io_settled

    state = await managed_state(session)
    if state is not None:
        return await export_managed_config(state, storages)
    return await _await_io_settled(asyncio.to_thread(_external_snapshot, storages))


def _credential_fd():
    if hasattr(os, "memfd_create"):
        return os.memfd_create("dv-provider-credential", os.MFD_CLOEXEC)
    # Some Python Linux builds omit the wrapper even though libc supports memfd.
    import ctypes

    native = ctypes.CDLL(None, use_errno=True).memfd_create
    native.argtypes = (ctypes.c_char_p, ctypes.c_uint)
    native.restype = ctypes.c_int
    descriptor = native(b"dv-provider-credential", 1)  # Linux MFD_CLOEXEC
    if descriptor < 0:
        raise OSError(ctypes.get_errno(), "Anonymous credential memory unavailable")
    return descriptor


def _materialize_credential(reference, files, descriptors, allocated):
    if reference in allocated:
        return allocated[reference]
    record = files[reference]
    data = base64.b64decode(record["content_b64"], validate=True)
    for dependency in record.get("dependencies", []):
        child = _materialize_credential(dependency, files, descriptors, allocated)
        data = data.replace(
            ("dv-recovery-file:" + dependency).encode(),
            f"/proc/self/fd/{child}".encode(),
        )
    descriptor = _credential_fd()
    descriptors.append(descriptor)
    with os.fdopen(descriptor, "wb", closefd=False) as stream:
        stream.write(data)
        stream.flush()
    allocated[reference] = descriptor
    return descriptor


def _run_managed(state, remote, args, timeout):
    root = next(
        (root for root in state["roots"] if root["remote_name"] == remote), None
    )
    ssh_overrides = {}
    if root is not None and root.get("reason") == "ssh_private_key_required":
        from app.config import settings

        allowed = [
            Path(path.strip()).resolve()
            for path in settings.ALLOWED_SSH_KEY_DIRS.split(",")
            if path.strip()
        ]
        for requirement in root["ssh_requirements"]:
            path = (
                Path(requirement["key_path"]).resolve()
                if requirement["key_path"]
                else None
            )
            if (
                path is None
                or not path.is_file()
                or not any(path.is_relative_to(directory) for directory in allowed)
            ):
                return 1, "", "Storage credentials required"
            ssh_overrides[environment_name(requirement["remote"], "key_file")] = str(
                path
            )
    if root is None or (root["state"] != "portable" and not ssh_overrides):
        return 1, "", "Storage credentials required"
    descriptors = []
    try:
        with (
            _config_lock(state) as (config, unlock),
            tempfile.TemporaryDirectory(
                prefix="dvrc-cache-", dir=state["directory"]
            ) as cache,
        ):
            common, environment = _common(config, unlock)
            environment.update(ssh_overrides)
            environment.update(
                {
                    "HOME": cache,
                    "XDG_CACHE_HOME": cache,
                    "TMPDIR": cache,
                    "AWS_EC2_METADATA_DISABLED": "true",
                    "AWS_CONFIG_FILE": os.devnull,
                }
            )
            files = json.loads(decrypt_value(state["files"]))
            # Never adopt source caches/queued uploads; completion must mean upstream completion.
            for name in state.get("cache_remotes", []):
                private_cache = Path(cache) / hashlib.sha256(name.encode()).hexdigest()
                for field, value in {
                    "db_path": str(private_cache / "metadata"),
                    "chunk_path": str(private_cache / "chunks"),
                    "tmp_upload_path": "",
                }.items():
                    environment[environment_name(name, field)] = value
            for name, fields in json.loads(
                decrypt_value(state.get("overrides")) or "{}"
            ).items():
                for field, value in fields.items():
                    environment[environment_name(name, field)] = value
            allocated = {}
            for name, fields in state["file_refs"].items():
                for field, reference in fields.items():
                    descriptor = _materialize_credential(
                        reference, files, descriptors, allocated
                    )
                    environment[environment_name(name, field)] = (
                        f"/proc/self/fd/{descriptor}"
                    )
            result = subprocess.run(
                _owner_bound(
                    common + ["--timeout", "300s", "--contimeout", "30s"] + args
                ),
                env=environment,
                pass_fds=tuple(descriptors),
                stdin=subprocess.DEVNULL,
                capture_output=True,
                timeout=timeout,
            )
            if len(result.stdout) > 8 * 1024 * 1024:
                return 1, "", "Storage response exceeds recovery limit"
            return (
                (0, result.stdout.decode(), "")
                if result.returncode == 0
                else (result.returncode, "", "Remote storage unavailable")
            )
    except (OSError, ValueError, subprocess.TimeoutExpired):
        return 1, "", "Remote storage unavailable"
    finally:
        for descriptor in descriptors:
            os.close(descriptor)


async def run_managed(state, remote, args, *, timeout=None):
    """Serialize native refresh+file use across processes and settle before cleanup."""
    from app.remote_storage import _await_io_settled

    return await _await_io_settled(
        asyncio.to_thread(_run_managed, state, remote, args, timeout)
    )
