import { expect, it } from 'vitest'
import { validateRead, validateWrite } from './responses'

const types = ['local','ssh','sftp','webdav','s3','ftp','rclone']
const row = (storage_type: unknown) => ({ id: 1, name: 'disabled synthetic', storage_type, enabled: false, base_path: '/fixture' })
it.each(types)('accepts real backend %s GET/POST/PUT responses', type => {
  expect(() => validateRead('/storage', [row(type)])).not.toThrow()
  expect(() => validateWrite('/storage', row(type))).not.toThrow()
  expect(() => validateWrite('/storage/1', row(type))).not.toThrow()
})
it('accepts a mixed list including SFTP but rejects unknown or malformed rows', () => {
  expect(() => validateRead('/storage', types.map(row))).not.toThrow()
  for (const value of ['unknown', '', null, {}, 7]) {
    expect(() => validateRead('/storage', [row('local'), row(value)])).toThrow()
    expect(() => validateWrite('/storage', row(value))).toThrow()
    expect(() => validateWrite('/storage/1', row(value))).toThrow()
  }
  expect(() => validateRead('/storage', [{...row('sftp'), enabled: 'false'}])).toThrow()
})
