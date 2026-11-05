// Real product/cookie/SQLite/file journeys; explicit transport-fault probes.
import assert from 'node:assert/strict'
import { randomBytes, randomUUID } from 'node:crypto'
import { readFile } from 'node:fs/promises'
import { chromium } from 'playwright'
const browser = await chromium.launch({ args: ['--no-sandbox', '--disable-dev-shm-usage'] })
const context = await browser.newContext({ reducedMotion: 'reduce' })
const page = await context.newPage()
const base = 'http://127.0.0.1'
const checks = []
const check = (condition, label) => { assert.ok(condition, label); checks.push(label); console.log('PASS ' + label) }
const username = 'truthful-states-operator', password = randomBytes(24).toString('hex')
const routes = [
  ['/', '/targets', 'Plans'], ['/protection', '/targets', 'Plans'], ['/history', '/backups', 'Archives'],
  ['/protection/schedules', '/schedules', 'Schedules'], ['/protection/retention', '/retention', 'Retention policies'],
  ['/storage', '/storage', 'Storage locations'], ['/settings', '/settings/storage', 'Storage settings'],
]
async function api(path, method = 'GET', data) {
  const response = await context.request.fetch(base + '/api/v1' + path, { method, data })
  assert.ok(response.ok(), `${method} ${path}: ${response.status()}`)
  return response.json()
}
async function ready(route, label) {
  await page.goto(base + route)
  await page.getByLabel(label + ' status', { exact: true }).filter({ hasText: /updated/ }).waitFor()
}
async function login() {
  await page.getByLabel('Username', { exact: true }).fill(username)
  await page.getByLabel('Password', { exact: true }).fill(password)
  await page.getByRole('button', { name: /Sign in/i }).click()
  await page.getByRole('main').waitFor()
}
async function fault(endpoint, mode) {
  const handler = async route => {
    if (new URL(route.request().url()).pathname !== '/api/v1' + endpoint) return route.continue()
    if (mode === 'network') return route.abort('failed')
    if (mode === 'malformed') return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ invalid: true }) })
    return route.fulfill({ status: mode, contentType: 'application/json', body: '{}' })
  }
  await page.route('**/api/v1/**', handler)
  return () => page.unroute('**/api/v1/**', handler)
}
try {
  await page.goto(base)
  await page.getByLabel('Username', { exact: true }).fill(username)
  await page.getByLabel('Account password', { exact: true }).fill(password)
  await page.getByLabel('Confirm account password').fill(password)
  await page.getByRole('button', { name: 'Create account', exact: true }).click()
  await page.getByRole('button', { name: /Continue without encryption/ }).click()
  await page.getByRole('button', { name: 'Continue to DockerVault' }).click()
  check((await context.cookies()).some(cookie => cookie.name === 'session_token' && cookie.httpOnly), 'real HttpOnly cookie authentication')
  for (const [route, endpoint, label] of routes.filter(r => ['/targets', '/backups', '/schedules', '/storage'].includes(r[1]))) {
    await ready(route, label)
    const rows = await api(endpoint)
    check(Array.isArray(rows) && rows.length === 0, `${route}: real confirmed initial empty resource`)
  }
  const schedule = await api('/schedules', 'POST', { name: 'Synthetic shared schedule', cron_expression: '0 2 * * *', enabled: true })
  const target = await api('/targets', 'POST', { name: 'Synthetic path plan', target_type: 'path', host_path: '/app/data/ux183-source', schedule_id: schedule.id, enabled: true, compression_enabled: true })
  const storage = await api('/storage', 'POST', { name: 'Synthetic local provider', storage_type: 'local', enabled: true, base_path: '/app/data/ux183-remote' })
  const run = await api('/backups', 'POST', { target_id: target.id, backup_type: 'full' })
  let settled
  for (let i = 0; i < 300; i++) { settled = await api('/backups/' + run.id); if (!['pending', 'running'].includes(settled.status)) break; await new Promise(resolve => setTimeout(resolve, 50)) }
  check(settled.status === 'completed', 'real backend path backup reaches completed')
  const accepted = await api('/operations', 'POST', { action: 'storage_delete', storage_id: 'local', path: '', paths: ['notification-delete'], request_id: randomUUID() })
  let job
  for (let i = 0; i < 200; i++) { job = await api('/operations/' + accepted.operation_id); if (!['queued', 'running'].includes(job.state)) break; await new Promise(resolve => setTimeout(resolve, 50)) }
  check(job.state === 'completed' && job.succeeded === 1, 'real canonical operation reaches settled item result')

  // Review regressions: native populated SSH responses and scoped nested inventory faults.
  const pageErrors = []
  page.on('pageerror', error => pageErrors.push(error.message))
  await ready('/storage', 'Storage locations')
  await page.getByRole('button', { name: 'Generate', exact: true }).click()
  const keyDialog = page.getByRole('dialog', { name: 'Generate SSH key', exact: true })
  await keyDialog.getByRole('textbox', { name: 'Name', exact: true }).fill('truthful_key')
  await keyDialog.getByRole('button', { name: 'Generate', exact: true }).click()
  await keyDialog.waitFor({ state: 'hidden' })
  await page.getByText('truthful_key', { exact: true }).waitFor()
  const realKeys = await api('/storage/ssh-keys')
  check(realKeys.length === 1 && typeof realKeys[0].created_at === 'number', 'B1: native keygen stores a real key with numeric filesystem timestamp')
  await page.getByRole('button', { name: 'Refresh SSH keys', exact: true }).first().click()
  await page.getByLabel('SSH keys status', { exact: true }).filter({ hasText: /updated/ }).first().waitFor()
  check(await page.getByText('truthful_key', { exact: true }).isVisible(), 'B1: populated SSH list remains visible after authoritative refresh')
  check(await page.getByRole('button', { name: 'Copy public key', exact: true }).isVisible() && await page.getByRole('link', { name: 'Download .pub', exact: true }).isVisible() && await page.getByRole('button', { name: 'Delete key', exact: true }).isVisible(), 'B1: existing key actions remain reachable')
  await page.getByRole('button', { name: 'Install on remote', exact: true }).click()
  await page.getByRole('dialog', { name: 'Install key on remote', exact: true }).waitFor()
  await page.keyboard.press('Escape')
  check(await page.getByRole('button', { name: 'Install on remote', exact: true }).evaluate(element => element === document.activeElement), 'B1: install dialog opens and returns keyboard focus without provider access')
  await page.getByRole('button', { name: 'Add Storage', exact: true }).click()
  const storageDraft = page.getByRole('dialog', { name: 'Add New Storage', exact: true })
  await storageDraft.getByRole('textbox', { name: 'Name', exact: true }).fill('Unsaved SSH selection')
  await storageDraft.getByRole('button', { name: /^SSH/ }).click()
  await storageDraft.getByRole('button', { name: 'Next', exact: true }).click()
  await storageDraft.getByRole('combobox').selectOption(realKeys[0].private_path)
  check(await storageDraft.getByRole('combobox').inputValue() === realKeys[0].private_path, 'B1: Storage connection editor reads and selects the same native populated key list')
  await page.keyboard.press('Escape')
  const malformedInventory = route => route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ contract: 'synthetic contract', timezone: 'UTC', items: [{ kind: 'schedule', id: schedule.id, name: schedule.name, expression: '0 2 * * *', warnings: null }] }) })
  await page.route('**/api/v1/schedules/compatibility/inventory', malformedInventory)
  await ready('/protection/schedules', 'Schedules')
  await page.getByRole('alert', { name: 'Schedule compatibility status', exact: true }).waitFor()
  check(await page.getByText(schedule.name, { exact: true }).isVisible() && pageErrors.length === 0, 'B2: nested malformed inventory is unavailable without losing independent real schedules or crashing')
  await page.unroute('**/api/v1/schedules/compatibility/inventory', malformedInventory)
  await page.getByRole('button', { name: 'Retry Schedule compatibility', exact: true }).click()
  await page.getByRole('button', { name: 'Refresh Schedule compatibility', exact: true }).waitFor()
  check(await page.getByRole('button', { name: 'Refresh Schedule compatibility', exact: true }).evaluate(element => element === document.activeElement), 'B2: scoped Retry recovers the native inventory without reload and returns focus')

  for (const [route, endpoint, label] of routes) {
    let release
    const gate = new Promise(resolve => { release = resolve })
    const delayed = async request => { if (new URL(request.request().url()).pathname !== '/api/v1' + endpoint) return request.continue(); await gate; await request.continue() }
    await page.route('**/api/v1/**', delayed)
    await page.goto(base + route, { waitUntil: 'domcontentloaded' })
    await page.getByText('Loading ' + label, { exact: true }).waitFor()
    check(!await page.getByText('No backups configured', { exact: true }).isVisible(), `${route}: delay does not claim verified empty`)
    release()
    await page.getByLabel(label + ' status', { exact: true }).filter({ hasText: /updated/ }).waitFor()
    await page.unroute('**/api/v1/**', delayed)
    for (const mode of [403, 503, 'network', 'malformed']) {
      const stop = await fault(endpoint, mode)
      await page.reload()
      const feedback = page.getByRole('alert', { name: label + ' status', exact: true })
      await feedback.waitFor()
      check((await feedback.innerText()).includes(mode === 403 ? 'access denied' : 'unavailable'), `${route}: ${mode} has persistent scoped feedback`)
      check(!await page.getByText('No backups configured', { exact: true }).isVisible(), `${route}: ${mode} is not successful zero`)
      await stop()
      await page.getByRole('button', { name: 'Retry ' + label, exact: true }).click()
      const refresh = page.getByRole('button', { name: 'Refresh ' + label, exact: true })
      await refresh.waitFor()
      check(await refresh.evaluate(element => element === document.activeElement), `${route}: ${mode} Retry preserves useful focus`)
    }
    await ready(route, label)
    const stop = await fault(endpoint, 503)
    await page.getByRole('button', { name: 'Refresh ' + label, exact: true }).click()
    const stale = page.getByRole('alert', { name: label + ' status', exact: true }).filter({ hasText: /stale data/ })
    await stale.waitFor()
    check((await stale.innerText()).includes('Last successful refresh'), `${route}: refetch503 retains dated stale snapshot`)
    await stop(); await page.getByRole('button', { name: 'Retry ' + label, exact: true }).click()
    await page.getByLabel(label + ' status', { exact: true }).filter({ hasText: /updated/ }).waitFor()
    const stop401 = await fault(endpoint, 401)
    await page.getByRole('button', { name: 'Refresh ' + label, exact: true }).click()
    await page.getByRole('button', { name: /Sign in/i }).waitFor()
    check(!await page.getByText('Synthetic path plan', { exact: true }).isVisible(), `${route}: 401 isolates private data`)
    check(await page.evaluate(() => sessionStorage.getItem('dockervault-notifications-v1')) === null, `${route}: 401 clears retained outcomes`)
    await stop401(); await login()
  }
  await ready('/protection', 'Plans')
  const search = page.getByPlaceholder('Search backup targets...')
  await search.fill('no matching synthetic plan')
  await page.getByRole('button', { name: 'Clear filters', exact: true }).waitFor()
  const stop = await fault('/targets', 503)
  await page.getByRole('button', { name: 'Refresh Plans', exact: true }).click()
  await page.getByRole('alert', { name: 'Plans status', exact: true }).waitFor()
  check(await search.inputValue() === 'no matching synthetic plan', 'safe filter input survives refetch failure')
  await stop(); await page.getByRole('button', { name: 'Retry Plans', exact: true }).click()
  await page.getByRole('button', { name: 'Clear filters', exact: true }).click()
  check(await search.inputValue() === '', 'filtered empty Clear filters restores list')
  await ready('/history', 'Archives')
  const stopPlans = await fault('/targets', 503)
  await page.reload()
  await page.getByRole('alert', { name: 'Plans status', exact: true }).waitFor()
  await page.getByLabel('Available archives', { exact: true }).getByRole('button', { name: 'Archive actions', exact: true }).waitFor()
  check(true, 'failed target lookup preserves successful archive rows instead of inventing missing plans')
  await stopPlans()

  let dropSocket = true, httpReads = 0, writes = 0
  const countRequests = request => { if (new URL(request.url()).pathname === '/api/v1/targets' && request.method() === 'GET') httpReads++; if (['POST', 'PUT', 'DELETE'].includes(request.method())) writes++ }
  page.on('request', countRequests)
  await context.routeWebSocket('**/ws/updates', socket => { if (dropSocket) socket.close({ code: 1012 }); else socket.connectToServer() })
  await ready('/protection', 'Plans')
  await page.getByText('Live updates disconnected', { exact: true }).waitFor()
  await page.getByText('API: reachable', { exact: true }).waitFor()
  check(true, 'WS interruption does not claim API or all services offline')
  const previousReads = httpReads
  const reconnectRead = page.waitForResponse(response => new URL(response.url()).pathname === '/api/v1/targets' && response.ok())
  dropSocket = false
  await page.getByText('Live updates connected', { exact: true }).waitFor()
  await reconnectRead
  check(httpReads > previousReads && writes === 0, 'WS reconnect refetches authoritative HTTP without re-posting jobs')
  page.off('request', countRequests)

  for (const [route, label] of [[`/protection?plan=${target.id}`, `Plan #${target.id}`], [`/history?run=${run.id}`, `Run #${run.id}`], [`/history?archive=${run.id}`, `Archive #${run.id}`], [`/storage?operation=${job.id}&storage=local`, `Operation ${job.id}`]]) {
    await page.goto(base + route)
    await page.getByLabel(label + ' context', { exact: true }).waitFor()
    check(true, label + ' is an implemented route context')
    await page.reload(); await page.getByLabel(label + ' context', { exact: true }).waitFor()
    check(true, label + ' survives reload with an authoritative read')
  }
  await page.goto(base + `/protection?plan=${target.id}`)
  await page.getByRole('button', { name: 'Open existing plan editor' }).click()
  const editor = page.getByRole('dialog', { name: 'Edit Backup', exact: true })
  await editor.getByRole('textbox', { name: 'Name', exact: true }).fill('Synthetic plan revised')
  const rejectSave = route => route.request().method() === 'PUT' ? route.fulfill({ status: 422, contentType: 'application/json', body: JSON.stringify({ detail: [{ loc: ['body', 'name'], type: 'value_error', msg: 'SYNTHETIC PRIVATE ECHO', input: 'SYNTHETIC PRIVATE ECHO' }] }) }) : route.continue()
  await page.route(`**/api/v1/targets/${target.id}`, rejectSave)
  await editor.getByRole('button', { name: 'Save Changes', exact: true }).click()
  await editor.getByRole('alert').filter({ hasText: 'Review the highlighted fields' }).waitFor()
  const retainedName = editor.getByRole('textbox', { name: 'Name', exact: true })
  check(await retainedName.inputValue() === 'Synthetic plan revised' && await retainedName.getAttribute('aria-invalid') === 'true', 'private editor retains input with associated static backend validation')
  check((await api('/targets/' + target.id)).name === target.name, 'injected rejected save has no backend plan change')
  check(!await editor.getByText('SYNTHETIC PRIVATE ECHO', { exact: true }).isVisible(), 'server validation echo never renders')
  await page.unroute(`**/api/v1/targets/${target.id}`, rejectSave)
  await editor.getByRole('button', { name: 'Save Changes', exact: true }).click()
  await editor.waitFor({ state: 'hidden' })
  check((await api('/targets/' + target.id)).name === 'Synthetic plan revised', 'retry saves preserved input through the real API and SQLite')
  await page.goto(base + '/history?run=' + run.id)
  await page.getByRole('button', { name: 'Open run logs' }).click()
  await page.getByLabel('Run logs status', { exact: true }).filter({ hasText: /updated/ }).waitFor()
  check((await api('/backups/' + run.id + '/logs')).length > 0, 'linked run opens real persisted backend logs')
  await page.keyboard.press('Escape')
  await page.goto(base + '/history?run=2147483647')
  await page.getByRole('alert', { name: 'Run #2147483647 context', exact: true }).waitFor()
  check(true, 'missing object has useful removed/denied fallback')
  await page.goBack(); await page.goForward()
  await page.getByRole('alert', { name: 'Run #2147483647 context', exact: true }).waitFor()
  check(true, 'Back/Forward preserves linked missing context')

  await ready('/storage', 'Storage locations')
  const partialTest = async route => { const response = await route.fetch(); const body = await response.json(); await route.fulfill({ response, json: { ...body, warnings: ['synthetic transport warning'] } }) }
  await page.route(`**/api/v1/storage/${storage.id}/test`, partialTest)
  await page.getByRole('button', { name: 'Test Connection', exact: true }).click()
  await page.getByText('Partial', { exact: true }).waitFor()
  check(true, 'HTTP200 warning is partial after a real local-provider test')
  await page.unroute(`**/api/v1/storage/${storage.id}/test`, partialTest)
  const mixedCopies = async route => { const response = await route.fetch(); const rows = await response.json(); await route.fulfill({ response, json: rows.map(row => row.id === run.id ? { ...row, remote_sync_details: [{ storage_id: storage.id, storage_name: storage.name, status: 'failed' }] } : row) }) }
  await page.route('**/api/v1/backups?*', mixedCopies)
  await ready('/history', 'Archives')
  await page.getByText('partial', { exact: true }).first().waitFor()
  check(true, 'transport-injected HTTP200 mixed copy result is partial in history, not completed')
  await page.unroute('**/api/v1/backups?*', mixedCopies)

  await page.getByRole('button', { name: /^Notifications \(/ }).click()
  const center = page.getByRole('dialog', { name: 'Notifications', exact: true })
  await center.getByRole('article', { name: 'Operation ' + job.id, exact: true }).waitFor()
  await center.getByRole('link', { name: `Open operation #${job.id}`, exact: true }).waitFor()
  check(true, 'global entry exposes canonical operation per-item results and typed link')
  await center.getByRole('link', { name: `Open operation #${job.id}`, exact: true }).click()
  await page.getByLabel(`Operation ${job.id} context`, { exact: true }).getByText('Result: completed', { exact: true }).waitFor()
  check(await page.evaluate(id => JSON.parse(sessionStorage.getItem('dockervault-notifications-v1')).items.some(item => item.target?.id === id && item.read), job.id), 'clicking the typed notification opens confirmed operation context and marks it read')
  await page.getByRole('button', { name: /^Notifications \(/ }).click()
  await center.getByRole('button', { name: 'Mark all as read', exact: true }).click()
  check(await page.evaluate(() => { const saved = JSON.parse(sessionStorage.getItem('dockervault-notifications-v1')); return saved?.version === 1 && saved.items.every(item => item.read) }), 'mark-all-read retained with schema/verified owner')
  const saved = await page.evaluate(() => sessionStorage.getItem('dockervault-notifications-v1'))
  check(saved && !saved.includes('notification-delete') && !saved.includes('synthetic transport warning') && !saved.includes('/app/data'), 'retention contains no paths or provider diagnostics')
  await page.keyboard.press('Escape')
  check(await page.getByRole('button', { name: /^Notifications \(/ }).evaluate(element => element === document.activeElement), 'notification Escape returns focus')
  await page.reload()
  await page.getByRole('button', { name: /^Notifications \(/ }).click()
  await page.getByRole('link', { name: `Open operation #${job.id}`, exact: true }).waitFor()
  check(true, 'verified same-owner reload retains linked result')
  await center.getByRole('button', { name: 'Clear all', exact: true }).click()
  check((await api('/operations/' + job.id)).state === 'completed', 'notification clear is not cancellation or a destructive operation')
  await page.keyboard.press('Escape')
  await page.reload()
  await page.getByRole('button', { name: /^Notifications \(/ }).click()
  await center.waitFor()
  check(!await center.getByRole('link', { name: `Open operation #${job.id}`, exact: true }).isVisible(), 'dismissed final notification stays dismissed across refetch and reload')
  await center.getByRole('link', { name: 'Inspect operation ' + job.id, exact: true }).waitFor()
  check(true, 'cleared notification does not hide the canonical operation record')
  await page.keyboard.press('Escape')

  for (const width of [320, 390, 768, 1024, 1440]) {
    await page.setViewportSize({ width, height: 900 })
    for (const [route, , label] of routes) {
      await ready(route, label)
      check(await page.getByLabel(label + ' status', { exact: true }).evaluate(element => { const box = element.getBoundingClientRect(); return box.left >= -1 && box.right <= innerWidth + 1 }), `${route}: query feedback fits ${width}px`)
    }
    await ready('/storage', 'Storage locations')
    check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), `storage feedback reflows at ${width}px`)
    await page.getByRole('button', { name: /^Notifications \(/ }).click()
    await center.waitFor()
    check(await center.evaluate(element => { const box = element.getBoundingClientRect(); return box.left >= -1 && box.right <= innerWidth + 1 }), `global operations fits ${width}px`)
    await page.keyboard.press('Escape')
  }
  await page.setViewportSize({ width: 1440, height: 1000 })
  check(await page.evaluate(() => matchMedia('(prefers-reduced-motion: reduce)').matches), 'reduced-motion preference is active in the real browser')
  const keyboardRead = page.waitForResponse(response => new URL(response.url()).pathname === '/api/v1/storage' && response.ok())
  await page.getByRole('button', { name: 'Refresh Storage locations', exact: true }).focus()
  await page.keyboard.press('Enter')
  await keyboardRead
  check(true, 'keyboard Enter refresh performs a real authoritative read')
  await ready('/protection', 'Plans')
  await page.route(base + '/__axe_probe.js', async route => route.fulfill({ status: 200, contentType: 'application/javascript', body: await readFile('/runner/axe.min.js', 'utf8') }))
  await page.addScriptTag({ url: base + '/__axe_probe.js' })
  let audit = await page.evaluate(async () => axe.run({ include: ['[aria-label="Plans status"]'] }, { runOnly: { type: 'tag', values: ['wcag2a', 'wcag2aa', 'wcag21aa'] } }))
  check(audit.violations.length === 0, 'axe query feedback WCAG A/AA scope: ' + JSON.stringify(audit.violations))
  await page.getByRole('button', { name: /^Notifications \(/ }).click()
  await center.waitFor()
  await page.waitForFunction(() => document.activeElement?.closest('[role="dialog"]'))
  await page.keyboard.press('Tab')
  await page.waitForFunction(() => document.activeElement?.closest('[role="dialog"]'))
  check(await center.evaluate(element => element.contains(document.activeElement)), 'notification keyboard focus stays within the dialog')
  audit = await page.evaluate(async () => axe.run({ include: ['[role="dialog"]'] }, { runOnly: { type: 'tag', values: ['wcag2a', 'wcag2aa', 'wcag21aa'] } }))
  check(audit.violations.length === 0, 'axe global notifications/operations A/AA scope: ' + JSON.stringify(audit.violations))
  await page.keyboard.press('Escape')
  // Oversized history uses real completed backups, not injected success rows.
  for (let index = 0; index < 60; index++) {
    // Respect the unchanged native POST /backups 10/minute gate; this is fixture pacing, not a UI timing assertion.
    if (index > 0 && index % 9 === 0) await new Promise(resolve => setTimeout(resolve, 61000))
    // Native archive names have second resolution per plan; distinct owned plans avoid intentional overwrite rejection.
    const batchTarget = await api('/targets', 'POST', { name: `Synthetic batch plan ${index}`, target_type: 'path', host_path: '/app/data/ux183-source', enabled: true, compression_enabled: true })
    const next = await api('/backups', 'POST', { target_id: batchTarget.id, backup_type: 'full' })
    let final
    for (let attempt = 0; attempt < 300; attempt++) { final = await api('/backups/' + next.id); if (!['pending', 'running'].includes(final.status)) break; await new Promise(resolve => setTimeout(resolve, 50)) }
    assert.equal(final.status, 'completed')
  }
  const realHistory = await api('/backups?limit=500')
  check(realHistory.length === 61 && realHistory.every(row => row.status === 'completed'), 'B3: oversized history consists of 61 real completed backend runs')
  const newestIds = [...realHistory].sort((a, b) => Date.parse(b.completed_at) - Date.parse(a.completed_at) || b.id - a.id).slice(0, 50).map(row => row.id)
  await ready('/history', 'Archives')
  await page.waitForFunction(() => JSON.parse(sessionStorage.getItem('dockervault-notifications-v1'))?.items.filter(item => item.target?.kind === 'run').length === 50)
  let retained = await page.evaluate(() => JSON.parse(sessionStorage.getItem('dockervault-notifications-v1')).items)
  check(JSON.stringify(retained.map(item => item.target.id)) === JSON.stringify(newestIds), 'B3: newest 50 backend results survive the bounded history ingestion')
  await page.getByRole('button', { name: /^Notifications \(/ }).click()
  await center.getByRole('button', { name: 'Mark all as read', exact: true }).click()
  await page.keyboard.press('Escape')
  await page.getByRole('button', { name: 'Refresh Archives', exact: true }).click()
  await page.getByLabel('Archives status', { exact: true }).filter({ hasText: /updated/ }).waitFor()
  await page.reload()
  await page.getByLabel('Archives status', { exact: true }).filter({ hasText: /updated/ }).waitFor()
  retained = await page.evaluate(() => JSON.parse(sessionStorage.getItem('dockervault-notifications-v1')).items)
  check(retained.length === 50 && retained.every(item => item.read) && JSON.stringify(retained.map(item => item.target.id)) === JSON.stringify(newestIds), 'B3: read state and latest membership survive identical oversized refetch and verified-owner reload')
  await page.getByRole('button', { name: /^Notifications \(/ }).click()
  await center.getByRole('button', { name: 'Clear all', exact: true }).click()
  await page.keyboard.press('Escape')
  await page.reload()
  await page.getByLabel('Archives status', { exact: true }).filter({ hasText: /updated/ }).waitFor()
  await page.getByRole('button', { name: /^Notifications \(/ }).click()
  await center.getByText('No notifications', { exact: true }).waitFor()
  retained = await page.evaluate(() => JSON.parse(sessionStorage.getItem('dockervault-notifications-v1')).items)
  check(retained.length === 50 && retained.every(item => item.dismissed), 'B3: clear stays clear after oversized backend replay and reload within the same 50-record cap')
  await page.keyboard.press('Escape')
  check(pageErrors.length === 0, 'review regressions and existing journeys have no uncaught application errors')
  await page.getByRole('button', { name: 'Log out', exact: true }).click()
  await page.getByRole('button', { name: /Sign in/i }).waitFor()
  check(await page.evaluate(() => sessionStorage.getItem('dockervault-notifications-v1')) === null, 'logout clears retention immediately')
  console.log(JSON.stringify({ status: 'passed', browser_checks: checks.length, checks, boundary: 'Chromium/Linux synthetic instance; transport faults explicit; no physical device or native AT claim' }))
} finally { await browser.close() }
