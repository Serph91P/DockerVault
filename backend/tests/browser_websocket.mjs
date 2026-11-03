// Real Chromium/cookie acceptance. No raw password/token is logged or put in URLs.
import assert from 'node:assert/strict'
import { randomBytes } from 'node:crypto'
import { chromium } from 'playwright'

const base = 'https://127.0.0.1:8443'
const socketURL = 'wss://127.0.0.1:8443/ws/updates'
const browser = await chromium.launch({ args: ['--no-sandbox', '--disable-dev-shm-usage'] })
const context = await browser.newContext({ ignoreHTTPSErrors: true })
const password = randomBytes(24).toString('hex')
const nextPassword = randomBytes(24).toString('hex')
async function until(predicate, timeout = 15000) {
  const end = Date.now() + timeout
  while (!predicate()) {
    assert.ok(Date.now() < end, 'browser acceptance condition did not settle')
    await new Promise(resolve => setTimeout(resolve, 20))
  }
}
async function denied(page) {
  return page.evaluate(url => new Promise(resolve => {
    let opened = false
    const socket = new WebSocket(url)
    socket.onopen = () => { opened = true; socket.close() }
    socket.onclose = event => resolve({ opened, code: event.code })
    socket.onerror = () => {}
  }), socketURL)
}
try {
  let ready = false
  const deadline = Date.now() + 30000
  while (!ready && Date.now() < deadline) {
    try { ready = (await context.request.get(`${base}/health`)).ok() } catch {}
    if (!ready) await new Promise(resolve => setTimeout(resolve, 100))
  }
  assert.ok(ready, 'owned backend did not become healthy')
  const anonymous = await browser.newContext({ ignoreHTTPSErrors: true })
  try {
    const page = await anonymous.newPage()
    await page.goto(`${base}/health`)
    assert.deepEqual(await denied(page), { opened: false, code: 1006 })
  } finally { await anonymous.close() }
  console.log('PASS initial anonymous WebSocket handshake denied (HTTP denial, browser 1006)')

  const setup = await context.request.post(`${base}/api/v1/auth/setup`, {
    data: { username: 'owned-browser-admin', password, confirm_password: password },
  })
  assert.equal(setup.status(), 200)
  const cookie = (await context.cookies(base)).find(value => value.name === 'session_token')
  assert.ok(cookie)
  assert.equal(cookie.httpOnly, true)
  assert.equal(cookie.secure, true)
  assert.equal(cookie.sameSite, 'Lax')

  const closes = new Map()
  await context.exposeBinding('__recordClose', ({ page }, code) => {
    const values = closes.get(page) ?? []
    values.push(code)
    closes.set(page, values)
  })
  await context.addInitScript(() => {
    window.__frames = []
    const Original = window.WebSocket
    window.WebSocket = class extends Original {
      constructor(...args) {
        super(...args)
        this.addEventListener('message', event => { window.__frames.push(JSON.parse(event.data)) })
        this.addEventListener('close', event => { window.__recordClose(event.code) })
      }
    }
  })
  const page = await context.newPage()
  let attempts = 0
  page.on('websocket', () => { attempts += 1 })
  await page.goto(base)
  await page.waitForFunction(() => window.__frames.some(frame => frame.type === 'connected'))
  assert.equal(await page.evaluate(() => document.cookie.includes('session_token')), false)
  assert.ok(attempts > 0)
  console.log('PASS compiled frontend opens authenticated WSS; real Secure/HttpOnly/SameSite=Lax cookie is unreadable to page JS')

  const foreign = await context.newPage()
  await foreign.goto('https://127.0.0.1:8444')
  assert.deepEqual(await denied(foreign), { opened: false, code: 1006 })
  await foreign.close()
  console.log('PASS foreign same-site/different-port Origin denied; independent server evidence records whether browser sent the cookie')

  for (const action of ['logout', 'password']) {
    if (action === 'password') {
      const login = await context.request.post(`${base}/api/v1/auth/login`, {
        data: { username: 'owned-browser-admin', password },
      })
      assert.equal(login.status(), 200)
      await page.goto(base)
      await page.waitForFunction(() => window.__frames.some(frame => frame.type === 'connected'))
    }
    const beforeAttempts = attempts
    const beforeCloses = (closes.get(page) ?? []).length
    const response = action === 'logout'
      ? await context.request.post(`${base}/api/v1/auth/logout`)
      : await context.request.post(`${base}/api/v1/auth/change-password`, {
          data: { current_password: password, new_password: nextPassword, confirm_password: nextPassword },
        })
    assert.equal(response.status(), 200)
    await until(() => (closes.get(page) ?? []).length > beforeCloses)
    assert.equal(closes.get(page).at(-1), 4001)
    // Observe one actual 5-second frontend retry window, not a mocked timer.
    await new Promise(resolve => setTimeout(resolve, 5500))
    assert.equal(attempts, beforeAttempts)
    if (action === 'logout') {
      assert.equal((await context.cookies(base)).some(value => value.name === 'session_token'), false)
    } else {
      assert.equal((await context.request.get(`${base}/api/v1/auth/me`)).status(), 200)
    }
    console.log(`PASS real API ${action} closes existing browser WSS with 4001; compiled client does not reconnect`)
  }
} finally {
  await context.close()
  await browser.close()
}
