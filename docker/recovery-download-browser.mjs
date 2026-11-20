import assert from 'node:assert/strict'
import { createHash, randomBytes } from 'node:crypto'
import { readFile } from 'node:fs/promises'
import { chromium } from 'playwright'
import { expect } from 'playwright/test'

const [, revision, mode, arch] = process.argv.slice(2)
const base = 'http://127.0.0.1'
const browser = await chromium.launch()
try {
  const page = await browser.newPage({ acceptDownloads: true, reducedMotion: 'reduce' })
  const button = (name, scope = page) => scope.getByRole('button', { name, exact: true })
  const download = async (link) => {
    const [result] = await Promise.all([page.waitForEvent('download'), link.click()])
    return [result.suggestedFilename(), await readFile(await result.path())]
  }
  await page.goto(base)
  await button('Create new installation').click()
  const password = randomBytes(24).toString('hex')
  await page.getByLabel('Username', { exact: true }).fill('recovery-download-fixture')
  await page.getByLabel('Account password', { exact: true }).fill(password)
  await page.getByLabel('Confirm account password', { exact: true }).fill(password)
  await button('Create account').click()
  await page.getByRole('button', { name: /Continue without encryption/ }).click()
  await button('Continue to DockerVault').click()
  await page.goto(base + '/settings#encryption')
  await button('Set Up Encryption').click()
  const custody = page.getByRole('dialog', { name: 'Save Your Private Key' })
  await expect(button('I have saved my private key', custody)).toBeDisabled()
  await Promise.all([page.waitForEvent('download'), button('Download Private Key File', custody).click()])
  await button('I have saved my private key', custody).click()
  await expect(custody).toBeHidden()
  await button('View Recovery Instructions').click()
  const dialog = page.getByRole('dialog', { name: 'Backup Recovery Instructions' })
  if (mode === 'native') {
    await expect(dialog).toContainText('./dockervault-recover --identity')
    await expect(dialog).not.toContainText('python3 recover_backup.py')
    const [manifestName, raw] = await download(dialog.getByRole('link', { name: 'recovery manifest', exact: true }))
    assert.equal(manifestName, 'manifest.json')
    const manifest = JSON.parse(raw.toString())
    const [binaryName, bytes] = await download(dialog.getByRole('link', { name: 'dockervault-recover', exact: true }))
    assert.equal(binaryName, 'dockervault-recover')
    assert.deepEqual(manifest, {os:'linux', arch, revision, file:binaryName, sha256:createHash('sha256').update(bytes).digest('hex')})
    assert.equal(bytes.subarray(0, 4).toString('hex'), '7f454c46')
  } else {
    assert.equal(mode, 'python')
    await expect(dialog).toContainText('python3 recover_backup.py')
    const [name, bytes] = await download(dialog.getByRole('link', { name: 'recover_backup.py', exact: true }))
    assert.equal(name, 'recover_backup.py')
    assert.ok(bytes.includes(Buffer.from('def main():')))
    assert.ok(bytes.includes(Buffer.from('if __name__ == "__main__":')))
    await expect(dialog.getByRole('link', { name: 'recovery manifest', exact: true })).toHaveCount(0)
  }
  console.log(`Recovery instructions and browser downloads passed: ${mode} ${arch} ${revision}`)
} finally {
  await browser.close()
}
