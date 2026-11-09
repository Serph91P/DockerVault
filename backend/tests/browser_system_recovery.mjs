// #143 actual standard-product UI, no recovery/auth/API interception.
import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { chmod, readFile, writeFile } from 'node:fs/promises'
import { chromium } from 'playwright'
import { expect } from 'playwright/test'
const phase = process.env.DV143_PHASE, seed = process.env.DV143_SEED
const marker = label => createHash('sha256').update(seed + ':' + label).digest('hex')
const shared = '/app/data/system143-retained', base = 'http://127.0.0.1', checks = []
const browser = await chromium.launch({ args: ['--no-sandbox','--disable-dev-shm-usage'] })
const context = await browser.newContext({ acceptDownloads:true, locale:'en-US', viewport:{width:390,height:844} })
const page = await context.newPage(); page.setDefaultTimeout(15000)
const check = (value,label) => { assert.ok(value,label); checks.push(label); console.log('PASS '+label) }
const secrets = [seed, ...['account','bundle','replacement','komodo:key','komodo:secret','pre-hook','post-hook','crypt:password','crypt:salt'].map(marker), ...['local','ssh','sftp','webdav','s3','ftp','rclone'].flatMap(kind=>['password','s3_access_key','s3_secret_key'].map(field=>marker(kind+':'+field)))]
let ageKey = '', exports = 0, imports = 0, encryptionSetups = 0
page.on('request', r => { if(r.method()==='POST'){if(r.url().endsWith('/recovery/export'))exports++;if(r.url().endsWith('/recovery/restore'))imports++;if(r.url().endsWith('/encryption/setup'))encryptionSetups++} })
async function api(path, method='GET', data) { const response=await context.request.fetch(base+'/api/v1'+path,{method,data}); assert.ok(response.ok(),`${method} ${path}: HTTP${response.status()}`); return response.json() }
async function scan() {
 const snapshot=await page.evaluate(()=>{
  const root=document.getElementById('root'), key=Object.keys(root).find(key=>key.startsWith('__reactContainer$'))
  const start=root[key]?.stateNode?.current??root[key], queue=[start], clients=new Set(), seen=new Set()
  while(queue.length){const fiber=queue.pop();if(!fiber||seen.has(fiber))continue;seen.add(fiber);queue.push(fiber.child,fiber.sibling);for(const value of [fiber.memoizedProps?.client,fiber.memoizedProps?.value])if(value&&typeof value.getQueryCache==='function')clients.add(value)}
  const values=[...clients].map(client=>({queries:client.getQueryCache().getAll().map(query=>query.state),mutations:client.getMutationCache().getAll().map(mutation=>mutation.state)}))
  const visited=new WeakSet()
  return {clients:clients.size,bytes:JSON.stringify({url:location.href,local:{...localStorage},session:{...sessionStorage},history:history.state,values},(key,value)=>{if(value&&typeof value==='object'){if(visited.has(value))return '[cycle]';visited.add(value)}return value})}
 })
 check(snapshot.clients>0,'actual running React Query client located')
 check(secrets.every(value=>!snapshot.bytes.includes(value))&&(!ageKey||!snapshot.bytes.includes(ageKey)),'canaries absent from actual query/mutation cache, URL, history and browser persistence')
}
async function widths(label) {
 for(const width of [320,390,768,1024,1440]){
  await page.setViewportSize({width,height:900})
  check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`${label} ${width}px no overflow`)
  if(!await page.evaluate(()=>Boolean(window.axe)))await page.addScriptTag({path:'/runner/axe.min.js'})
  const violations=await page.evaluate(async()=>{const result=await window.axe.run(document);return result.violations.map(item=>({id:item.id,impact:item.impact,nodes:item.nodes.map(node=>node.target)}))})
  await writeFile(`/runner/system143-${phase}-${label}-${width}-axe.json`,JSON.stringify(violations))
  check(violations.length===0,`${label} ${width}px automated axe`)
  await page.screenshot({path:`/runner/system143-${phase}-${label}-${width}.png`,fullPage:true})
 }
 await page.evaluate(()=>{document.body.style.zoom='2'})
 check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),label+' 200% zoom no overflow')
 await page.evaluate(()=>{document.body.style.zoom=''})
}
async function chooseFile(password=marker('bundle')) {
 await page.getByRole('button',{name:'Restore system backup',exact:true}).click()
 await page.getByLabel('System backup file',{exact:true}).setInputFiles(shared+'/bundle.dockervault')
 await expect(page.getByLabel('System-backup password',{exact:true})).toBeEnabled()
 await page.getByLabel('System-backup password',{exact:true}).fill(password)
}
async function preview() {
 await page.getByRole('button',{name:'Validate and preview',exact:true}).click()
 await expect(page.getByRole('heading',{name:'Validated preview — not restored',exact:true})).toBeFocused()
}
async function restore(username) {
 await page.getByLabel('New administrator username',{exact:true}).fill(username)
 await page.getByLabel('New account password',{exact:true}).fill(marker('replacement'))
 await page.getByLabel('Confirm new account password',{exact:true}).fill(marker('replacement'))
 await page.getByRole('button',{name:'Restore configuration and create administrator',exact:true}).dblclick()
}
try {
 await page.goto(base)
 if(phase==='source'){
  check(await page.getByLabel('Username',{exact:true}).count()===0,'choice precedes account creation')
  await widths('choice')
  await page.getByRole('button',{name:'Create new installation',exact:true}).click()
  await page.getByLabel('Username',{exact:true}).fill('system-operator')
  await page.getByLabel('Account password',{exact:true}).fill(marker('account'))
  await page.getByLabel('Confirm account password',{exact:true}).fill(marker('account'))
  await page.getByRole('button',{name:'Create account',exact:true}).click()
  await page.getByRole('button',{name:'Set Up Encryption',exact:true}).click()
  const next=page.waitForEvent('download')
  await page.getByRole('button',{name:'Download Private Key File',exact:true}).click()
  const identity=await next;await identity.saveAs('/runner/external-system143.key');await chmod('/runner/external-system143.key',0o600)
  ageKey=(await readFile('/runner/external-system143.key','utf8')).match(/^AGE-SECRET-KEY-1[A-Z0-9]+$/m)[0]
  await page.getByRole('button',{name:'I have saved my private key',exact:true}).click()
  await page.getByRole('button',{name:'Continue to DockerVault',exact:true}).click()
  await api('/storage/ssh-keys','POST',{name:'original',comment:'synthetic public reconnect reference'})
  await api('/test-system143/seed','POST')
  const job=await api('/backups','POST',{target_id:42,backup_type:'full'})
  await expect.poll(async()=>{const result=await api('/backups/'+job.id);assert.ok(!['failed','cancelled'].includes(result.status),'source backup failed');return result.status},{timeout:90000}).toBe('completed')
  await api('/test-system143/finalize','POST')
  await page.goto(base+'/settings#system-recovery')
  await widths('export')
  await page.getByLabel('System-backup password',{exact:true}).fill(marker('bundle'))
  await page.getByLabel('Confirm system-backup password',{exact:true}).fill(marker('bundle'))
  const downloadable=page.waitForEvent('download'), response=page.waitForResponse(r=>r.url().endsWith('/recovery/export'))
  await page.getByRole('button',{name:'Download encrypted system backup',exact:true}).click()
  const download=await downloadable;await download.saveAs(shared+'/bundle.dockervault')
  check((await response).headers()['cache-control']==='no-store','actual export no-store')
  await expect(page.getByLabel('System-backup password',{exact:true})).toHaveValue('')
  await expect(page.getByLabel('Confirm system-backup password',{exact:true})).toHaveValue('')
  check(exports===1,'single actual export request')
  const bundle=await readFile(shared+'/bundle.dockervault','utf8')
  check(secrets.every(secret=>!bundle.includes(secret))&&!bundle.includes(ageKey),'encrypted download lacks plaintext secret canaries')
  const verified=await api('/test-system143/verify-bundle','POST',{bundle,password:marker('bundle')})
  check(verified.all_tables&&verified.providers===7&&verified.no_private_keys_or_archives,'all nine tables and seven providers inside real encrypted bundle, excluded private keys and archive content')
  await scan()
  await api('/test-system143/switch','POST')
 } else if(phase==='failure'){
  await widths('fresh-choice')
  await chooseFile('synthetic-incorrect-password')
  await page.getByRole('button',{name:'Validate and preview',exact:true}).click()
  await expect(page.getByRole('alert')).toContainText('Bundle validation failed')
  await expect(page.getByLabel('System-backup password',{exact:true})).toHaveValue('')
  check((await api('/auth/status')).setup_complete===false,'wrong password does not create setup/admin')
  await page.getByLabel('System-backup password',{exact:true}).fill(marker('bundle'))
  await preview()
  await widths('preview')
  await restore('fail-activation')
  await expect(page.getByRole('alert')).toContainText('Activation was not confirmed',{timeout:60000})
  check(await page.getByRole('heading',{name:'Configuration restored',exact:true}).count()===0,'real SQL-trigger activation failure never reported restored')
  check((await api('/auth/status')).setup_complete===false,'SQL rollback keeps first-run setup')
  await scan()
 } else {
  ageKey=(await readFile('/runner/external-system143.key','utf8')).match(/^AGE-SECRET-KEY-1[A-Z0-9]+$/m)[0]
  await chooseFile()
  await preview()
  await restore('replacement')
  await expect(page.getByRole('heading',{name:'Configuration restored',exact:true})).toBeFocused({timeout:60000})
  check(imports===1,'single actual restore activation request despite double click')
  check(await page.getByText('Storage 7 (rclone): Connected (read observation)',{exact:true}).isVisible(),'contained crypt credentials really read the retained native rclone depot')
  check(await page.getByText('Komodo: Connected (read observation)',{exact:true}).isVisible(),'contained Komodo credentials really authenticate to isolated HTTP peer')
  await widths('result')
  await page.getByRole('button',{name:'Continue to DockerVault',exact:true}).click()
  await expect(page.getByRole('region',{name:'System recovery',exact:true})).toBeVisible()
  const auth=await api('/auth/status')
  check(auth.authenticated&&auth.user.username==='replacement'&&auth.user.is_admin,'new administrator and real server session authoritative')
  check((await context.cookies()).some(cookie=>cookie.name==='session_token'&&cookie.httpOnly&&cookie.sameSite==='Lax'),'new HttpOnly session cookie')
  check(encryptionSetups===0&&(await api('/encryption/status')).setup_completed,'public age custody preserved without regenerating a key')
  const targets=await api('/targets'), schedules=await api('/schedules'), policies=await api('/retention/policies'), storages=await api('/storage')
  check(targets.some(target=>target.id===42)&&schedules.some(schedule=>schedule.id===41)&&policies.some(policy=>policy.id===40)&&storages.length===7,'actual target/schedule/policy/storage APIs after import')
  await page.reload()
  await expect(page.getByRole('heading',{name:'Previous system recovery',exact:true})).toBeVisible()
  await scan()
  await api('/test-system143/restart','POST')
  await expect.poll(async()=>{try{return (await api('/auth/status')).authenticated}catch{return false}},{timeout:30000}).toBe(true)
  await page.reload()
  await expect(page.getByRole('heading',{name:'Previous system recovery',exact:true})).toBeVisible()
  const original=JSON.parse(await readFile(shared+'/expected-public.json','utf8'))
  await page.goto(base+`/history/archives/${original.backup_id}/restore`)
  await expect(page.getByRole('heading',{name:'Source and availability',exact:true})).toBeVisible()
  await page.getByRole('button',{name:'Next',exact:true}).click()
  await page.getByRole('radio',{name:'Custom path',exact:true}).check()
  await page.getByLabel('Destination path',{exact:true}).fill('/backups/system143-restored')
  await page.getByLabel('Private recovery key',{exact:true}).fill(ageKey)
  await page.getByRole('button',{name:'Review effects',exact:true}).click()
  await expect(page.getByRole('heading',{name:'Review restore impact',exact:true})).toBeVisible()
  await page.getByLabel(/I reviewed the destination/).check()
  await page.getByRole('button',{name:'Confirm restore',exact:true}).click()
  await expect(page.getByRole('heading',{name:'Restore completed',exact:true})).toBeVisible({timeout:60000})
  await page.getByRole('button',{name:'Close restore',exact:true}).click()
  await scan()
  check(imports===1&&encryptionSetups===0,'restart and archive management do not repeat system import or age setup')
 }
 await writeFile(`/runner/system143-${phase}-result.json`,JSON.stringify({checks,phase,exports,imports,encryptionSetups},null,2))
} catch(error) {
 let message=String(error.stack??error)
 for(const value of [...secrets,ageKey].filter(Boolean))message=message.split(value).join('[REDACTED]')
 console.error(message);process.exitCode=1
} finally {await browser.close()}
