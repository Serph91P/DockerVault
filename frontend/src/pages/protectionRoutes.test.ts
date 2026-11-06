import { expect, it } from 'vitest'
import { protectionSearch, returnPath, planId } from './protectionRoutes'
it('allowlists local return filters and rejects external/navigation injection', () => {
  expect(returnPath('https://evil.test/')).toBe('/protection')
  expect(returnPath('//evil.test/')).toBe('/protection')
  expect(returnPath('/protection/9?secret=x')).toBe('/protection')
  expect(returnPath('/protection?q=needle&type=path&status=active&sort=created&key=private')).toBe('/protection?q=needle&type=path&status=active&sort=created')
  expect(protectionSearch(new URLSearchParams('type=wrong&sort=wrong&plan=9&token=private'))).toBe('')
})
it('strictly validates safe positive plan identities', () => {
  for (const raw of ['0','-1','1x','01','9007199254740992','new']) expect(planId(raw)).toBeNull()
  expect(planId('917')).toBe(917)
})
it('preserves allowlisted policy return context without allowing arbitrary navigation', () => {
  expect(returnPath('/protection/retention/7?q=default&usage=used&secret=x')).toBe('/protection/retention/7?q=default&usage=used')
  expect(returnPath('/protection/schedules/8/edit?status=disabled')).toBe('/protection/schedules/8/edit?status=disabled')
  for (const raw of ['/protection/schedules/0', '/protection/schedules//evil', '/protection/retention/7/../../settings', '/protection/schedules/new', '/protection/schedules/9007199254740992']) expect(returnPath(raw)).toBe('/protection')
})
