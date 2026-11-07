// Native #186 product acceptance: real cookies/API/SQLite/engine/TAR/age/DinD.
import assert from 'node:assert/strict'
import {randomBytes} from 'node:crypto'
import {readFile,writeFile} from 'node:fs/promises'
import {chromium} from 'playwright'
import {expect} from 'playwright/test'
const browser=await chromium.launch({args:['--no-sandbox','--disable-dev-shm-usage']})
const context=await browser.newContext({locale:'en-US',timezoneId:'UTC'})
const page=await context.newPage(); page.setDefaultTimeout(15000)
const base='http://127.0.0.1', checks=[]
const check=(v,label)=>{assert.ok(v,label);checks.push(label);console.log('PASS '+label)}
async function response(path,method='GET',data){return context.request.fetch(base+'/api/v1'+path,{method,data})}
async function api(path,method='GET',data){const r=await response(path,method,data);assert.ok(r.ok(),`${method} ${path}: ${r.status()} ${await r.text()}`);return r.json()}
const goto=path=>page.goto(base+path)
const proof=()=>api('/test-restore186/proof')
const bytes=(p,name)=>Object.values(p.bytes[name]).every(Boolean)
let lastExecute=0, executeCount=0
page.on('request',r=>{if(/\/backups\/\d+\/restore$/.test(new URL(r.url()).pathname)&&r.method()==='POST')executeCount++})
async function pace(){const delay=6500-(Date.now()-lastExecute);if(delay>0) await new Promise(r=>setTimeout(r,delay));lastExecute=Date.now()}
async function open(id){await goto(`/history/archives/${id}/restore?q=kept&custom=context`);await expect(page.getByRole('heading',{name:'Source and availability',exact:true})).toBeVisible();await page.getByRole('button',{name:'Next',exact:true}).click()}
async function review(id,kind='path',name='custom',key){
 await open(id)
 if(kind==='path'){await page.getByRole('radio',{name:'Custom path',exact:true}).check();await page.getByLabel('Destination path',{exact:true}).fill('/backups/restore186/'+name)}
 if(kind==='volume'){await page.getByRole('radio',{name:'Different volume',exact:true}).check();await page.getByLabel('Destination volume',{exact:true}).selectOption('restore186-alternative')}
 if(key)await page.getByLabel('Private recovery key',{exact:true}).fill(key)
 await page.getByRole('button',{name:'Review effects',exact:true}).click()
 await expect(page.getByRole('heading',{name:'Review restore impact',exact:true})).toBeVisible()
}
async function confirm(status=200, duplicate=false){
 await pace()
 await page.getByLabel(/I reviewed the destination/).check()
 const done=page.waitForResponse(r=>/\/backups\/\d+\/restore$/.test(new URL(r.url()).pathname)&&r.request().method()==='POST')
 if(duplicate) await page.getByRole('button',{name:'Confirm restore',exact:true}).evaluate(button=>{button.click();button.click()})
 else await page.getByRole('button',{name:'Confirm restore',exact:true}).click()
 const r=await done; assert.equal(r.status(),status,await r.text())
 if(status===200)await expect(page.getByRole('heading',{name:'Restore completed',exact:true})).toBeVisible()
 else await expect(page.getByRole('alert')).toBeVisible()
 return r
}
async function backup(target){
 const job=await api('/backups','POST',{target_id:target.id})
 for(let i=0;i<600;i++){const row=await api('/backups/'+job.id);if(row.status==='completed')return row;assert.ok(!['failed','cancelled'].includes(row.status),row.error_message);await new Promise(r=>setTimeout(r,100))}
 throw new Error('Native backup not complete')
}
async function scan(key){
 const safe=await page.evaluate(()=>({url:location.href,local:{...localStorage},session:{...sessionStorage},history:history.state}))
 check(!JSON.stringify(safe).includes(key),'secret absent from URL/storage/history')
}
try{
 await goto('/')
 const password=randomBytes(24).toString('hex')
 await page.getByLabel('Username',{exact:true}).fill('restore-operator')
 await page.getByLabel('Account password',{exact:true}).fill(password)
 await page.getByLabel('Confirm account password',{exact:true}).fill(password)
 await page.getByRole('button',{name:'Create account',exact:true}).click()
 await page.getByRole('button',{name:/Continue without encryption/}).click()
 await page.getByRole('button',{name:'Create first backup plan',exact:true}).click()
 check((await context.cookies()).some(c=>c.name==='session_token'&&c.httpOnly),'native cookie authentication')
 const fixture=await api('/test-restore186/seed','POST')
 const target=await api('/targets','POST',{name:'Restore path',target_type:'path',host_path:fixture.source,compression_enabled:true,stop_container:false,dependencies:fixture.dependencies})
 const plain=await backup(target)
 const volumeTarget=await api('/targets','POST',{name:'Restore volume',target_type:'volume',volume_name:fixture.original_volume,compression_enabled:false,stop_container:false,dependencies:fixture.dependencies})
 const volume=await backup(volumeTarget)
 await api('/test-restore186/change','POST',{action:'mutate'})
 await goto(`/history/archives/${plain.id}?q=kept&custom=context`)
 await expect(page.getByRole('button',{name:'Expand source',exact:true})).toBeVisible()
 await page.getByRole('button',{name:'Expand source',exact:true}).click()
 await expect(page.getByRole('button',{name:'Expand nested',exact:true})).toBeVisible()
 check(true,'real backup -> inspect nested archive via History')
 await page.getByRole('link',{name:'Restore archive',exact:true}).click()
 check(new URL(page.url()).searchParams.get('custom')==='context','route entry preserves unknown query context')
 await page.getByRole('button',{name:'Next',exact:true}).click()
 await page.getByRole('radio',{name:'Custom path',exact:true}).check()
 await page.getByLabel('Destination path',{exact:true}).fill('/backups/restore186/custom')
 await page.getByLabel('Destination path',{exact:true}).press('Enter')
 check(!(await proof()).exists.custom,'Enter before confirmation has no filesystem write')
 await page.getByRole('button',{name:'Review effects',exact:true}).click()
 await expect(page.getByRole('heading',{name:'Review restore impact',exact:true})).toBeFocused()
 check(!(await proof()).exists.custom,'read-only review has no restore write')
 await page.getByRole('button',{name:'Close restore',exact:true}).click()
 check(new URL(page.url()).pathname===`/history/archives/${plain.id}`&&!(await proof()).exists.custom,'close before confirmation preserves context and files')
 await review(plain.id)
 const before=await proof()
 await page.getByLabel(/I reviewed the destination/).check()
 await page.getByRole('button',{name:'Confirm restore',exact:true}).press('Enter')
 check(!(await proof()).exists.custom,'Enter on explicit confirm never submits')
 const final=await confirm(200,true)
 check(executeCount===1,'immediate duplicate confirmation dispatches exactly one native restore')
 check(bytes(await proof(),'custom'),'custom-path completed UI has nested byte-identical files')
 check((await final.json()).destination.path==='/backups/restore186/custom','review/payload/result destination agree')
 const after=await proof()
 check(after.states['restore186-running'].Status==='running'&&after.states['restore186-running'].StartedAt!==before.states['restore186-running'].StartedAt&&after.states['restore186-running'].FinishedAt!==before.states['restore186-running'].FinishedAt,'real isolated Docker stop/restart on originally-running configured dependency')
 assert.deepEqual(after.states['restore186-stopped'],before.states['restore186-stopped']);check(true,'originally-stopped dependency never started')
 await review(plain.id,'original');await confirm();check(bytes(await proof(),'source'),'original path restore bytes')
 await review(volume.id,'original');await confirm();check(bytes(await proof(),'restore186-original'),'original named volume restore bytes')
 await review(volume.id,'volume');const volumeResult=await confirm();const volumeProof=await proof()
 check(bytes(volumeProof,'restore186-alternative')&&(await volumeResult.json()).destination.path===volumeProof.mounts['restore186-alternative'],'alternative volume resolves native server mountpoint and restores bytes')
 // Real encrypted archive produced by existing backup engine.
 const keys=await api('/encryption/setup','POST');const key=keys.private_key
 await api('/encryption/confirm-setup','POST',{confirmed:true})
 const encrypted=await backup(target);check(encrypted.encrypted===true,'native age encrypted engine backup')
 await open(encrypted.id);await page.getByRole('radio',{name:'Custom path',exact:true}).check();await page.getByLabel('Destination path',{exact:true}).fill('/backups/restore186/encrypted')
 check(await page.getByRole('button',{name:'Review effects',exact:true}).isDisabled(),'missing key disables review without writes')
 const wrong=await api('/test-restore186/wrong-key','POST')
 await page.getByLabel('Private recovery key',{exact:true}).fill(wrong.private_key)
 await page.getByRole('button',{name:'Review effects',exact:true}).click()
 const wrongBefore=await proof();await confirm(500)
 assert.deepEqual((await proof()).states,wrongBefore.states)
 check(!(await proof()).exists.encrypted,'wrong key neither stops workloads nor writes destination')
 await expect(page.getByLabel('Private recovery key',{exact:true})).toHaveValue('')
 await review(encrypted.id,'path','encrypted',key);await scan(key);await confirm();check(bytes(await proof(),'encrypted'),'correct age key -> completed UI and restored nested bytes')
 await scan(key)
 // Canonical Local provider sync/materialization, encrypted sidecars included.
 const remote=await api('/storage','POST',{name:'Owned offline-capable local remote',storage_type:'local',base_path:'/backups/restore186/offsite',enabled:true})
 await api('/targets/'+target.id,'PUT',{remote_storage_ids:[remote.id]})
 await api('/backups/'+encrypted.id+'/retry-sync','POST')
 await api('/test-restore186/change','POST',{action:'remote-only',id:encrypted.id})
 await review(encrypted.id,'path','remote',key);await confirm();check(bytes(await proof(),'remote'),'remote-only encrypted materialization/sidecar -> real restored bytes')
 await api('/storage/'+remote.id,'PUT',{enabled:false})
 await review(encrypted.id,'path','remote',key);await confirm(404);check(true,'offline storage produces recoverable unavailable-copy feedback')
 await api('/storage/'+remote.id,'PUT',{enabled:true})
 await api('/test-restore186/change','POST',{action:'remove-sidecar'})
 await review(encrypted.id,'path','remote',key);await confirm(400);check(true,'missing remote key sidecar has scoped retry feedback')
 // Invalid destinations / unsupported originals: native API rejection before effects.
 for(const body of [{destination_type:'path',target_path:'/etc'},{destination_type:'volume',target_volume:'absent'},{destination_type:'original',target_path:'/backups/ignored'}]){
  const r=await response('/backups/'+plain.id+'/restore-review','POST',body);check([400,422].includes(r.status()),'native destination rejection '+JSON.stringify(body))
 }
 for(const kind of ['container','stack']){await api('/test-restore186/change','POST',{action:'unsupported',id:plain.id,kind});await open(plain.id);check(await page.getByRole('radio',{name:'Original location',exact:true}).isDisabled(),'unsupported original '+kind+' explained disabled')}
 await api('/test-restore186/change','POST',{action:'unsupported',id:plain.id,kind:'path'})
 await api('/targets/'+target.id,'PUT',{dependencies:fixture.dependencies})
 await review(plain.id,'path','stale')
 const staleBefore=await proof()
 await api('/targets/'+target.id,'PUT',{dependencies:[]})
 await confirm(409)
 assert.deepEqual((await proof()).states,staleBefore.states)
 check(!(await proof()).exists.stale,'stale lifecycle review rejects execution without stop or destination write')
 await api('/targets/'+target.id,'PUT',{dependencies:fixture.dependencies})
 // An actual restore is held only at its response boundary; no fabricated success.
 await review(plain.id,'path','unknown')
 let release, completed
 const completedPromise=new Promise(r=>{completed=r}), releasePromise=new Promise(r=>{release=r})
 await page.route('**/api/v1/backups/'+plain.id+'/restore',async route=>{const r=await route.fetch();assert.equal(r.status(),200);completed();await releasePromise;await route.abort('failed')})
 await pace();await page.getByLabel(/I reviewed the destination/).check();await page.getByRole('button',{name:'Confirm restore',exact:true}).click()
 await completedPromise
 await expect(page.getByRole('heading',{name:'Restoring — awaiting final backend result',exact:true})).toBeVisible()
 check(bytes(await proof(),'unknown'),'response-loss control: backend really wrote bytes while UI stays pending')
 release();await expect(page.getByRole('alert')).toContainText(/unknown/)
 check(await page.getByRole('heading',{name:'Restore completed',exact:true}).count()===0,'lost final response never reports success')
 await page.unroute('**/api/v1/backups/'+plain.id+'/restore');await page.reload();await expect(page.getByText(/After reload, reconnect/)).toBeVisible()
 check(true,'reload honestly explains unknown result/tracking boundary without replay')
 // Five CSS widths, complete review/Back/keyboard and axe; no device/native AT claim.
 await page.route(base+'/__axe_restore.js',async route=>route.fulfill({status:200,contentType:'application/javascript',body:await readFile('/runner/axe.min.js','utf8')}))
 for(const width of [320,390,768,1024,1440]){
  await page.setViewportSize({width,height:900});await review(plain.id)
  await page.getByRole('button',{name:'Back',exact:true}).click();await expect(page.getByLabel('Destination path',{exact:true})).toHaveValue('/backups/restore186/custom')
  await page.getByRole('button',{name:'Review effects',exact:true}).click()
  await expect(page.getByRole('heading',{name:'Review restore impact',exact:true})).toBeFocused()
  check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`CSS${width} review reflow and keyboard focus`)
  await page.addScriptTag({url:base+'/__axe_restore.js'})
  const violations=await page.evaluate(async()=> (await axe.run(document.querySelector('main')||document.body,{rules:{'color-contrast':{enabled:true}}})).violations)
  assert.deepEqual(violations.map(v=>({id:v.id,nodes:v.nodes.map(n=>n.target)})),[])
  check(true,`CSS${width} axe review`)
  await page.screenshot({path:`/runner/restore-${width}.png`,fullPage:true})
 }
 // Logout during a private key view clears the key, subsequent login cannot revive it.
 await open(encrypted.id);await page.getByLabel('Private recovery key',{exact:true}).fill(key)
 await api('/auth/logout','POST');await page.reload();await expect(page.getByRole('button',{name:'Sign In',exact:true})).toBeVisible()
 await page.getByLabel('Username',{exact:true}).fill('restore-operator');await page.getByLabel('Password',{exact:true}).fill(password);await page.getByRole('button',{name:'Sign In',exact:true}).click()
 await expect(page.getByRole('button',{name:'Log out',exact:true})).toBeVisible()
 await open(encrypted.id);await expect(page.getByLabel('Private recovery key',{exact:true})).toHaveValue('');await scan(key)
 check(true,'session change clears private key and stale restore view')
 await api('/test-restore186/change','POST',{action:'remote-only',id:plain.id})
 await goto(`/history/archives/${plain.id}/restore`)
 await expect(page.getByRole('alert')).toContainText('No usable copy')
 check(await page.getByRole('button',{name:'Next',exact:true}).isDisabled(),'missing local archive without remote recovery never advances')
 await goto('/history/archives/999999/restore?q=preserved')
 await expect(page.getByRole('alert')).toContainText('Archive not found')
 check(new URL(page.url()).searchParams.get('q')==='preserved','missing archive route retains return context')
 await api('/test-restore186/change','POST',{action:'cleanup'})
 await writeFile('/runner/restore186-result.json',JSON.stringify({checks,limits:['Native isolated Docker/Local provider only, not external providers/Komodo','Named transport-response fault is not cancellation','CSS/axe/keyboard, not physical devices/native AT']},null,2))
 console.log('PASS restore186 checks='+checks.length)
}catch(e){await page.screenshot({path:'/runner/restore-failure.png',fullPage:true});throw e}
finally{await browser.close()}
