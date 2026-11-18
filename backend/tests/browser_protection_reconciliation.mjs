// Real cookie/API effects; lost/delayed responses and an explicit KeepLast payload fault are injected.
import assert from 'node:assert/strict'
import { randomBytes } from 'node:crypto'
import { chromium } from 'playwright'
const browser = await chromium.launch({ args: ['--no-sandbox', '--disable-dev-shm-usage'] })
const context = await browser.newContext()
const page = await context.newPage()
page.setDefaultTimeout(15000)
const base = 'http://127.0.0.1'
const checks = []
const check = (ok, name) => { assert.ok(ok, name); checks.push(name); console.log('PASS ' + name) }
async function api(path, method = 'GET', data) {
  const response = await context.request.fetch(base + '/api/v1' + path, { method, data })
  assert.ok(response.ok(), `${method} ${path}: ${response.status()}`)
  return response.json()
}
const group = name => page.getByRole('button', { name, exact: true }).click()
const save = () => page.getByRole('button', { name: 'Save plan', exact: true }).click()
const read = () => page.getByRole('button', { name: 'Read back candidates', exact: true }).click()
const drafts = () => page.evaluate(() => Object.keys(localStorage).filter(k => k.startsWith('dockervault-plan-draft:')).length)
async function newPath(name) {
  await page.goto(base + '/protection/new')
  await page.getByRole('heading', { name: 'Create protection plan', exact: true }).waitFor()
  await page.getByRole('button', { name: /^Host Path / }).click()
  await page.getByPlaceholder('/path/to/directory').fill('/app/data/184-fixtures/path')
  await page.getByRole('textbox', { name: 'Backup name', exact: true }).fill(name)
}
async function losePost(kind) {
  let count = 0, committed = false, release
  const gate = new Promise(resolve => { release = resolve })
  const before = kind === 'schedules' ? (await api('/schedules')).length : null
  const handler = async route => {
    if (route.request().method() !== 'POST') return route.continue()
    count++
    await gate
    const response = await route.fetch()
    assert.ok(response.ok())
    committed = true
    await route.abort('failed')
  }
  await page.route('**/api/v1/' + kind, handler)
  await save()
  await page.getByRole('heading', { name: 'Unconfirmed ' + ({ schedules: 'schedule', retention: 'retention', targets: 'target' }[kind]) + ' creation', exact: true }).waitFor()
  if (kind === 'schedules') check(!committed && (await api('/schedules')).length === before, 'forced transport gate: Unconfirmed checkpoint precedes commit; heading alone cannot synchronize durable assertions')
  release()
  await page.getByText(/Could not save backup configuration/).waitFor()
  await page.unroute('**/api/v1/' + kind, handler)
  check(count === 1 && committed, kind + ': exactly one committed POST, response lost, no automatic replay')
}
try {
  const password = randomBytes(24).toString('hex')
  await page.goto(base)
  await page.getByRole('button', { name: 'Create new installation', exact: true }).click()
  await page.getByLabel('Username', { exact: true }).fill('reconciliation-operator')
  await page.getByLabel('Account password', { exact: true }).fill(password)
  await page.getByLabel('Confirm account password').fill(password)
  await page.getByRole('button', { name: 'Create account', exact: true }).click()
  await page.getByRole('button', { name: /Continue without encryption/ }).click()
  await page.getByRole('button', { name: 'Create first backup plan', exact: true }).click()
  await page.waitForURL('**/protection/new')
  for (let i = 1; i <= 2; i++) await api('/targets', 'POST', { name: 'Foreign plan ' + i, target_type: 'path', host_path: '/app/data/184-fixtures/path' })
  const defaultPolicy = (await api('/retention'))[0]
  for (const withRetention of [true, false]) {
    const name = withRetention ? 'Reconciled retention' : 'Reconciled target'
    await newPath(name)
    await group('Strategy')
    await page.getByRole('combobox', { name: 'Select Schedule', exact: true }).selectOption('new')
    await page.getByRole('textbox', { name: 'Schedule name', exact: true }).fill(name + ' schedule')
    if (withRetention) {
      await page.getByRole('combobox', { name: 'Retention Policy', exact: true }).selectOption('new')
      await page.getByRole('textbox', { name: 'Policy name', exact: true }).fill(name + ' policy')
    }
    await group('Execution & review')
    await losePost('schedules')
    const schedule = (await api('/schedules')).find(s => s.name === name + ' schedule')
    check(schedule.id === (withRetention ? defaultPolicy.id : 2), 'fixture: schedule ID collides with foreign retention/target')
    await read()
    await page.getByRole('button', { name: 'Use explicitly selected #' + schedule.id, exact: true }).click()
    await page.getByRole('heading', { name: 'Unconfirmed schedule creation', exact: true }).waitFor({ state: 'hidden' })
    let policy
    if (withRetention) {
      await losePost('retention')
      check(await page.getByRole('button', { name: /^Use explicitly selected #/ }).count() === 0, 'Schedule → Retention: no stale candidate may be selected')
      policy = (await api('/retention')).find(p => p.name === name + ' policy')
      check(policy.id !== defaultPolicy.id, 'fixture: intended retention differs from colliding foreign default')
      await read()
      await page.getByRole('button', { name: 'Use explicitly selected #' + policy.id, exact: true }).click()
      await page.getByRole('heading', { name: 'Unconfirmed retention creation', exact: true }).waitFor({ state: 'hidden' })
    }
    await losePost('targets')
    const target = (await api('/targets')).find(t => t.name === name)
    check(target.id !== schedule.id, 'fixture: actual target differs from stale schedule ID')
    check(await page.getByRole('button', { name: /^Open and inspect #/ }).count() === 0, 'Policy → Target: no stale candidate navigation')
    check(await drafts() > 0, 'unknown target retains safe intent draft')
    await read()
    await page.getByRole('button', { name: 'Open and inspect #' + target.id, exact: true }).click()
    await page.waitForURL(new RegExp('/protection/' + target.id + '(\\?|$)'))
    const stored = await api('/targets/' + target.id)
    check(stored.schedule_id === schedule.id && (!policy || stored.retention_policy_id === policy.id), 'real GET binds exactly selected policies, not colliding foreign IDs')
    check(await drafts() === 0, 'explicit correct target selection cleans only completed intent')
    await page.reload()
    await page.getByRole('heading', { name, exact: true }).waitFor()
  }
  // Edit existing private hooks: whitespace clears; nonempty whitespace is preserved byte-for-byte.
  const target = (await api('/targets')).find(t => t.name === 'Reconciled retention')
  const body = '  pg_dump ' + randomBytes(18).toString('hex') + '  '
  await api('/targets/' + target.id, 'PUT', { pre_backup_command: body, post_backup_command: body })
  for (const hooks of [{ pre: '   ', post: '  ' }, { pre: body, post: body }]) {
    await page.goto(base + '/protection/' + target.id + '/edit')
    await group('Execution & review')
    await page.getByRole('button', { name: 'Advanced Options', exact: true }).click()
    await page.getByRole('textbox', { name: 'Pre-backup Command', exact: true }).fill(hooks.pre)
    await page.getByRole('textbox', { name: 'Post-backup Command', exact: true }).fill(hooks.post)
    await save()
    await page.waitForURL(new RegExp('/protection/' + target.id + '(\\?|$)'))
    const stored = await api('/targets/' + target.id)
    check(stored.pre_backup_command === (hooks.pre.trim() ? hooks.pre : null) && stored.post_backup_command === (hooks.post.trim() ? hooks.post : null), 'private hook edit: correct normalized clear / unchanged nonempty readback')
    check(!(await page.evaluate(() => JSON.stringify([localStorage, sessionStorage]))).includes(body), 'private hook body absent from browser persistence')
  }
  // Delayed old response cannot populate a new same-kind, same-name intent after Discard.
  await newPath('Discarded ambiguity')
  await group('Execution & review')
  await losePost('targets')
  let release, entered
  const gate = new Promise(resolve => { release = resolve })
  const started = new Promise(resolve => { entered = resolve })
  const delayed = async route => {
    const response = await route.fetch()
    entered()
    await gate
    await route.fulfill({ response })
  }
  await page.route('**/api/v1/targets?*', delayed)
  await read()
  await started
  await page.getByRole('button', { name: 'Discard changes', exact: true }).click()
  await page.getByRole('button', { name: /^Host Path / }).click()
  await page.getByPlaceholder('/path/to/directory').fill('/app/data/184-fixtures/path')
  await page.getByRole('textbox', { name: 'Backup name', exact: true }).fill('Discarded ambiguity')
  await group('Execution & review')
  await losePost('targets')
  const responseArrived = page.waitForResponse(r => r.url().includes('/api/v1/targets?'))
  release()
  await responseArrived
  await page.unroute('**/api/v1/targets?*', delayed)
  check(await page.getByRole('button', { name: /^Open and inspect #/ }).count() === 0, 'late readback after Discard cannot select for repeated same-name intent')
  const duplicates = (await api('/targets')).filter(t => t.name === 'Discarded ambiguity')
  check(duplicates.length === 2, 'Discard disclosed orphan; new intent created once, never automatic compensation/replay')
  await read()
  const chosen = Math.max(...duplicates.map(t => t.id))
  await page.getByRole('button', { name: 'Open and inspect #' + chosen, exact: true }).click()
  await page.waitForURL(new RegExp('/protection/' + chosen + '(\\?|$)'))
  check((await api('/targets/' + chosen)).name === 'Discarded ambiguity', 'fresh explicit readback selection remains possible after stale response')
  check((await api('/backups')).length === 0, 'all recovery/save paths create zero backup runs')
  check((await api('/schedules')).length === 2 && (await api('/retention')).length === 2, 'no duplicated policy POSTs across unknown outcomes')
  // #187 now blocks KeepLast0 before POST. Keep both the native API422 and UI-adapter gates.
  await page.goto(base + '/retention')
  await page.getByRole('button', { name: 'New Policy', exact: true }).click()
  await page.getByRole('textbox', { name: 'Policy name', exact: true }).fill('Invalid policy preserved')
  await page.getByRole('spinbutton', { name: 'Keep Last', exact: true }).fill('0')
  let submitted = 0
  const count = request => { if (request.method() === 'POST' && new URL(request.url()).pathname === '/api/v1/retention') submitted++ }
  page.on('request', count)
  await page.getByRole('button', { name: 'Save', exact: true }).click()
  await page.locator('input[aria-invalid="true"]').waitFor()
  check(submitted === 0, 'KeepLast0 is now rejected locally without a POST')
  const direct = await context.request.post(base + '/api/v1/retention', { data: { name: 'Invalid native control', keep_last: 0 } })
  const directEnvelope = await direct.json()
  check(direct.status() === 422 && directEnvelope.detail === 'Validation error' && directEnvelope.errors.some(e => e.field === 'body.keep_last'), 'unintercepted native 422 uses canonical field envelope')
  await page.getByRole('spinbutton', { name: 'Keep Last', exact: true }).fill('3')
  // Explicit transport fault changes only a valid UI payload's KeepLast to0; native API emits422.
  const injectInvalid = route => route.request().method() === 'POST' ? route.continue({ postData: JSON.stringify({ ...route.request().postDataJSON(), keep_last: 0 }) }) : route.continue()
  await page.route(base + '/api/v1/retention', injectInvalid)
  const rejected = page.waitForResponse(r => new URL(r.url()).pathname === '/api/v1/retention' && r.request().method() === 'POST')
  await page.getByRole('button', { name: 'Save', exact: true }).click()
  const response = await rejected
  const envelope = await response.json()
  check(response.status() === 422 && envelope.detail === 'Validation error' && envelope.errors.some(e => e.field === 'body.keep_last'), 'real server422 under declared payload fault retains canonical field envelope')
  await page.locator('input[aria-invalid="true"]').waitFor()
  const field = page.getByRole('spinbutton', { name: 'Keep Last', exact: true })
  check(await field.getAttribute('aria-invalid') === 'true' && !!(await field.getAttribute('aria-describedby')), 'real 422 associates static safe field feedback')
  check(await field.inputValue() === '3' && await page.getByRole('textbox', { name: 'Policy name', exact: true }).inputValue() === 'Invalid policy preserved', 'real validation keeps both safe inputs')
  check((await api('/retention')).length === 2, 'native rejected policy has no SQLite/API side effect')
  await page.unroute(base + '/api/v1/retention', injectInvalid)
  page.off('request', count)
  console.log(JSON.stringify({ status: 'passed', checks: checks.length, fixture: 'real Auth/API/SQLite; injected lost/delayed responses and KeepLast payload fault; synthetic discovery/lifecycle; no providers' }))
} finally { await browser.close() }
