// Real rendered pages + authenticated API + SQLite/disk/provider effects.
import assert from 'node:assert/strict'
import { chromium } from 'playwright'

const base = 'http://127.0.0.1:8443'
const phase = process.argv[2]
const browser = await chromium.launch({ args: ['--no-sandbox', '--disable-dev-shm-usage'] })
const context = await browser.newContext()
const page = await context.newPage()
const errors = []
page.on('pageerror', error => errors.push(error.message))
async function api(path, data) {
  const response = data === undefined ? await context.request.get(base + '/api/v1/' + path) : await context.request.post(base + '/api/v1/' + path, { data })
  assert.equal(response.status(), 200, `API ${path.split('?')[0]} failed (${response.status()})`)
  return response.json()
}
async function state(item) { return api(`removal-fixture/state/${item.backup_id}`) }
function card(item) { return page.locator('div.bg-dark-800.rounded-xl').filter({ has: page.getByRole('heading', { name: item.name, exact: true }) }) }
async function open(item, copies) {
  const legacy = item.entry.startsWith('Targets')
  await page.goto(base + (legacy ? '/tests/removal.html' : '/backups'))
  await page.getByRole('heading', { name: item.name, exact: true }).waitFor()
  if (!legacy) await page.getByRole('heading', { name: item.name, exact: true }).click()
  const button = copies ? card(item).getByRole('button', { name: legacy ? 'Delete archive copies' : 'Review archive copy deletion' }) : page.getByRole('button', { name: `Remove protection plan ${item.name}`, exact: true })
  await button.focus()
  await button.press('Enter')
  await page.getByRole('dialog').waitFor()
}
try {
  let ready = false
  for (let i = 0; i < 300 && !ready; i++) {
    try { ready = (await context.request.get(base + '/health')).ok() } catch {}
    if (!ready) await new Promise(resolve => setTimeout(resolve, 100))
  }
  assert.ok(ready, 'isolated backend must start')
  await api(phase === 'plans' ? 'auth/setup' : 'auth/login', { username: 'removal-browser', password: process.env.DV_FIXTURE_PASSWORD, confirm_password: process.env.DV_FIXTURE_PASSWORD })
  const manifest = await api('removal-fixture/manifest')
  const outsider = await browser.newContext()
  const denied = await outsider.request.post(base + '/api/v1/backups/removal/preview', { data: { scope: 'all' } })
  assert.equal(denied.status(), 401, 'removal must require a real authenticated session')
  await outsider.close()
  const private_key = (await api('removal-fixture/key')).private_key
  if (phase === 'plans') {
    for (const item of manifest.filter(i => !i.entry.includes('copies'))) {
      const before = await state(item)
      await open(item, false)
      await page.getByRole('button', { name: 'Cancel', exact: true }).press('Escape')
      await page.getByRole('dialog').waitFor({ state: 'detached' })
      assert.deepEqual(await state(item), before, 'Escape/cancel must not mutate copies or plan')
      await open(item, false)
      await page.getByRole('button', { name: 'Review exact scope' }).click()
      await page.getByText('No archive or copy will be deleted.').waitFor()
      const confirm = page.getByRole('button', { name: 'Remove plan only — keep recovery points' })
      await confirm.focus()
      await confirm.press('Tab')
      assert.equal(await page.evaluate(() => document.activeElement.textContent), 'Cancel', 'Tab must wrap inside confirmation')
      await confirm.press('Enter')
      await page.getByText('Protection plan removed. Existing recovery points retained.').waitFor()
      await page.getByRole('button', { name: 'Close', exact: true }).click()
      await page.reload()
      const after = await state(item)
      assert.deepEqual(after, { ...before, removed: true })
      const active = await api('targets')
      assert.ok(!active.some(target => target.id === item.target_id))
      console.log(`PASS keyboard/cancel/plan-only ${item.entry} ${item.scope}: SQLite tombstone; exact archive/key/provider bytes retained`)
    }
  } else {
    // The orchestrator restarted the real application process and kept its owned disk.
    for (const item of manifest.filter(i => !i.entry.includes('copies'))) {
      assert.ok((await state(item)).removed)
      const detail = await api(`backups/${item.backup_id}`)
      assert.equal(detail.target_name, item.name)
      const logs = await api(`backups/${item.backup_id}/logs`)
      assert.ok(logs.some(log => log.message === 'Retained synthetic log'))
      const files = await api(`backups/${item.backup_id}/files`, { private_key })
      assert.ok(files.some(file => file.name === 'payload.txt' || file.path === 'payload.txt'))
      await api(`backups/${item.backup_id}/restore`, { private_key })
      assert.ok((await state(item)).restored_bytes_match)
      await page.goto(base + '/backups')
      await page.getByRole('heading', { name: item.name, exact: true }).waitFor()
      console.log(`PASS process-restart/history/encrypted browse+byte-checked restore ${item.entry} ${item.scope}`)
    }
    for (const item of manifest.filter(i => i.entry.includes('copies'))) {
      await open(item, true)
      const scope = item.scope === 'mixed' ? 'all' : item.scope
      await page.getByLabel(scope === 'all' ? 'All selected local and remote copies' : scope === 'local' ? 'Local copies only' : 'Remote copies only', { exact: true }).check()
      await page.getByRole('button', { name: 'Review exact scope' }).click()
      await page.getByText(new RegExp(`1 archives · ${scope === 'all' ? 2 : 1} selected copies`)).waitFor()
      const before = await state(item)
      await page.getByRole('button', { name: 'Cancel', exact: true }).click()
      assert.deepEqual(await state(item), before, 'cancel after preview must not mutate')
      await open(item, true)
      await page.getByLabel(scope === 'all' ? 'All selected local and remote copies' : scope === 'local' ? 'Local copies only' : 'Remote copies only', { exact: true }).check()
      await page.getByRole('button', { name: 'Review exact scope' }).click()
      await page.getByText(new RegExp(`1 archives · ${scope === 'all' ? 2 : 1} selected copies`)).waitFor()
      if (scope === 'all') await api(`removal-fixture/block/${item.backup_id}`, {})
      let executions = 0
      const listener = request => { if (request.url().endsWith('/backups/removal/execute')) executions++ }
      page.on('request', listener)
      await page.getByRole('button', { name: 'Permanently delete confirmed copies' }).dblclick()
      if (scope === 'all') {
        await page.getByText('Removal incomplete', { exact: true }).waitFor()
        const partial = await state(item)
        assert.ok(!partial.archive && partial.key && partial.catalog && partial.syncs === 1)
        assert.equal(executions, 1, 'double click must be bounded')
        await api(`removal-fixture/block/${item.backup_id}?blocked=false`, {})
        await page.getByRole('button', { name: 'Retry failed confirmed copies only' }).click()
      }
      await page.getByText('Selected copies removed', { exact: true }).waitFor()
      const after = await state(item)
      assert.equal(after.archive, scope === 'remote')
      assert.equal(after.remote, scope === 'local')
      assert.equal(after.syncs, scope === 'local' ? 1 : 0)
      if (scope !== 'all') {
        assert.ok(after.catalog && after.key)
        await api(`backups/${item.backup_id}/restore`, { private_key })
        assert.ok((await state(item)).restored_bytes_match)
      }
      page.off('request', listener)
      await page.getByRole('button', { name: 'Close', exact: true }).click()
      console.log(`PASS ${item.entry} ${scope} copy review/cancel/double-click/results${scope === 'all' ? '/actual-provider-failure/failed-only-retry' : '/retained-copy-byte-restore'}`)
    }
  }
  assert.deepEqual(errors, [], 'no browser runtime exceptions')
} finally { await browser.close() }
