import assert from 'node:assert/strict'
import { randomBytes } from 'node:crypto'
import { chromium } from 'playwright'
import { expect } from 'playwright/test'
const [image, revision] = process.argv.slice(2)
const base = 'http://127.0.0.1'
const browser = await chromium.launch()
try {
  const ctx = await browser.newContext({locale:'en-US', timezoneId:'UTC', reducedMotion:'reduce'})
  const page = await ctx.newPage()
  async function api(path, method='GET', data) {
    const {status, body} = await page.evaluate(async ({path,method,data}) => {
      const response = await fetch('/api/v1'+path, {method,
        ...(data === undefined ? {} : {headers:{'Content-Type':'application/json'},body:JSON.stringify(data)})})
      return {status:response.status,body:await response.json()}
    }, {path,method,data})
    assert.ok(status >= 200 && status < 300, `${method} ${path}: ${status}`)
    return body
  }
  const field = (label, value) => page.getByLabel(label,{exact:true}).fill(value)
  const named = (role,name) => page.getByRole(role,{name,exact:true})
  const save = async kind => {
    await named('button','Save').click()
    await page.waitForURL(new RegExp(`/protection/${kind}/[1-9]\\d*(\\?|$)`))
    await named('link','Edit').waitFor()
    return Number(new URL(page.url()).pathname.split('/')[3])
  }
  await page.goto(base)
  const password=randomBytes(24).toString('hex')
  await named('button','Create new installation').click()
  await field('Username','policy-browser')
  await field('Account password',password)
  await field('Confirm account password',password)
  await named('button','Create account').click()
  await page.getByRole('button',{name:/Continue without encryption/}).click()
  await named('button','Continue to DockerVault').click()
  await expect.poll(async () => (await ctx.cookies()).some(c => c.name === 'session_token' && c.httpOnly)).toBe(true)
  await page.goto(base+'/protection/schedules/new')
  await field('Schedule name','Browser schedule')
  await field('Cron expression','not cron')
  await field('Description','Clear me')
  await named('button','Save').click()
  await expect(page.getByLabel('Cron expression',{exact:true})).toHaveAttribute('aria-invalid','true')
  assert.equal((await api('/schedules')).length,0)
  await named('combobox','Preset').selectOption('0 3 * * *')
  const id=await save('schedules')
  const initial=await api('/schedules/'+id)
  assert.equal(initial.cron_expression,'0 3 * * *')
  const preview=await api('/schedules/preview','POST',{cron_expression:initial.cron_expression})
  await expect(page.getByText('server timezone: '+preview.timezone,{exact:false})).toBeVisible()
  await named('link','Edit').click()
  await field('Schedule name','Browser schedule edited')
  await field('Description','')
  await field('Cron expression','0 4 * * sun')
  assert.equal(await save('schedules'),id)
  await page.reload()
  await named('link','Edit').click()
  await expect(page.getByLabel('Schedule name',{exact:true})).toHaveValue('Browser schedule edited')
  await expect(page.getByLabel('Description',{exact:true})).toHaveValue('')
  const edited=await api('/schedules/'+id)
  assert.equal(edited.description,'')
  assert.equal(edited.cron_expression,'0 4 * * sun')
  const dst=await api('/schedules/preview','POST',{cron_expression:'30 2 * * *',base_time:'2027-03-13T00:00:00Z',count:3})
  assert.equal(dst.timezone,'America/New_York')
  assert.deepEqual(dst.next_runs,['2027-03-13T02:30:00-05:00','2027-03-14T02:30:00-05:00','2027-03-15T02:30:00-04:00'])
  await page.goto(base+'/protection/retention/new')
  await field('Policy name','Browser retention')
  await expect(named('spinbutton','Keep Last')).toHaveValue('3')
  await field('Keep Last','1')
  const retention=await save('retention')
  assert.equal((await api('/retention/'+retention)).keep_last,1)
  await page.reload()
  await named('link','Edit').click()
  await expect(named('spinbutton','Keep Last')).toHaveValue('1')
  console.log(JSON.stringify({revision,image,chromium:browser.version(),invalid_cron_did_not_save:true,displayed_timezone_agrees:true,dst_offsets_match:true,schedule_clear_persisted:true,retention_policy_persisted:true}))
} finally { await browser.close() }
