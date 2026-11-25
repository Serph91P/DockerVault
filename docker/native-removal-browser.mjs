import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { chmodSync, constants, accessSync, lstatSync, mkdirSync, readFileSync, readdirSync, renameSync, utimesSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { DatabaseSync } from 'node:sqlite'
import { chromium, request } from 'playwright'
import { expect } from 'playwright/test'

const baseURL = 'http://127.0.0.1'
const data = '/app/data/removal-browser'
const replicaRoot = join(data, 'replica')
const manifest = join(data, 'fixture.json')
const username = 'removal-browser'
const password = process.env.DV_FIXTURE_PASSWORD
assert.match(password, /^[a-f0-9]{32}$/)
assert.equal(process.getuid(), 1000, 'fault must run under the native backend UID')

async function api(client, path, data) {
  const response = await client.fetch(`/api/v1${path}`, {
    method: data === undefined ? 'GET' : 'POST', data, timeout: 15000,
  })
  assert.ok(response.ok(), `${path}: HTTP ${response.status()}`)
  return response.json()
}

function catalog() {
  const db = new DatabaseSync('/app/data/backup.db', { readOnly: true })
  try {
    assert.equal(db.prepare('PRAGMA foreign_key_check').all().length, 0)
    return JSON.parse(JSON.stringify({
      backups: db.prepare('SELECT id,target_id,status,encrypted,file_path,encryption_key_path FROM backups ORDER BY id').all(),
      syncs: db.prepare('SELECT id,backup_id,storage_id,remote_path,sync_status FROM backup_storage_syncs ORDER BY id').all(),
    }))
  } finally { db.close() }
}

function digest(path) {
  assert.ok(lstatSync(path).isFile(), `expected regular file: ${path}`)
  const bytes = readFileSync(path)
  assert.ok(bytes.length, `expected nonempty file: ${path}`)
  return createHash('sha256').update(bytes).digest('hex')
}

function files(root) {
  return readdirSync(root, { recursive: true, withFileTypes: true })
    .filter(entry => !entry.isDirectory())
    .map(entry => join(entry.parentPath, entry.name)).sort()
}

if (process.argv[2] === 'seed') {
  const client = await request.newContext({ baseURL, extraHTTPHeaders: { Origin: baseURL } })
  try {
    await api(client, '/auth/setup', { username, password, confirm_password: password })
    mkdirSync(replicaRoot, { recursive: true })
    const source = join(data, 'source')
    mkdirSync(source)
    writeFileSync(join(source, 'payload'), 'owned synthetic copy removal payload\n')
    const { id: storage } = await api(client, '/storage', {
      name: 'Removal browser replica', storage_type: 'local', base_path: replicaRoot, enabled: true,
    })
    await api(client, '/encryption/setup', {})
    await api(client, '/encryption/confirm-setup', { confirmed: true })
    const { id: target } = await api(client, '/targets', {
      name: 'Removal browser plan', target_type: 'path', host_path: source, stop_container: false,
      compression_enabled: true, remote_storage_ids: [storage], delete_local_after_sync: false,
    })
    const { id } = await api(client, '/backups', { target_id: target, backup_type: 'full' })
    const record = () => api(client, `/backups/${id}`)
    await expect.poll(async () => {
      const entry = await record()
      return [entry.status, entry.encrypted, entry.local_available, entry.remote_sync_details?.[0]?.status]
    }, { timeout: 30000 }).toEqual(['completed', true, true, 'completed'])
    const before = catalog()
    assert.equal(before.backups.length, 1)
    assert.equal(before.syncs.length, 1)
    const [{ id: recorded, target_id, status, encrypted, file_path: local, encryption_key_path: key }] = before.backups
    const [{ backup_id, storage_id, remote_path, sync_status }] = before.syncs
    assert.deepEqual([recorded, target_id, status, encrypted, backup_id, storage_id, sync_status],
      [id, target, 'COMPLETED', 1, id, storage, 'completed'])
    assert.ok(local.startsWith('/backups/') && key.startsWith('/backups/'))
    const remote = resolve(replicaRoot, remote_path)
    assert.ok(remote.startsWith(replicaRoot + '/') && remote.endsWith('.tar.gz.enc'))
    const sidecar = remote.replace(/\.enc$/, '.key')
    const sentinel = join(dirname(remote), 'unselected.txt')
    writeFileSync(sentinel, 'unselected replica neighbor\n', { flag: 'wx' })
    const hashes = [local, key, remote, sidecar, sentinel].map(digest)
    assert.deepEqual(hashes.slice(0, 2), hashes.slice(2, 4), 'seeded replica must match the local encrypted pair')
    assert.deepEqual(files('/backups'), [local, key].sort())
    assert.deepEqual(files(replicaRoot), [remote, sidecar, sentinel].sort())
    writeFileSync(manifest, JSON.stringify({ id, storage, local, key, remote, sidecar, sentinel, hashes, before }))
    console.log(`Copy removal seeded: archive=${id} storage=${storage} sync=${before.syncs[0].id}, exact encrypted pairs present`)
  } finally { await client.dispose() }
} else {
  assert.equal(process.argv[2], 'browser')
  const { id, storage, local, key, remote, sidecar, sentinel, hashes, before } = JSON.parse(readFileSync(manifest))
  const detail = `/backups/history/${id}`
  assert.deepEqual(catalog(), before, 'original SQLite catalog across restart')
  assert.deepEqual([local, key, remote, sidecar, sentinel].map(digest), hashes, 'all original bytes across restart')

  const browser = await chromium.launch()
  try {
    const ctx = await browser.newContext({ baseURL, locale: 'en-US', reducedMotion: 'reduce' })
    const page = await ctx.newPage()
    const errors = []
    page.on('pageerror', error => errors.push(error.message))
    const route = `/history/archives/${id}`
    await page.goto(route)
    assert.equal((await ctx.request.get(`/api/v1${detail}`)).status(), 401)
    await page.getByLabel('Username', { exact: true }).fill(username)
    await page.getByLabel('Password', { exact: true }).fill(password)
    await page.getByRole('button', { name: 'Sign In', exact: true }).click()
    await expect.poll(async () => (await ctx.cookies()).some(c => c.name === 'session_token' && c.httpOnly)).toBe(true)
    await page.goto(route)
    await expect(page).toHaveURL(baseURL + route)

    async function availability(expected) {
      const record = await api(ctx.request, detail)
      assert.deepEqual([record.id, record.target_id, record.archive_state, record.encrypted, record.local_available, record.remote_synced],
        [id, before.backups[0].target_id, 'recorded', true, true, expected])
      const copies = record.remote_sync_details ?? []
      assert.deepEqual(copies.map(copy => [copy.storage_id, copy.remote_path, copy.status]),
        expected ? [[storage, before.syncs[0].remote_path, 'completed']] : [])
    }
    await availability(true)
    await page.getByRole('button', { name: 'Review copy removal', exact: true }).click()
    const dialog = page.getByRole('dialog', { name: 'Delete archive copies', exact: true })
    const button = name => dialog.getByRole('button', { name, exact: true })
    const retry = button('Retry failed confirmed copies only')
    async function clickAndRead(action, name) {
      const [response] = await Promise.all([
        page.waitForResponse(r => r.url() === `${baseURL}/api/v1/backups/removal/${action}` && r.request().method() === 'POST'),
        button(name).click(),
      ])
      assert.equal(response.status(), 200, action)
      return { payload: response.request().postDataJSON(), body: await response.json() }
    }
    const items = body => body.items.map(item => [item.backup_id, item.copy, item.storage_id ?? null, item.outcome ?? null])
    const localItem = [id, 'local', null]
    const replicaItem = [id, 'remote', storage]
    const { payload, body: preview } = await clickAndRead('preview', 'Review exact scope')
    assert.deepEqual(payload, { backup_ids: [id], scope: 'all' })
    assert.deepEqual(items(preview).sort(), [[...localItem, null], [...replicaItem, null]].sort())
    const confirmed = dialog.getByRole('region', { name: 'Confirmed scope' })
    await expect(confirmed).toContainText('1 archives · 2 selected copies · all scope')
    await expect(confirmed.getByRole('listitem')).toHaveCount(2)
    await expect(confirmed.getByRole('listitem').filter({ hasText: `Archive #${id} · local · Local copy` })).toBeVisible()
    await expect(confirmed.getByRole('listitem').filter({ hasText: `Archive #${id} · remote · Removal browser replica` })).toBeVisible()

    // Change only the primary identity. Keep the replica readable but deny writes to its own parent.
    const old = lstatSync(local)
    utimesSync(local, old.atime, new Date(old.mtimeMs + 2000))
    const changed = lstatSync(local, { bigint: true }).mtimeNs
    assert.notEqual(lstatSync(local).mtimeMs, old.mtimeMs)
    assert.deepEqual([digest(local), digest(key)], hashes.slice(0, 2))
    const parent = dirname(remote)
    const mode = lstatSync(parent).mode & 0o777
    chmodSync(parent, mode & ~0o222)
    async function incomplete(counts) {
      const status = dialog.getByRole('status').filter({ hasText: 'Removal incomplete' })
      await expect(status).toContainText(counts)
      await expect(status.getByRole('listitem').filter({ hasText: /local.*skipped.*scope changed/i })).toBeVisible()
      await expect(dialog.getByText('Selected copies removed', { exact: true })).toHaveCount(0)
    }
    let partial
    try {
      assert.throws(() => accessSync(parent, constants.W_OK), { code: 'EACCES' })
      assert.deepEqual([digest(remote), digest(sidecar)], hashes.slice(2, 4))
      const first = await clickAndRead('execute', 'Permanently delete confirmed copies')
      assert.ok(typeof preview.token === 'string' && preview.token.length && first.payload.token === preview.token)
      partial = first.body
      assert.deepEqual(items(partial).sort(), [[...localItem, 'skipped'], [...replicaItem, 'failed']].sort())
      assert.deepEqual([partial.status, partial.succeeded, partial.failed, partial.skipped], ['incomplete', 0, 1, 1])
      assert.ok(typeof partial.retry_token === 'string' && partial.retry_token.length && partial.retry_token !== preview.token)
      await incomplete('0 succeeded · 1 failed · 1 skipped')
      await expect(retry).toBeEnabled()
      assert.deepEqual(catalog(), before)
      assert.deepEqual([local, key, remote, sidecar, sentinel].map(digest), hashes)
      console.log('Copy removal Chromium: fresh login, exact rendered preview, 0 succeeded / 1 failed / 1 skipped')
    } finally { chmodSync(parent, mode) }

    const second = await clickAndRead('execute', 'Retry failed confirmed copies only')
    assert.deepEqual(second.payload, { token: partial.retry_token })
    assert.deepEqual(items(second.body), [[...replicaItem, 'succeeded']], 'the server retried the replica only')
    assert.equal(second.body.retry_token, null)
    await incomplete('1 succeeded · 0 failed · 1 skipped')
    await expect(retry).toHaveCount(0)
    await availability(false)

    function finalState() {
      assert.deepEqual([digest(local), digest(key)], hashes.slice(0, 2), 'retained local archive and sidecar bytes')
      assert.equal(lstatSync(local, { bigint: true }).mtimeNs, changed, 'retained local identity after preview change')
      assert.deepEqual(files('/backups'), [local, key].sort())
      assert.deepEqual(files(replicaRoot), [sentinel], 'only the exact replica archive and sidecar disappear')
      assert.equal(digest(sentinel), hashes[4])
      assert.deepEqual(catalog(), { backups: before.backups, syncs: [] }, 'only the exact replica sync row disappears')
    }
    finalState()
    // Negative control: the same final-state check must reject a missing primary.
    const held = `${local}.negative-control`
    renameSync(local, held)
    try { assert.throws(finalState, { code: 'ENOENT', path: local }) }
    finally { renameSync(held, local) }
    finalState()
    assert.deepEqual(errors, [])
    console.log('Copy removal Chromium: 1 succeeded / 0 failed / 1 skipped, scope change retained, no retry or full success')
    console.log('Copy removal effects: API primary only; exact SQLite association and local bytes retained; replica pair and sync row removed; missing-primary negative control rejected')
  } finally { await browser.close() }
}
