// Real production UI + cookie auth; only generated synthetic test credentials.
import assert from 'node:assert/strict'
import { randomBytes } from 'node:crypto'
import { chromium } from 'playwright'

const base = 'http://127.0.0.1'
const browser = await chromium.launch({ args: ['--no-sandbox', '--disable-dev-shm-usage'] })
const context = await browser.newContext()
const page = await context.newPage()
const checks = []
const check = (condition, label) => { assert.ok(condition, label); checks.push(label) }
async function api(path, method = 'GET', data) {
  const response = await context.request.fetch(base + '/api/v1' + path, { method, data })
  assert.ok(response.ok(), `${method} ${path} status ${response.status()}`)
  return response.json()
}
try {
  await page.goto(base)
  await page.getByRole('button', { name: 'Create new installation', exact: true }).click()
  await page.getByLabel('Username', { exact: true }).waitFor()
  const password = randomBytes(24).toString('hex')
  await page.getByLabel('Username', { exact: true }).fill('synthetic-operator')
  await page.getByLabel('Account password', { exact: true }).fill(password)
  await page.getByLabel('Confirm account password').fill(password)
  await page.getByRole('button', { name: 'Create account', exact: true }).click()
  await page.getByRole('button', { name: /Continue without encryption/ }).click()
  await page.getByRole('button', { name: 'Continue to DockerVault' }).click()
  const cookies = await context.cookies()
  check(cookies.some(c => c.name === 'session_token' && c.httpOnly), 'real HttpOnly cookie session')
  await page.getByRole('link', { name: 'Storage', exact: true }).click()
  await page.getByTitle('Browse Files').first().click()
  let panel = page.getByRole('region', { name: 'Browse Local Backups' })
  const row = panel.getByRole('row', { name: /synthetic-backup/ })
  await row.getByText('1.5 KB', { exact: true }).waitFor()
  check(await row.isVisible(), 'recursive 1536-byte folder size rendered')
  check((await api('/storage/local/files/size?path=synthetic-backup')).bytes === 1536, 'authenticated recursive size excludes external link')
  for (const path of ['../sentinel', '/synthetic-backup', '.', 'parent/sentinel']) {
    const result = await context.request.delete(base + '/api/v1/storage/local/files', { params: { path } })
    check(result.status() === 400, 'unsafe delete rejected: ' + path)
  }
  await row.getByTitle('Delete', { exact: true }).click()
  const confirm = page.getByRole('dialog', { name: 'Delete File', exact: true })
  await confirm.getByRole('button', { name: 'Delete', exact: true }).click()
  await confirm.waitFor({ state: 'hidden' })
  await row.waitFor({ state: 'detached' })
  check(await panel.isVisible(), 'browser remains open after directory deletion')
  check(!(await api('/storage/local/files')).files.some(f => f.name === 'synthetic-backup'), 'authoritative listing confirms directory removed')
  const bulk = await api('/storage/local/files/bulk-delete', 'POST', { paths: ['first', '../escape', 'missing', 'locked/data', 'last'] })
  check(JSON.stringify(bulk.results.map(r => r.success)) === JSON.stringify([true, false, false, false, true]), 'real mixed bulk preserves every item and permission failure')
  check(bulk.succeeded === 2 && bulk.failed === 3, 'real mixed bulk counts agree')
  const lockedSize = await api('/storage/local/files/size?path=locked')
  check(lockedSize.state === 'unavailable' && lockedSize.bytes === null, 'real permission size unavailable, not zero')
  await panel.getByRole('button', { name: 'Close storage browser' }).click()
  const configured = await api('/storage', 'POST', { name: 'Synthetic mounted root', storage_type: 'local', base_path: '/app/data/mounted', enabled: true })
  await page.reload()
  await page.getByRole('heading', { name: 'Synthetic mounted root', exact: true }).waitFor()
  await page.getByTitle('Browse Files').last().click()
  panel = page.getByRole('region', { name: 'Browse Synthetic mounted root' })
  const mounted = panel.getByRole('row', { name: /mounted-backup/ })
  await mounted.getByText('10.0 B', { exact: true }).waitFor()
  check((await api(`/storage/${configured.id}/files/size?path=mounted-backup`)).bytes === 10, 'configured local root size via authenticated API and UI')
  await mounted.getByTitle('Delete', { exact: true }).click()
  const mountedConfirm = page.getByRole('dialog', { name: 'Delete File', exact: true })
  await mountedConfirm.getByRole('button', { name: 'Delete', exact: true }).click()
  await mountedConfirm.waitFor({ state: 'hidden' })
  await mounted.waitFor({ state: 'detached' })
  check(await panel.isVisible(), 'configured-root browser remains open after delete')
  console.log(JSON.stringify({ status: 'passed', checks }))
} finally {
  await browser.close()
}
