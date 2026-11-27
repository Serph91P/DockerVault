# Internal API reference

`openapi.json` starts from the OpenAPI 3.1 document served by the packaged
Python `production` target at `c7a5594432c82d46c20e2d17068a1e85e8b23957`.
This is the post-Komodo reference. Capture it with `ENABLE_DOCS=true` and a
valid synthetic session at `http://127.0.0.1:8000/openapi.json`. The unmodified
response has SHA-256 `ad298c09bfb21554c9464b69d268103ffdc75d08e87819d14fba60894217dbfe`.
The native document preserves its 89 paths, 112 operations and 72 component
schemas, adds the native GET variant of the orphan scan, and records that both
methods for that path are read-only on the native backend.

When changing the native router, update this document in the same change.
The snapshot preserves the reference's level of annotation; it does not
invent response types where FastAPI published none, and the snapshot is not
a substitute for wire-level compatibility tests.

Swagger UI and ReDoc use the same CDN asset families as the Python docs.
Only the HTML and specification are served by the backend; rendering those
pages requires access to the CDN. Swagger's remote spec validator is off.