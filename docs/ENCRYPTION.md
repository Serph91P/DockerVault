# Encrypted archive compatibility

## New v1 archives

New `.enc` files are encrypt-then-MAC envelopes:

1. Eight bytes `DVENC\x00\x01\n` (format/version).
2. OpenSSL's salted AES-256-CBC ciphertext (`enc -pbkdf2 -iter 100000`), using
   the 32-byte random DEK's lowercase 64-character hex representation as passphrase.
3. A 32-byte HMAC-SHA256 over **all header and ciphertext bytes**. Its independent
   authentication key is `HMAC-SHA256(DEK, "DockerVault archive authentication v1")`.

The existing age-armored `.key` container remains the key envelope. New envelopes
wrap `DVKEY\x00\x01\n` followed by the raw 32-byte DEK. This authenticated version
binding prevents stripping the archive header/tag and presenting a new archive as
legacy CBC. Wrong, unknown or truncated key versions are rejected. No DB schema
change, key-pair regeneration, existing-data rewrite or migration is required.

Authentication is checked with constant-time tag comparison before OpenSSL is
invoked on the copied ciphertext. Neither restore, browse nor standalone recovery
can release plaintext before that succeeds. Decryption writes inside a private
0700 temporary directory, then atomically replaces the requested output only after
OpenSSL succeeds. Existing outputs survive authentication/decryption errors.
The standalone codec reads in 1-MiB blocks; no whole-archive memory allocation.
Key/passphrase files never appear in argv or logs, and temporary material is
removed on success/error. The app stops/joins the codec process group on cancel.

## Legacy read support

Legacy age envelopes unwrap to **exactly 32 raw bytes** with no version marker;
legacy ciphertext begins with OpenSSL's `Salted__` header. These pairs remain
readable, including binary DEKs containing NUL/newline. Hex conversion has no
trailing newline. Legacy CBC does **not** authenticate content; successful padding
or decompression is not proof against tampering. Verify against an independently
trusted original checksum before extraction. Never downgrade a v1 key/archive or
ignore an authentication error to force recovery.

Old archives are not silently rewritten. To obtain the v1 integrity guarantee,
create and verify a new backup; retain existing copies until that succeeds.
An attacker able to replace both an archive and the separately trusted private
key is outside this storage-tampering boundary.

## Disaster recovery

Save `/recover_backup.py` from the DockerVault installation with exported keys,
or save `backend/app/recovery.py` from a trusted release source under that name.
The same standalone standard-library codec is used by the application. It needs
Python 3, `age`, and `openssl`, but no app, database or Python package installation:

```bash
python3 recover_backup.py --identity private_key.txt --key backup.tar.gz.key \
  --archive backup.tar.gz.enc --output backup.tar.gz
```

Only extract after exit 0. The helper handles both explicit formats, validates
v1 before destination mutation, converts legacy binary DEKs correctly, and cleans
its temporary files. Keep the private key and recovery script separately backed up;
the script is public, not a secret. Generated key-export, API, README and frontend
instructions are exercised against actual age/OpenSSL in native runtime CI.
