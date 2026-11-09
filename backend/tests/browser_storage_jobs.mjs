// Production UI + real cookie/SQLite/files. Fault holds are external test fixtures.
import assert from 'node:assert/strict'
import { randomBytes, randomUUID } from 'node:crypto'
import { readFile, writeFile } from 'node:fs/promises'
import { chromium } from 'playwright'

const phase = process.env.PHASE || 'ordinary'
const base = 'http://127.0.0.1'
const browser = await chromium.launch({ args: ['--no-sandbox', '--disable-dev-shm-usage'] })
const context = await browser.newContext(phase === 'ordinary' ? {} : { storageState: '/runner/storage142-session.json' })
const page = await context.newPage()
const checks = []
const check = (condition, label) => { assert.ok(condition, label); checks.push(label) }
async function api(path, method = 'GET', data) {
  const response = await context.request.fetch(base + '/api/v1' + path, { method, data })
  assert.ok(response.ok(), `${method} ${path} status ${response.status()}`)
  return response.json()
}
async function done(id) {
  for (let i = 0; i < 300; i++) {
    const job = await api('/operations/' + id)
    if (!['queued', 'running'].includes(job.state)) return job
    await page.waitForTimeout(50)
  }
  throw new Error('Operation did not settle')
}
async function open() {
  await page.goto(base + '/storage')
  await page.getByTitle('Browse Files').first().click()
  return page.getByRole('region', { name: 'Browse Local Backups' })
}
async function runDelete(panel, names) {
  await panel.getByRole('button', { name: 'Select All', exact: true }).click()
  await panel.getByRole('button', { name: 'Clear Selection', exact: true }).click()
  for (const name of names) await panel.getByRole('row').filter({ has: page.getByText(name, { exact: true }) }).click()
  await panel.locator('div.overflow-y-auto').evaluate(e => { e.scrollTop = 100 })
  await panel.getByRole('button', { name: 'Delete', exact: true }).click()
  const confirmation = page.getByRole('dialog', { name: 'Delete Files' })
  const accepted = page.waitForResponse(r => r.url().endsWith('/api/v1/operations') && r.request().method() === 'POST')
  await confirmation.getByRole('button', { name: 'Delete', exact: true }).dblclick()
  const response = await accepted
  check(response.status() === 202, 'delete is accepted, not reported completed')
  const { operation_id } = await response.json()
  await confirmation.waitFor({ state: 'hidden' })
  return operation_id
}
try {
  if (phase === 'ordinary') {
    await page.goto(base)
    const password = randomBytes(24).toString('hex')
    await page.getByRole('button', { name: 'Create new installation', exact: true }).click()
    await page.getByLabel('Username', { exact: true }).fill('synthetic-storage-operator')
    await page.getByLabel('Account password', { exact: true }).fill(password)
    await page.getByLabel('Confirm account password').fill(password)
    await page.getByRole('button', { name: 'Create account', exact: true }).click()
    await page.getByRole('button', { name: /Continue without encryption/ }).click()
    await page.getByRole('button', { name: 'Continue to DockerVault' }).click()
    await context.storageState({ path: '/runner/storage142-session.json' })
    check((await context.cookies()).some(c => c.name === 'session_token' && c.httpOnly), 'real HttpOnly cookie authentication')
    let panel = await open()
    await panel.getByRole('row', { name: /nested-delete/ }).getByText('1.5 KB', { exact: true }).waitFor()
    check(true, '#141 asynchronous recursive sizes retained')
    await panel.getByRole('button', { name: 'Select All', exact: true }).click()
    const count = (await api('/storage/local/files')).files.length
    check(await panel.getByText(`${count} selected`, { exact: true }).isVisible(), 'Select All includes files, directories and links')
    check(await panel.getByRole('checkbox', { name: 'Select all entries' }).isChecked(), 'native all checkbox checked')
    await panel.getByRole('row').filter({ has: page.getByText('nested-delete', { exact: true }) }).click()
    check(await panel.getByRole('checkbox', { name: 'Select all entries' }).evaluate(e => e.indeterminate), 'native partial selection state')
    await panel.getByRole('button', { name: 'Clear Selection' }).click()
    check(await panel.getByText('0 selected', { exact: true }).isVisible(), 'clear and selected count visible')
    // Keyboard selection works with native checkbox; navigation remains available.
    await panel.getByRole('checkbox', { name: 'Select all entries' }).focus()
    await page.keyboard.press('Space')
    check(await panel.getByRole('checkbox', { name: 'Select all entries' }).isChecked(), 'keyboard native selection')
    await panel.getByRole('button', { name: 'Clear Selection' }).click()
    let creates = 0
    page.on('request', r => { if (r.url().endsWith('/api/v1/operations') && r.method() === 'POST') creates++ })
    const id = await runDelete(panel, ['nested-delete', 'leaf-link', 'ordinary-file', 'locked'])
    const job = await done(id)
    check(job.state === 'failed' && job.total === 4 && job.completed === 4 && job.succeeded === 3 && job.failed === 1 && job.unknown === 0, 'real mixed-permission results and exact counters, not success')
    await panel.getByRole('button', { name: 'Refresh operations' }).click()
    const article = panel.getByRole('article', { name: `Operation ${id}` })
    await article.getByText('Permission denied', { exact: true }).waitFor()
    check(await panel.locator('div.overflow-y-auto').evaluate(e => e.scrollTop) === 100, 'listing scroll position retained after deletion and refresh')
    await panel.getByRole('button', { name: 'Refresh operations' }).focus()
    const deniedStatus = '**/api/v1/operations?*'
    await page.route(deniedStatus, route => route.abort('failed'))
    await panel.getByRole('button', { name: 'Refresh operations' }).click()
    await panel.getByRole('alert').filter({ hasText: 'Operation status unavailable' }).waitFor()
    check(await panel.getByRole('button', { name: 'Refresh operations' }).evaluate(e => e === document.activeElement), 'live stale-error announcement preserves keyboard focus')
    check(await article.getByRole('button', { name: 'Retry failed items' }).isDisabled(), 'stale results cannot start a retry')
    await page.unroute(deniedStatus)
    await panel.getByRole('button', { name: 'Refresh operations' }).click()
    await article.getByRole('button', { name: 'Retry failed items' }).waitFor()
    await page.waitForFunction(() => !document.querySelector('[role="alert"]')?.textContent?.includes('Operation status unavailable'))
    check(await panel.isVisible() && await panel.getByText('0 selected', { exact: true }).isVisible(), 'panel and selection mode retained after partial failure')
    check(creates === 1, 'double-click submitted once')
    const unauthenticated = await browser.newContext()
    check((await unauthenticated.request.get(base + '/api/v1/operations/' + id)).status() === 401, 'missing authentication cannot inspect operation')
    await unauthenticated.close()
    await writeFile('/runner/storage142-mixed.json', JSON.stringify({ id }))
    // Current folder and scroll context survive successful deletion.
    await panel.getByTitle('Select files').click()
    await panel.getByRole('row').filter({ has: page.getByText('context-folder', { exact: true }) }).click()
    const contextId = await runDelete(panel, ['context-file'])
    check((await done(contextId)).state === 'completed', 'nested browser deletion actually completed')
    await panel.getByRole('button', { name: 'Refresh operations' }).click()
    check(await panel.getByRole('button', { name: 'context-folder', exact: true }).isVisible(), 'current path retained after success')
    check(await panel.isVisible(), 'successful operation leaves browser open')
    await panel.getByRole('button', { name: 'Close storage browser' }).click()
    const configured = await api('/storage', 'POST', { name: 'Configured synthetic', storage_type: 'local', base_path: '/app/data/configured', enabled: true })
    await page.reload()
    await page.getByRole('heading', { name: 'Configured synthetic', exact: true }).waitFor()
    await page.getByTitle('Browse Files').last().click()
    panel = page.getByRole('region', { name: 'Browse Configured synthetic' })
    const configuredId = await runDelete(panel, ['configured-folder', 'configured-file'])
    const configuredJob = await done(configuredId)
    check(configuredJob.state === 'completed' && configuredJob.succeeded === 2 && configuredJob.storage_id === configured.id, 'configured root physical directory and file operation completed')
    await panel.getByRole('button', { name: 'Refresh operations' }).click()
    check(await panel.getByText(configuredId, { exact: true }).isVisible(), 'configured root operations discoverable')
    for (const width of [320, 390]) {
      await page.setViewportSize({ width, height: 840 })
      check(await panel.getByRole('button', { name: 'Select All', exact: true }).isVisible(), `${width}px selection controls visible`)
      check(await panel.evaluate(e => e.getBoundingClientRect().left >= 0 && e.getBoundingClientRect().right <= innerWidth), `${width}px modal fits viewport`)
    }
  } else if (phase === 'retry') {
    const { id } = JSON.parse(await readFile('/runner/storage142-mixed.json', 'utf8'))
    const panel = await open()
    const article = panel.getByRole('article', { name: `Operation ${id}` })
    const accepted = page.waitForResponse(r => r.url().endsWith(`/operations/${id}/retry`))
    await article.getByRole('button', { name: 'Retry failed items' }).click()
    const child = (await (await accepted).json()).operation_id
    const result = await done(child)
    check(result.parent_id === id && result.state === 'completed' && result.total === 1 && result.items[0].path === 'locked', 'failed-only child retry physically completed')
    check((await api(`/operations/${id}`)).succeeded === 3 && (await api(`/operations/${id}`)).failed === 1, 'original outcomes immutable after retry')
  } else if (phase === 'held-start' || phase === 'restart-start' || phase === 'cancel-start') {
    const prefix = phase.split('-')[0]
    const panel = await open()
    const names = prefix === 'cancel' ? ['held-cancel-one', 'held-cancel-two'] : [prefix === 'held' ? 'held-delete' : 'restart-delete', prefix === 'held' ? 'held-future' : 'restart-future']
    const id = await runDelete(panel, names)
    for (let i = 0; i < 100; i++) {
      if ((await api('/operations/' + id)).state === 'running') break
      await page.waitForTimeout(20)
    }
    await page.reload()
    const reopened = await open()
    await reopened.getByText(id, { exact: true }).waitFor()
    check((await api('/operations/' + id)).state === 'running', 'active operation rediscovered after reload without resubmission')
    await writeFile(`/runner/storage142-${prefix}.json`, JSON.stringify({ id }))
    if (prefix === 'cancel') {
      const article = reopened.getByRole('article', { name: `Operation ${id}` })
      await article.getByRole('button', { name: 'Cancel operation' }).click()
      await article.getByText(/Cancellation requested/).waitFor()
      const pending = await api('/operations/' + id)
      check(pending.cancel_requested && pending.completed === 0 && pending.state === 'running', 'cancel persisted but physical effect not falsely settled')
    }
  } else if (phase === 'held-settle' || phase === 'cancel-settle') {
    const prefix = phase.split('-')[0]
    const { id } = JSON.parse(await readFile(`/runner/storage142-${prefix}.json`, 'utf8'))
    await page.routeWebSocket('**', socket => socket.close())
    let polls = 0
    page.on('request', r => { if (r.url().includes('/api/v1/operations?')) polls++ })
    const panel = await open()
    const article = panel.getByRole('article', { name: `Operation ${id}` })
    await article.getByText(/running -/).waitFor()
    console.log('CHECKPOINT polling-ready')
    await article.getByText(prefix === 'held' ? /completed - 2\/2/ : /cancelled - 2\/2/).waitFor({ timeout: 15000 })
    check(polls >= 2, 'WebSocket loss falls back to active 2s polling and settles UI')
    const job = await api('/operations/' + id)
    check(prefix === 'held' ? job.succeeded === 2 : job.succeeded === 1 && job.skipped === 1, 'actual held effect and skipped future counters match')
    check(await panel.isVisible(), 'held result retains open panel')
  } else if (phase === 'restart-check') {
    const { id } = JSON.parse(await readFile('/runner/storage142-restart.json', 'utf8'))
    const panel = await open()
    const article = panel.getByRole('article', { name: `Operation ${id}` })
    await article.getByText(/Interrupted effects are unknown/).waitFor()
    const job = await api('/operations/' + id)
    check(job.state === 'failed' && job.unknown === 1 && job.skipped === 1 && job.completed === 2, 'real backend process loss becomes unknown+skipped, never completed')
    check(await article.getByRole('button', { name: 'Retry failed items' }).count() === 0, 'unknown effects not offered as retryable failures')
    const retry = await context.request.post(base + `/api/v1/operations/${id}/retry`, { data: { request_id: randomUUID() } })
    check(retry.status() === 409, 'unknown-only retry refused by authoritative API')
  } else throw new Error('Unknown phase')
  console.log(JSON.stringify({ status: 'passed', phase, checks }))
} finally {
  await browser.close()
}
