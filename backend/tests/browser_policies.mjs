// #187 six-AC acceptance: native cookie/API/SQLite/files; declared faults only.
import assert from 'node:assert/strict'
import { randomBytes } from 'node:crypto'
import { readFile, writeFile } from 'node:fs/promises'
import { chromium } from 'playwright'
import { expect } from 'playwright/test'
const browser = await chromium.launch({ args: ['--no-sandbox', '--disable-dev-shm-usage'] })
const context = await browser.newContext({ locale: 'en-US', timezoneId: 'UTC', reducedMotion: 'reduce' })
const page = await context.newPage()
page.setDefaultTimeout(15000)
const base = 'http://127.0.0.1'
const checks = []
const check = (ok, label) => { assert.ok(ok, label); checks.push(label); console.log('PASS ' + label) }
async function response(path, method = 'GET', data) { return context.request.fetch(base + '/api/v1' + path, { method, data }) }
async function api(path, method = 'GET', data) { const result = await response(path, method, data); assert.ok(result.ok(), `${method} ${path}: ${result.status()} ${await result.text()}`); return result.json() }
async function control(action, data = {}) { return api('/test-policy187/' + action, 'POST', data) }
async function goto(path) { await page.goto(base + path) }
async function savePolicy(kind, keyboard = false) { const button = page.getByRole('button', { name: 'Save', exact: true }); if (keyboard) await button.press('Enter'); else await button.click(); await page.waitForURL(new RegExp(`/protection/${kind}/[1-9]\\d*(\\?|$)`)); await page.getByRole('link', { name: 'Edit', exact: true }).waitFor(); return Number(new URL(page.url()).pathname.split('/')[3]) }
async function settled(targetId) {
  // Runner Playwright1.63 treats an async waitForFunction's Promise as truthy; poll the actual HTTP result.
  await expect.poll(async () => (await api('/backups?target_id=' + targetId))[0]?.status, { timeout: 15000, intervals: [100] }).toBe('completed')
  return (await api('/backups?target_id=' + targetId))[0]
}
async function field(name, value) { await page.getByLabel(name, { exact: true }).fill(value) }
async function editPlan(id, schedule, retention) {
  await goto(`/protection/${id}/edit?returnTo=${encodeURIComponent('/protection/retention?q=Managed')}`)
  await page.getByRole('heading', { name: /^Edit protection:/ }).waitFor()
  await page.getByRole('button', { name: 'Strategy', exact: true }).click()
  await page.getByRole('combobox', { name: 'Select Schedule', exact: true }).selectOption(schedule)
  await page.getByRole('combobox', { name: 'Retention Policy', exact: true }).selectOption(retention)
  await page.getByRole('button', { name: 'Execution & review', exact: true }).click()
  await page.getByRole('button', { name: 'Save plan', exact: true }).click()
  await page.waitForURL(new RegExp(`/protection/${id}\\?`))
  return api('/targets/' + id)
}
try {
  await goto('/')
  const password = randomBytes(24).toString('hex')
  await field('Username', 'policy-operator')
  await field('Account password', password)
  await field('Confirm account password', password)
  await page.getByRole('button', { name: 'Create account', exact: true }).click()
  await page.getByRole('button', { name: /Continue without encryption/ }).click()
  await page.getByRole('button', { name: 'Create first backup plan', exact: true }).click()
  await page.waitForURL('**/protection/new')
  check((await context.cookies()).some(cookie => cookie.name === 'session_token' && cookie.httpOnly), 'real HttpOnly cookie authentication')

  // A1: real defaults, browser-side invalid values, real backend 422, retained inputs.
  await goto('/protection/retention/new?q=Managed')
  check(await page.getByRole('spinbutton', { name: 'Keep Last', exact: true }).inputValue() === '3', 'A1 Keep Last defaults to supported schema value 3')
  await field('Policy name', 'Managed defaults')
  for (const [label, value] of [['Keep Last', ''], ['Keep Last', '0'], ['Keep Last', '1001'], ['Max age (days)', '0'], ['Max age (days)', '36501']]) {
    await field(label, value)
    await page.getByRole('button', { name: 'Save', exact: true }).press('Enter')
    check(await page.getByLabel(label, { exact: true }).getAttribute('aria-invalid') === 'true', `A1 inline ${label}=${value || 'empty'}; route/values retained`)
    await field(label, label === 'Keep Last' ? '3' : '365')
  }
  const rejectKeepLast = async route => {
    if (route.request().method() !== 'POST') return route.continue()
    const body = route.request().postDataJSON()
    await route.continue({ postData: JSON.stringify({ ...body, keep_last: 0 }) })
  }
  await page.route('**/api/v1/retention', rejectKeepLast)
  await page.getByRole('button', { name: 'Save', exact: true }).click()
  await page.getByRole('spinbutton', { name: 'Keep Last', exact: true }).locator('xpath=..').waitFor()
  await page.waitForFunction(() => document.querySelector('input[aria-invalid="true"]') !== null)
  check(await page.getByLabel('Policy name', { exact: true }).inputValue() === 'Managed defaults' && page.url().includes('/new'), 'A1 real native 422 under explicitly modified request preserves form and route')
  await page.unroute('**/api/v1/retention', rejectKeepLast)
  const retentionId = await savePolicy('retention', true)
  const retention = await api('/retention/' + retentionId)
  check(JSON.stringify([retention.keep_last, retention.keep_daily, retention.keep_weekly, retention.keep_monthly, retention.keep_yearly, retention.max_age_days]) === JSON.stringify([3, 7, 4, 6, 2, 365]), 'A1 first successful create GET confirms every documented default')
  await page.getByText(/0 effective plans; 0 removed references/).waitFor()
  check((await page.locator('main').innerText()).includes('0 effective plans; 0 removed references'), 'A3 zero-assignment usage is authoritative, not unknown')

  // A2: authoritative validation/preview, create/edit/description-clear/toggle/jobs/run.
  await goto('/protection/schedules/new?q=Managed')
  await field('Schedule name', 'Managed schedule')
  await field('Cron expression', 'not cron')
  await field('Description', 'Clear this description')
  await page.getByRole('button', { name: 'Save', exact: true }).click()
  await page.waitForFunction(() => document.querySelector('input[aria-invalid="true"]') !== null)
  check(await page.getByLabel('Cron expression', { exact: true }).getAttribute('aria-invalid') === 'true' && await page.getByLabel('Description', { exact: true }).inputValue() === 'Clear this description', 'A2 native invalid cron is field-linked and recoverable')
  await page.getByRole('combobox', { name: 'Preset', exact: true }).selectOption('0 3 * * *')
  const scheduleId = await savePolicy('schedules')
  let schedule = await api('/schedules/' + scheduleId)
  const preview = await api('/schedules/preview', 'POST', { cron_expression: schedule.cron_expression })
  await page.getByRole('heading', { name: 'Actual scheduler jobs', exact: true }).waitFor()
  await page.getByText(new RegExp('server timezone: ' + preview.timezone)).waitFor()

  check((await page.locator('main').innerText()).includes(preview.timezone), 'A2 displayed server timezone agrees with native preview')
  check((await page.locator('main').innerText()).includes(new Date(preview.next_runs[0]).toLocaleString('en-US', { timeZone: 'UTC' })), 'A2 displayed next occurrence agrees with native preview')
  const dst = await api('/schedules/preview', 'POST', { cron_expression: '30 2 * * *', base_time: '2027-03-13T00:00:00Z', count: 3 })
  check(dst.timezone === 'America/New_York' && dst.next_runs.length === 3 && dst.next_runs.every(text => Number.isFinite(Date.parse(text))), 'A2 native timezone transition preview uses configured America/New_York (no client cron engine)')
  await page.getByRole('link', { name: 'Edit', exact: true }).click()
  await field('Schedule name', 'Managed renamed')
  await field('Description', '')
  await field('Cron expression', '0 4 * * sun')
  await savePolicy('schedules')
  schedule = await api('/schedules/' + scheduleId)
  check(schedule.name === 'Managed renamed' && schedule.description === '' && schedule.cron_expression === '0 4 * * sun', 'A2 edit and intentional description clear persisted in API GET')
  // Fast re-edit must read the just-saved state, never an earlier query baseline.
  await page.getByRole('link', { name: 'Edit', exact: true }).click()
  check(await page.getByLabel('Schedule name', { exact: true }).inputValue() === 'Managed renamed' && await page.getByLabel('Description', { exact: true }).inputValue() === '', 'A2 fast re-edit initializes from fresh saved GET')
  await field('Description', 'Second edit')
  await savePolicy('schedules')
  check((await api('/schedules/' + scheduleId)).description === 'Second edit', 'A2 second immediate edit persists successfully')

  const newPlan = async (name, extra = {}) => api('/targets', 'POST', { name, target_type: 'path', host_path: '/app/data/184-fixtures/path', compression_enabled: true, stop_container: false, dependencies: [], ...extra })
  const one = await newPlan('Assigned one', { schedule_id: scheduleId, retention_policy_id: retentionId })
  const two = await newPlan('Assigned disabled', { schedule_id: scheduleId, retention_policy_id: retentionId, enabled: false })
  const retired = await newPlan('Assigned removed', { schedule_id: scheduleId, retention_policy_id: retentionId })
  await api('/targets/' + retired.id, 'DELETE')
  const implicit = await newPlan('Default implicit', { retention_policy_id: null, enabled: false })
  await goto('/protection/schedules/' + scheduleId)
  await page.getByRole('button', { name: 'Disable schedule Managed renamed', exact: true }).click()
  await page.getByRole('button', { name: 'Enable schedule Managed renamed', exact: true }).waitFor()
  check((await api('/schedules/' + scheduleId)).enabled === false && !(await api('/schedules/jobs')).some(job => job.id === 'backup_target_' + one.id), 'A2 disable readback and actual scheduler jobs')
  await page.getByRole('button', { name: 'Enable schedule Managed renamed', exact: true }).click()
  await page.getByRole('button', { name: 'Disable schedule Managed renamed', exact: true }).waitFor()
  check((await api('/schedules/' + scheduleId)).enabled === true && (await api('/schedules/jobs')).some(job => job.id.includes(String(one.id))), 'A2 enable reconciles a native target job')
  await page.getByRole('button', { name: 'Run All', exact: true }).click()
  await page.getByText(/Accepted 1 runs, skipped 1/).waitFor()
  const actualRun = await settled(one.id)
  check(actualRun.status === 'completed' && actualRun.file_size > 0, 'A2 run-now accepted/skipped is distinct from actual completed archive')
  // A distinct plan avoids the existing intentional same-second archive-name collision guard.
  await api('/targets/' + one.id, 'PUT', { enabled: false })
  await api('/targets/' + two.id, 'PUT', { enabled: true })
  let batchPosts = 0
  const loseBatch = async route => { batchPosts++; const native = await route.fetch(); const outcome = await native.json(); check(native.ok() && outcome.triggered_targets.length === 1, 'A2 lost batch response follows actual native acceptance'); await route.abort() }
  await page.route('**/api/v1/schedules/' + scheduleId + '/trigger-all', loseBatch)
  await page.getByRole('button', { name: 'Run All', exact: true }).click()
  await page.getByText(/Action result not confirmed/).waitFor()
  check(await page.getByRole('button', { name: 'Run All', exact: true }).isDisabled() && batchPosts === 1, 'A2 unknown non-idempotent run batch is locked without blind replay')
  const uncertainRun = await settled(two.id)
  check(uncertainRun.file_size > 0 && (await control('snapshot')).backups.some(row => row.id === uncertainRun.id && row.exists), 'A2 unknown batch really completed a second file-backed native archive')
  await page.unroute('**/api/v1/schedules/' + scheduleId + '/trigger-all', loseBatch)
  await api('/targets/' + one.id, 'PUT', { enabled: true })
  await api('/targets/' + two.id, 'PUT', { enabled: false })
  check((await api('/targets/' + one.id)).enabled && !(await api('/targets/' + two.id)).enabled, 'A2 isolated batch target-state transitions restored and verified by GET')

  // A3: explicit, null/default, disabled and removed references; real plan-editor readbacks.
  await goto('/protection/retention/' + retentionId + '?q=Managed')
  await page.getByRole('link', { name: 'Assigned removed', exact: true }).waitFor()
  check((await page.locator('main').innerText()).includes('2 effective plans; 1 removed references'), 'A3 authoritative 2 active including disabled plus 1 removed FK')
  check(await page.getByRole('button', { name: 'Delete policy Managed defaults', exact: true }).isDisabled(), 'A3 assigned-policy deletion blocked before request')
  check((await response('/retention/' + retentionId, 'DELETE')).status() === 409, 'A3 backend preserves assigned/removed FK deletion constraint')
  let plan = await editPlan(one.id, 'none', '')
  check(plan.schedule_id === null && plan.schedule_cron === null && plan.retention_policy_id === null, 'A3 browser clear verified by subsequent native plan GET, including legacy cron')
  check((await page.getByRole('link', { name: 'Return to protection list', exact: true }).getAttribute('href')) === '/protection/retention?q=Managed', 'A3 usage -> plan editor -> readback detail preserves policy return context')
  plan = await editPlan(one.id, String(scheduleId), String(retentionId))
  check(plan.schedule_id === scheduleId && plan.retention_policy_id === retentionId, 'A3 intentional reassignment verified by subsequent native plan GET')
  const defaultPolicy = (await api('/retention')).find(policy => policy.name === 'default')
  await goto('/protection/retention/' + defaultPolicy.id)
  await page.getByRole('link', { name: 'Default implicit', exact: true }).waitFor()
  check((await page.locator('main').innerText()).includes('1 effective plans; 0 removed references'), 'A3 exactly one default/null user has the correct linked usage count')
  check((await page.locator('main').innerText()).includes('Default fallback (null assignment)'), 'A3 default/null fallback is linked rather than falsely unused')
  check((await response('/retention/' + defaultPolicy.id, 'DELETE')).status() === 400, 'A4 native default-name deletion constraint unchanged')
  await api('/targets/' + two.id, 'PUT', { retention_policy_id: null })
  await api('/targets/' + one.id, 'PUT', { retention_policy_id: null })
  check((await api('/targets/' + two.id)).retention_policy_id === null && (await api('/targets/' + one.id)).retention_policy_id === null, 'A3 additional explicit clears read back individually')
  check((await response('/retention/' + retentionId, 'DELETE')).status() === 409, 'A3 removed-only FK still blocks deletion after active clears')

  // A4: all field boundaries, true GFS union/age/KeepLast effects; no fabricated dry run.
  await goto('/protection/retention/' + retentionId + '/edit')
  await field('Policy name', 'Managed bounds')
  for (const label of ['Keep Last', 'Daily', 'Weekly', 'Monthly', 'Yearly']) await field(label, '1000')
  await field('Max age (days)', '36500')
  await savePolicy('retention')
  let bound = await api('/retention/' + retentionId)
  check(bound.keep_last === 1000 && bound.keep_yearly === 1000 && bound.max_age_days === 36500 && bound.name === 'Managed bounds', 'A4 rename and inclusive upper schema boundaries persist')
  await goto('/protection/retention/' + retentionId + '/edit')
  for (const label of ['Daily', 'Weekly', 'Monthly', 'Yearly']) await field(label, '0')
  await field('Keep Last', '1'); await field('Max age (days)', '1')
  await savePolicy('retention')
  bound = await api('/retention/' + retentionId)
  check(bound.keep_last === 1 && bound.keep_daily === 0 && bound.max_age_days === 1, 'A4 inclusive lower boundaries, only GFS zero supported')
  check((await page.locator('main').innerText()).includes('not a destructive dry run'), 'A4 explanation never claims unsupported destructive preview')
  const agePlan = await newPlan('Age effects', { retention_policy_id: retentionId, enabled: false })
  const ageRows = await control('seed-pruning', { target_id: agePlan.id, kind: 'age' })
  const ageResult = await api('/retention/' + agePlan.id + '/apply', 'POST')
  let snapshot = await control('snapshot')
  const ageRemaining = snapshot.backups.filter(row => ageRows.ids.includes(row.id))
  check(ageResult.deleted === 2 && ageRemaining.length === 1 && ageRemaining[0].id === ageRows.ids[0] && ageRemaining[0].exists, 'A4 real age pruning removes two old files/rows but KeepLast protects newest old archive')
  const gfsPolicy = await api('/retention', 'POST', { name: 'GFS effects', keep_last: 1, keep_daily: 2, keep_weekly: 0, keep_monthly: 0, keep_yearly: 0, max_age_days: 365 })
  const gfsPlan = await newPlan('GFS effects', { retention_policy_id: gfsPolicy.id, enabled: false })
  const gfsRows = await control('seed-pruning', { target_id: gfsPlan.id, kind: 'gfs' })
  const gfsResult = await api('/retention/' + gfsPlan.id + '/apply', 'POST')
  snapshot = await control('snapshot')
  check(gfsResult.deleted === 1 && snapshot.backups.filter(row => gfsRows.ids.includes(row.id) && row.exists).length === 2, 'A4 real GFS daily union protects two, not KeepLast+Daily summed three')
  await goto('/protection/retention/' + defaultPolicy.id + '/edit')
  for (const label of ['Daily', 'Weekly', 'Monthly', 'Yearly']) await field(label, '0')
  await field('Keep Last', '1'); await field('Max age (days)', '1')
  await savePolicy('retention', true)
  const updatedDefault = await api('/retention/' + defaultPolicy.id)
  check(updatedDefault.name === 'default' && updatedDefault.keep_last === 1 && updatedDefault.max_age_days === 1, 'A4 default policy edit persists through native GET')
  const defaultRows = await control('seed-pruning', { target_id: implicit.id, kind: 'age' })
  const defaultResult = await api('/retention/' + implicit.id + '/apply', 'POST')
  snapshot = await control('snapshot')
  check(defaultResult.deleted === 2 && snapshot.backups.filter(row => defaultRows.ids.includes(row.id) && row.exists).length === 1 && (await api('/targets/' + implicit.id)).retention_policy_id === null, 'A4 actual default/null fallback uses edited rules to prune real files/SQLite')
  const unused = await api('/retention', 'POST', { name: 'Unused deletion' })
  await goto('/protection/retention/' + unused.id)
  await page.getByRole('button', { name: 'Delete policy Unused deletion', exact: true }).click()
  await page.getByRole('dialog', { name: 'Delete Retention Policy', exact: true }).getByRole('button', { name: 'Delete', exact: true }).click()
  await page.waitForURL(/\/protection\/retention$/)
  check((await response('/retention/' + unused.id)).status() === 404, 'A4 unassigned deletion verified by authoritative absence')

  // A5: global orphan scope, real filesystem/SQLite, all four final result classes.
  const cleanupPlan = await newPlan('Cleanup catalog fixture', { enabled: false })
  for (const mode of ['empty', 'success', 'partial', 'failure']) {
    await control('cleanup', { mode, target_id: cleanupPlan.id })
    await goto('/protection/retention')
    await page.getByRole('button', { name: 'Cleanup', exact: true }).click()
    const dialog = page.getByRole('dialog', { name: 'Cleanup Orphaned Files', exact: true })
    check((await dialog.innerText()).includes('Global backup-base directory scan') && (await dialog.innerText()).includes('not retention pruning'), 'A5 '+mode+' review declares real global orphan scope')
    let release, entered
    const gate = new Promise(resolve => { release = resolve })
    const accepted = new Promise(resolve => { entered = resolve })
    const hold = async route => { const native = await route.fetch(); entered(); await gate; await route.fulfill({ response: native }) }
    if (mode === 'success') await page.route('**/api/v1/retention/cleanup-orphaned', hold)
    await dialog.getByRole('button', { name: 'Cleanup', exact: true }).click()
    if (mode === 'success') {
      await accepted
      await dialog.getByRole('button', { name: 'Processing...', exact: true }).waitFor()
      check(await dialog.getAttribute('aria-busy') === 'true', 'A5 held native response retains pending state, not premature completion')
      await page.keyboard.press('Escape')
      for (let index = 0; index < 6; index++) {
        await page.keyboard.press('Tab')
        const active = await dialog.evaluate(node => ({ inside: node.contains(document.activeElement), guard: document.activeElement?.matches('[data-base-ui-focus-guard]'), html: document.activeElement?.outerHTML }))
        assert.ok(active.inside || active.guard, 'Pending focus reached background: ' + active.html)
        await expect.poll(() => dialog.evaluate(node => node.contains(document.activeElement)), { timeout: 15000 }).toBe(true)
        check(await dialog.evaluate(node => node.contains(document.activeElement)), 'A5 pending focus cannot reach modal background')
      }
      check(await dialog.isVisible(), 'A5 pending Escape cannot dismiss action custody')
      release()
    }
    await dialog.getByRole('heading', { name: 'Cleanup result: ' + mode, exact: true }).waitFor()
    const counts = { empty: '0 deleted; 0 failed', success: '2 deleted; 0 failed', partial: '1 deleted; 1 failed', failure: '0 deleted; 2 failed' }
    check((await dialog.innerText()).includes(counts[mode]), 'A5 '+mode+' visible exact final counters match independent expected effects')
    if (mode === 'success') await page.unroute('**/api/v1/retention/cleanup-orphaned', hold)
    snapshot = await control('snapshot')
    const remaining = mode === 'partial' ? [true, false] : mode === 'failure' ? [true, true] : [false, false]
    check(snapshot.files['orphan0.tar.gz'] === remaining[0] && snapshot.files['orphan1.tar.gz'] === remaining[1], 'A5 '+mode+' API outcome agrees with actual isolated orphan files')
    check(snapshot.files['catalog.tar.gz'] && snapshot.files['active.tar.gz.tmp'] && snapshot.foreign_key_violations === 0, 'A5 '+mode+' catalog/active files and real SQLite protected')
    for (let index = 0; index < 6; index++) { await page.keyboard.press(index % 2 ? 'Shift+Tab' : 'Tab'); check(await dialog.evaluate(node => node.contains(document.activeElement)), 'A5 '+mode+' result keyboard focus remains in modal') }
    check(await dialog.getByRole('button', { name: 'Cleanup', exact: true }).count() === 0, 'A5 '+mode+' does not silently offer destructive replay from result')
    await dialog.getByRole('button', { name: 'Close', exact: true }).click()
  }
  // Lose an actual native response after successful effects, not a fabricated response body.
  await control('cleanup', { mode: 'success', target_id: cleanupPlan.id })
  const lose = async route => { const native = await route.fetch(); check(native.ok(), 'A5 lost response was a real successful native cleanup'); await route.abort() }
  await page.route('**/api/v1/retention/cleanup-orphaned', lose)
  await page.getByRole('button', { name: 'Cleanup', exact: true }).click()
  let dialog = page.getByRole('dialog', { name: 'Cleanup Orphaned Files', exact: true })
  await dialog.getByRole('button', { name: 'Cleanup', exact: true }).click()
  await dialog.getByRole('alert').filter({ hasText: /unknown/ }).waitFor()
  snapshot = await control('snapshot')
  check(!snapshot.files['orphan0.tar.gz'] && !snapshot.files['orphan1.tar.gz'] && await dialog.getByRole('button', { name: 'Cleanup', exact: true }).count() === 0, 'A5 lost response remains unknown despite confirmed real effects, without automatic retry')
  await dialog.getByRole('button', { name: 'Close', exact: true }).click()
  await page.unroute('**/api/v1/retention/cleanup-orphaned', lose)

  // A2 reconcile: real committed schedule + injected scheduler failure, no duplicate POST.
  await control('reconcile-once')
  let creates = 0
  const observe = request => { if (request.method() === 'POST' && new URL(request.url()).pathname === '/api/v1/schedules') creates++ }
  page.on('request', observe)
  await goto('/protection/schedules/new')
  await field('Schedule name', 'Committed reconciliation')
  await page.getByRole('button', { name: 'Save', exact: true }).click()
  await page.getByText(/Write outcome requires verification/).waitFor()
  check(await page.getByRole('button', { name: 'Save', exact: true }).isDisabled() && (await api('/schedules')).filter(row => row.name === 'Committed reconciliation').length === 1, 'A2 postcommit503 keeps unknown Create locked with one real persisted row')
  await page.getByRole('button', { name: 'Reload and inspect server state', exact: true }).click()
  await page.waitForURL(/\/protection\/schedules$/)
  check(creates === 1 && (await api('/schedules/jobs')).length > 0, 'A2 native reload/jobs reconcile without second Create')
  page.off('request', observe)

  // A3 schedule delete unlinks even retained FK inventory; GET verifies each active plan.
  await goto('/protection/schedules/' + scheduleId)
  await page.getByRole('button', { name: 'Delete schedule Managed renamed', exact: true }).click()
  dialog = page.getByRole('dialog', { name: 'Delete Schedule', exact: true })
  check((await dialog.innerText()).includes('legacy cron'), 'A3 schedule deletion review explains unlink/legacy fallback impact')
  await dialog.getByRole('button', { name: 'Delete', exact: true }).click()
  await page.waitForURL(/\/protection\/schedules$/)
  check((await response('/schedules/' + scheduleId)).status() === 404 && (await api('/targets?include_removed=true')).every(target => target.schedule_id !== scheduleId), 'A3 schedule deletion verifies policy absence and all active/removed unlinks')

  // A6: native routes, safe context, query filters, narrow reflow, keyboard cancel/save.
  const filteredSchedule = await api('/schedules', 'POST', { name: 'Filter inactive', description: 'Find description', enabled: false, cron_expression: '0 3 * * *' })
  await goto('/protection/schedules?q=Find&status=disabled')
  await page.getByRole('link', { name: 'Filter inactive', exact: true }).waitFor()
  check(await page.getByRole('searchbox').inputValue() === 'Find', 'A6 description search and disabled schedule filter from durable URL')
  await page.getByRole('combobox', { name: 'Status', exact: true }).selectOption('enabled')
  await page.getByText('No matching policies.', { exact: true }).waitFor()
  check((await api('/schedules')).length > 0, 'A6 enabled schedule filter gives truthful filtered-empty state')
  await goto('/protection/retention?q=Managed&usage=used')
  await page.getByRole('link', { name: 'Managed bounds', exact: true }).click()
  await page.getByRole('link', { name: 'Edit', exact: true }).click()
  await page.getByRole('button', { name: 'Cancel', exact: true }).press('Enter')
  await page.getByRole('link', { name: 'Back to Retention Policies', exact: true }).click()
  check(await page.getByRole('searchbox').inputValue() === 'Managed' && new URL(page.url()).searchParams.get('usage') === 'used', 'A6 list/detail/edit/cancel/back retains query filters')
  await page.getByRole('link', { name: 'Managed bounds', exact: true }).click()
  await page.getByRole('heading', { name: 'Managed bounds', exact: true }).waitFor()
  await page.reload()
  await page.getByRole('heading', { name: 'Managed bounds', exact: true }).waitFor()
  check(new URL(page.url()).searchParams.get('q') === 'Managed' && new URL(page.url()).searchParams.get('usage') === 'used' && (await page.locator('main').innerText()).includes('Keep Last: 1;'), 'A6 actual detail reload restores persisted identity/rules and filter context')
  await page.goBack()
  await page.getByRole('link', { name: 'Managed bounds', exact: true }).waitFor()
  check(await page.getByRole('searchbox').inputValue() === 'Managed' && new URL(page.url()).searchParams.get('usage') === 'used', 'A6 actual browser Back returns to the filtered policy list')
  await page.getByRole('searchbox').fill('definitely no match')
  await page.getByText('No matching policies.', { exact: true }).waitFor()
  check((await api('/retention')).length > 0, 'A6 filtered empty is not backend empty')
  const directRoutes = [
    { path: '/protection/retention/new', heading: 'New Retention Policy', form: 'retention', fresh: true, value: '' },
    { path: '/protection/retention/' + retentionId, heading: 'Managed bounds' },
    { path: '/protection/retention/' + retentionId + '/edit', heading: 'Edit policy', form: 'retention', value: 'Managed bounds' },
    { path: '/protection/schedules/new', heading: 'New Schedule', form: 'schedules', fresh: true, value: '' },
    { path: '/protection/schedules/' + filteredSchedule.id, heading: 'Filter inactive' },
    { path: '/protection/schedules/' + filteredSchedule.id + '/edit', heading: 'Edit policy', form: 'schedules', value: 'Filter inactive' },
  ]
  for (const entry of directRoutes) {
    await goto(entry.path)
    await page.getByRole('heading', { name: entry.heading, exact: true }).waitFor()
    if (entry.form === 'retention') {
      check(await page.getByLabel('Policy name', { exact: true }).inputValue() === entry.value && await page.getByLabel('Keep Last', { exact: true }).inputValue() === (entry.fresh ? '3' : '1') && await page.getByLabel('Max age (days)', { exact: true }).inputValue() === (entry.fresh ? '365' : '1'), 'A6 direct retention form is finished with correct name/rules '+entry.path)
    } else if (entry.form === 'schedules') {
      check(await page.getByLabel('Schedule name', { exact: true }).inputValue() === entry.value && await page.getByLabel('Cron expression', { exact: true }).inputValue() === (entry.fresh ? '0 2 * * *' : '0 3 * * *') && await page.getByLabel('Description', { exact: true }).inputValue() === (entry.fresh ? '' : 'Find description') && await page.getByRole('checkbox', { name: 'Enable schedule', exact: true }).isChecked() === !!entry.fresh, 'A6 direct schedule form is finished with correct identity/cron/description/state '+entry.path)
    } else {
      const text = await page.locator('main').innerText()
      check(entry.heading === 'Managed bounds' ? text.includes('Keep Last: 1;') && text.includes('Bucket age cutoff: 1 days.') : text.includes('Find description') && text.includes('Cron: 0 3 * * *') && text.includes('Disabled'), 'A6 direct detail is finished with correct persisted policy identity/rules '+entry.path)
    }
  }
  await goto('/protection/schedules/edit')
  await page.getByRole('alert').filter({ hasText: 'This address is not a valid policy route.' }).waitFor()
  check(!(await page.locator('main').innerText()).includes('Invalid protection plan address'), 'A6 invalid static schedule/edit is rejected as a policy route, not a plan ID')
  await goto('/schedules'); await page.waitForURL('**/protection/schedules'); check(true, 'A6 legacy schedule route redirects')
  await goto('/retention'); await page.waitForURL('**/protection/retention'); check(true, 'A6 legacy retention route redirects')
  await page.setViewportSize({ width: 320, height: 720 })
  for (const path of ['/protection/retention', '/protection/retention/' + retentionId + '/edit', '/protection/schedules/new']) {
    await goto(path)
    await page.locator('main h1').waitFor()
    if (path === '/protection/retention') await page.getByRole('link', { name: 'Default implicit', exact: true }).waitFor()
    else if (path.endsWith('/edit')) {
      await page.getByRole('heading', { name: 'Usage and edit impact', exact: true }).waitFor()
      assert.equal(await page.getByLabel('Policy name', { exact: true }).inputValue(), 'Managed bounds')
    } else await page.getByText(/Next runs \(server timezone:/).waitFor()
    check(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1), 'A6 320 CSS px reflow '+path)
  }
  await page.route(base + '/__policy187_axe.js', async route => route.fulfill({ status: 200, contentType: 'application/javascript', body: await readFile('/runner/axe.min.js', 'utf8') }))
  await page.addScriptTag({ url: base + '/__policy187_axe.js' })
  const axe = await page.evaluate(async () => window.axe.run(document.querySelector('main')))
  check(axe.violations.length === 0, 'A6 narrow policy editor axe including contrast has no violations')
  snapshot = await control('snapshot')
  check(snapshot.foreign_key_violations === 0 && snapshot.backups.find(row => row.id === actualRun.id)?.exists, 'final native SQLite has no FK violations and run-now archive remains')
  const result = { status: 'passed', checks: checks.length, labels: checks, retentionId, scheduleId, runId: actualRun.id, boundary: 'Chromium/Linux emulation only; fixture-only controls and synthetic discovery/lifecycle; native policy/auth/SQLite/files/engine. Injected unlink/reconcile faults and explicitly lost/modified transport named per assertion.' }
  await writeFile('/runner/policy187-result.json', JSON.stringify(result, null, 2))
  console.log(JSON.stringify(result))
} finally { await browser.close() }
