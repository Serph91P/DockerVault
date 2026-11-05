import assert from 'node:assert/strict'
import { randomBytes } from 'node:crypto'
import { chromium } from 'playwright'

// Hold real HTTP snapshots, not mocked business effects or acceptance responses.
const scenario = process.env.SCENARIO
const browser = await chromium.launch({ args: ['--no-sandbox', '--disable-dev-shm-usage'] })
const context = await browser.newContext()
const page = await context.newPage()
const base = 'http://127.0.0.1'
const requests = []
let release = () => {}
try {
  await page.routeWebSocket('**', socket => socket.close())
  await page.goto(base)
  const password = randomBytes(24).toString('hex')
  await page.getByLabel('Username', { exact: true }).fill('recovery-operator')
  await page.getByLabel('Account password', { exact: true }).fill(password)
  await page.getByLabel('Confirm account password').fill(password)
  await page.getByRole('button', { name: 'Create account', exact: true }).click()
  await page.getByRole('button', { name: /Continue without encryption/ }).click()
  await page.getByRole('button', { name: 'Continue to DockerVault' }).click()
  let reads = 0
  let entered
  const gotRead = new Promise(resolve => { entered = resolve })
  const hold = new Promise(resolve => { release = resolve })
  if (scenario === 'discovery') {
    await page.route('**/api/v1/operations?*', async route => {
      reads++
      if (reads === 1) {
        const response = await route.fetch()
        assert.deepEqual(await response.json(), [])
        entered()
        await hold
        // The browser may have already cancelled this obsolete request.
        await route.fulfill({ response }).catch(() => {})
      } else await route.continue()
    })
  }
  await page.route('**/api/v1/operations', async route => {
    if (route.request().method() !== 'POST') return route.continue()
    requests.push(route.request().postDataJSON())
    if (scenario === 'ambiguous' && requests.length === 1) {
      const response = await route.fetch()
      assert.equal(response.status(), 202)
      await route.abort('failed')
    } else await route.continue()
  })
  await page.goto(base + '/storage')
  await page.getByTitle('Browse Files').first().click()
  const panel = page.getByRole('dialog', { name: 'Browse Local Backups' })
  if (scenario === 'discovery') {
    await gotRead
    assert.equal(await panel.getByText('Loading storage operations...', { exact: true }).isVisible(), true)
    assert.equal(await panel.getByText('No recent storage operations', { exact: true }).count(), 0)
  }
  if (scenario === 'rejection') await panel.getByRole('row').filter({ has: page.getByText('folder', { exact: true }) }).click()
  await panel.getByRole('button', { name: 'Select All', exact: true }).click()
  await panel.getByText(scenario === 'rejection' ? '1001 selected' : '1 selected', { exact: true }).waitFor()
  async function submit() {
    await panel.getByRole('button', { name: 'Delete', exact: true }).click()
    const reply = page.waitForResponse(response => response.url().endsWith('/api/v1/operations') && response.request().method() === 'POST')
    await page.getByRole('dialog', { name: 'Delete Files' }).getByRole('button', { name: 'Delete', exact: true }).click()
    return reply
  }
  let acceptance
  if (scenario === 'ambiguous') {
    // No browser response is expected: the committed 202 is deliberately lost.
    await panel.getByRole('button', { name: 'Delete', exact: true }).click()
    await page.getByRole('dialog', { name: 'Delete Files' }).getByRole('button', { name: 'Delete', exact: true }).click()
    await panel.getByText(/Operation request failed/).waitFor()
    const reply = page.waitForResponse(response => response.url().endsWith('/api/v1/operations') && response.request().method() === 'POST')
    await panel.getByRole('button', { name: 'Retry request with same ID' }).click()
    acceptance = await reply
    assert.deepEqual(requests[1], requests[0])
  } else if (scenario === 'rejection') {
    const rejection = await submit()
    assert.equal(rejection.status(), 422)
    await panel.getByText(/Operation request failed/).waitFor()
    assert.deepEqual(await (await context.request.get(base + '/api/v1/operations')).json(), [])
    const listing = await (await context.request.get(base + '/api/v1/storage/local/files?path=folder')).json()
    assert.equal(listing.files.length, 1001)
    await panel.getByRole('row').filter({ has: page.getByText('item-0', { exact: true }) }).click()
    await panel.getByText('1000 selected', { exact: true }).waitFor()
    assert.equal(await panel.getByRole('button', { name: 'Delete', exact: true }).isEnabled(), true)
    assert.equal(await panel.getByRole('button', { name: 'Retry request with same ID' }).count(), 0)
    assert.equal(await panel.getByRole('button', { name: 'folder', exact: true }).isVisible(), true)
    acceptance = await submit()
    assert.notEqual(requests[0].request_id, requests[1].request_id)
    assert.equal(requests[1].paths.length, 1000)
    assert.equal(requests[1].paths.includes('folder/item-0'), false)
  } else acceptance = await submit()
  assert.equal(acceptance.status(), 202)
  const { operation_id } = await acceptance.json()
  await page.getByRole('dialog', { name: 'Delete Files' }).waitFor({ state: 'hidden' })
  release()
  await panel.getByText(operation_id, { exact: true }).waitFor()
  const article = panel.getByRole('article', { name: 'Operation ' + operation_id })
  const total = scenario === 'rejection' ? 1000 : 1
  await article.getByText(`completed - ${total}/${total} entries`, { exact: true }).waitFor({ timeout: 90000 })
  const jobs = await (await context.request.get(base + '/api/v1/operations')).json()
  assert.equal(jobs.length, 1)
  assert.equal(jobs[0].id, operation_id)
  assert.equal(jobs[0].succeeded, total)
  assert.equal(requests.length, scenario === 'discovery' ? 1 : 2)
  if (scenario === 'discovery') assert.ok(reads >= 2)
  assert.equal(await panel.isVisible(), true)
  if (scenario === 'rejection') assert.equal(await panel.getByRole('button', { name: 'folder', exact: true }).isVisible(), true)
  console.log(JSON.stringify({ status: 'passed', scenario, operation_id, requests: requests.length, total, reads, websocket: 'unavailable', durableJobs: jobs.length, panelRetained: true }))
} finally { release(); await browser.close() }
