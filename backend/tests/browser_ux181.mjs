// Real Chromium + cookie auth + SQLite + age; synthetic material never logged.
import assert from 'node:assert/strict'
import { randomBytes } from 'node:crypto'
import { readFile, writeFile, readdir } from 'node:fs/promises'
import { chromium } from 'playwright'

const scenario = process.env.DV_SCENARIO ?? 'encrypted'
const base = 'https://127.0.0.1:8443'
const root = '/app/data/ux181'
const browser = await chromium.launch({ args: ['--no-sandbox', '--disable-dev-shm-usage'] })
const context = await browser.newContext({ ignoreHTTPSErrors: true, acceptDownloads: true, viewport: { width: 390, height: 844 } })
const page = await context.newPage()
const password = randomBytes(24).toString('hex')
let accountRequests = 0, keyRequests = 0, confirmations = 0
page.on('request', request => {
  if (request.method() !== 'POST') return
  if (request.url().endsWith('/auth/setup')) accountRequests++
  if (request.url().endsWith('/encryption/setup')) keyRequests++
  if (request.url().endsWith('/encryption/confirm-setup')) confirmations++
})
const check = (value, message) => assert.ok(value, message)
async function api(path, method = 'GET', data) {
  const response = await context.request.fetch(base + '/api/v1' + path, { method, data })
  check(response.ok(), `HTTP contract failed: ${method} ${path} status=${response.status()}`)
  return response.json()
}
async function until(fn, message, timeout = 45000) {
  const end = Date.now() + timeout
  while (Date.now() < end) { if (await fn()) return; await page.waitForTimeout(50) }
  throw new Error(message)
}
async function noOverflow() {
  check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), 'narrow form overflows')
}
async function login() {
  await page.getByLabel('Username', { exact: true }).fill('synthetic-operator')
  await page.getByLabel('Password', { exact: true }).fill(password)
  await page.getByRole('button', { name: 'Sign In' }).click()
}
async function scan(secret) {
  check(await page.evaluate(value => !JSON.stringify(localStorage).includes(value) && !JSON.stringify(sessionStorage).includes(value) && !location.href.includes(value), secret), 'secret reached browser persistence or URL')
}
async function exportKey() {
  const first = page.waitForEvent('download')
  await page.getByRole('button', { name: 'Download Private Key File' }).click()
  const cancelled = await first
  await cancelled.cancel()
  check(!(await api('/encryption/status')).setup_completed, 'export initiation falsely confirms custody')
  const next = page.waitForEvent('download')
  await page.getByRole('button', { name: /Export started/ }).click()
  const download = await next
  await download.saveAs(root + '/exported.key')
  const contents = await readFile(root + '/exported.key', 'utf8')
  const match = contents.match(/^AGE-SECRET-KEY-1[A-Z0-9]+$/m)
  check(Boolean(match), 'export lacks a real age identity')
  const secret = match[0]
  check(contents.split('\n').every(line => !line.trim() || line.startsWith('#') || line === secret), 'export not age-readable')
  await scan(secret)
  check(confirmations === 0, 'custody confirmed without explicit acknowledgement')
  await page.getByRole('button', { name: 'I have saved my private key' }).dblclick()
  await page.getByRole('heading', { name: 'Next: create your first backup' }).waitFor()
  check((await api('/encryption/status')).setup_completed, 'custody not persisted')
  check(confirmations === 1, 'duplicate custody submission')
  return secret
}
try {
  await until(async () => { try { return (await context.request.get(base + '/health')).ok() } catch { return false } }, 'backend not ready')
  await page.route('**/api/v1/auth/status', route => route.fulfill({ status: 503, contentType: 'application/json', body: '{}' }))
  await page.goto(base)
  await page.getByRole('button', { name: 'Retry', exact: true }).waitFor()
  check(await page.getByRole('button', { name: 'Create account' }).count() === 0, '503 offered account setup')
  await page.unroute('**/api/v1/auth/status')
  await page.getByRole('button', { name: 'Retry', exact: true }).click()
  await page.getByLabel('Username', { exact: true }).waitFor()
  await noOverflow()
  await page.keyboard.press('Tab')
  await page.getByLabel('Username', { exact: true }).focus()
  await page.keyboard.type('synthetic-operator')
  await page.keyboard.press('Tab')
  check(await page.getByLabel('Account password', { exact: true }).evaluate(el => el === document.activeElement), 'account password missing from keyboard order')
  await page.keyboard.type(password)
  await page.keyboard.press('Tab')
  await page.keyboard.type(password + 'mismatch')
  await page.keyboard.press('Tab')
  await page.keyboard.press('Enter')
  await page.getByRole('alert').waitFor()
  await page.getByLabel('Confirm account password').fill(password)
  await page.getByRole('button', { name: 'Create account' }).dblclick()
  await page.getByRole('button', { name: 'Set Up Encryption' }).waitFor()
  check(accountRequests === 1, 'duplicate account submission')
  const cookie = (await context.cookies()).find(c => c.name === 'session_token')
  check(cookie?.httpOnly && cookie.secure && cookie.sameSite === 'Lax', 'real secure HttpOnly cookie missing')
  if (scenario === 'skip') {
    await page.getByRole('button', { name: /Continue without encryption/ }).click()
    await page.getByRole('heading', { name: 'Next: create your first backup' }).waitFor()
    const status = await api('/encryption/status')
    check(status.onboarding_deferred && !status.setup_completed && !status.encryption_enabled, 'deliberate unencrypted choice not authoritative')
    await page.reload()
    await page.getByRole('button', { name: /Logout|Sign out/i }).waitFor()
    await page.getByRole('button', { name: /Logout|Sign out/i }).click()
    await page.getByRole('button', { name: 'Sign In' }).waitFor()
    await login()
    await page.getByRole('link', { name: 'Settings' }).click()
    await page.getByText(/deliberately deferred/i).waitFor()
    await noOverflow()
    console.log('PASS deliberate unencrypted choice persists through reload and real cookie re-login')
  } else {
    if (scenario === 'encrypted') {
      await page.route('**/api/v1/encryption/setup', route => route.fulfill({ status: 500, contentType: 'application/json', body: '{}' }))
      await page.getByRole('button', { name: 'Set Up Encryption' }).click()
      await page.getByRole('alert').waitFor()
      await page.reload()
      await page.getByRole('button', { name: 'Set Up Encryption' }).waitFor()
      check(await page.getByRole('button', { name: 'Create account' }).count() === 0, 'reload replayed account setup')
      await page.unroute('**/api/v1/encryption/setup')
    }
    await page.getByRole('button', { name: 'Set Up Encryption' }).click()
    await page.getByRole('dialog', { name: 'Save Your Private Key' }).waitFor()
    await noOverflow()
    check(await page.getByRole('dialog').evaluate(el => el.contains(document.activeElement)), 'custody keyboard focus outside dialog')
    await page.keyboard.press('Shift+Tab')
    check(await page.getByRole('dialog').evaluate(el => el.contains(document.activeElement)), 'custody focus escaped')
    if (scenario === 'lost') {
      const old = (await api('/encryption/status')).public_key
      await page.reload()
      await page.getByText('Key custody pending', { exact: true }).waitFor()
      check((await api('/encryption/status')).public_key === old, 'reload replaced pending identity')
      check(await page.getByRole('button', { name: 'Set Up Encryption' }).count() === 0, 'pending key setup replay available')
      await page.getByRole('button', { name: 'Regenerate Keys', exact: true }).click()
      await page.getByRole('button', { name: 'Cancel', exact: true }).click()
      check((await api('/encryption/status')).public_key === old, 'cancel rotated identity')
      await page.getByRole('button', { name: 'Regenerate Keys', exact: true }).click()
      await page.getByRole('button', { name: 'Regenerate Keys', exact: true }).last().click()
      await page.getByRole('dialog', { name: 'Save Your Private Key' }).waitFor()
      check((await api('/encryption/status')).public_key !== old, 'explicit regeneration did not rotate')
    }
    const key = await exportKey()
    await page.getByRole('button', { name: 'Create first backup plan' }).click()
    await page.getByRole('heading', { name: 'Create Backup Target' }).waitFor()
    await page.getByRole('heading', { name: 'Host Path', exact: true }).click()
    await page.getByPlaceholder('/path/to/directory').fill(root + '/source')
    await page.getByPlaceholder('Enter a name for this backup target').fill('Synthetic first backup')
    await page.getByRole('button', { name: 'Next', exact: true }).click()
    await page.getByRole('button', { name: 'No schedule (manual only)', exact: true }).click()
    await page.getByRole('button', { name: 'Next', exact: true }).click()
    await page.getByRole('button', { name: 'Next', exact: true }).click()
    await page.getByRole('button', { name: 'No retention policy', exact: true }).click()
    await page.getByRole('button', { name: 'Next', exact: true }).click()
    await page.getByRole('button', { name: 'Next', exact: true }).click()
    await page.getByRole('button', { name: 'Create Target', exact: true }).click()
    await until(async () => (await api('/targets')).length === 1, 'wizard did not persist target')
    check((await api('/backups')).length === 0, 'onboarding falsely created a backup')
    const target = (await api('/targets'))[0]
    const backup = await api('/backups', 'POST', { target_id: target.id, backup_type: 'full' })
    let completed
    await until(async () => { completed = await api(`/backups/${backup.id}`); check(!['failed','cancelled'].includes(completed.status), 'actual backup failed'); return completed.status === 'completed' }, 'actual backup did not finish', 90000)
    check(completed.encrypted === true, 'actual backup is not encrypted')
    await page.setViewportSize({ width: 1280, height: 900 })
    await page.getByRole('button', { name: 'Continue to DockerVault' }).click()
    await page.getByRole('link', { name: 'Backups', exact: true }).click()
    await page.getByText('Synthetic first backup', { exact: true }).first().click()
    await page.getByTitle('Actions', { exact: true }).click()
    await page.getByRole('button', { name: 'Browse files' }).click()
    await page.getByLabel('Age Private Key').fill(key)
    await page.getByRole('button', { name: /Decrypt.*Browse/ }).click()
    await page.getByRole('button', { name: 'Next page' }).waitFor()
    await until(async () => await page.getByRole('button', { name: 'Next page' }).isEnabled(), 'encrypted bounded pagination unavailable')
    await page.getByRole('button', { name: 'Next page' }).click()
    await page.getByText(/2001-/).waitFor()
    // Nested content sorts after the 2003 files. Open any ancestor directory first.
    const folder = page.getByText('nested', { exact: true })
    if (await folder.count() === 0) {
      const sourceFolder = page.getByText('source', { exact: true })
      if (await sourceFolder.count()) await sourceFolder.click()
    }
    await folder.click()
    await page.getByText('payload.txt', { exact: true }).waitFor()
    const downloaded = page.waitForEvent('download')
    await page.getByText('payload.txt', { exact: true }).locator('..').getByTitle('Download file', { exact: true }).click()
    const file = await downloaded
    const stream = await file.createReadStream(); const chunks = []
    for await (const chunk of stream) chunks.push(chunk)
    check(Buffer.concat(chunks).equals(Buffer.from('DockerVault synthetic recovery bytes\n')), 'encrypted nested download bytes differ')
    await page.getByRole('button', { name: 'Close', exact: true }).click()
    await scan(key)
    await page.getByTitle('Actions', { exact: true }).click()
    await page.getByRole('button', { name: 'Restore', exact: true }).click()
    await page.getByText('Custom path', { exact: true }).click()
    await page.getByPlaceholder('/path/to/restore').fill('/backups/ux181-restore')
    await page.getByRole('button', { name: 'Next', exact: true }).click()
    await page.getByPlaceholder(/AGE-SECRET-KEY/).fill(key)
    await page.getByRole('button', { name: 'Next', exact: true }).click()
    await page.getByRole('button', { name: 'Restore Now' }).click()
    await until(async () => await page.getByRole('heading', { name: 'Restore Backup', exact: true }).count() === 0, 'actual restore not completed')
    await scan(key)
    await page.getByRole('button', { name: /Logout|Sign out/i }).click()
    await page.getByRole('button', { name: 'Sign In' }).waitFor()
    check(await page.getByText('Synthetic first backup', { exact: true }).count() === 0, 'prior protected presentation after logout')
    await login()
    await page.getByRole('link', { name: 'Settings' }).click()
    await page.getByText('Encryption Enabled', { exact: true }).waitFor()
    check(await page.getByRole('dialog').count() === 0, 'private custody returned after login')
    await scan(key)
    await writeFile(root + '/browser-result.json', JSON.stringify({ backupId: backup.id, restored: true }))
    console.log(`PASS ${scenario}: account/custody/export/cancellation/reload/keyboard, real encrypted backup/browse/page/download/restore/re-login`)
  }
  check(accountRequests === 1, 'account was replayed')
  console.log('PASS real Chromium narrow-screen emulation; not physical mobile or screenreader evidence')
} finally { await context.close(); await browser.close() }
