import { expect, it } from 'vitest'
import { safeRequestError, ResponseError } from './responses'

const marker = 'PRIVATE_VALIDATION_MARKER'
it('maps the real global 422 errors envelope to static allowlisted field messages only', () => {
  const result = safeRequestError({ response: { status: 422, data: {
    detail: 'Validation error', errors: [
      { field: 'body.keep_last', message: marker, msg: marker, input: marker, ctx: { secret: marker } },
      { field: 'body.pre_backup_command', message: marker },
      { field: 'body.cron_expression', message: marker },
    ],
  } } })
  expect(result).toBeInstanceOf(ResponseError)
  expect(result.fields).toEqual({ keep_last: 'Check this value.', pre_backup_command: 'Check this value.', cron_expression: 'Check this value.' })
  expect(JSON.stringify(result)).not.toContain(marker)
  expect(result.message).not.toContain(marker)
  expect('response' in result).toBe(false)
  expect('config' in result).toBe(false)
})
it.each([
  null, 1, [], marker, {}, { field: null }, { field: ['body.keep_last'] }, { field: { toString: () => 'body.keep_last' } },
  { field: 'keep_last' }, { field: 'query.keep_last' }, { field: 'body.keep_last.extra' }, { field: 'body.remote_storage_ids.0' },
  { field: 'body.__proto__' }, { field: 'body.constructor' }, { field: 'body.toString' }, { field: 'body.' + marker },
  Object.create({ field: 'body.keep_last' }),
])('ignores unsafe/malformed canonical fields without copying raw errors (%j)', item => {
  const result = safeRequestError({ response: { status: 422, data: { errors: [item] } } })
  expect(result.fields).toEqual({})
  expect(JSON.stringify(result)).not.toContain(marker)
})
it.each([null, {}, marker, ['body.keep_last']])('rejects malformed errors containers (%j)', errors => {
  expect(safeRequestError({ response: { status: 422, data: { errors } } }).fields).toEqual({})
})
it('retains compatible detail-array validation and static typed messages without unsafe coercion', () => {
  const result = safeRequestError({ response: { status: 422, data: { detail: [
    { loc: ['body', 'keep_last'], type: 'greater_than_equal', msg: marker, input: marker },
    { loc: ['body', ['name']], type: 'missing' },
    { loc: ['body', 'name'], type: ['missing'], msg: marker },
    { loc: ['body', '__proto__'], type: 'missing' }, null,
  ] } } })
  expect(result.fields).toEqual({ keep_last: 'Value is outside the allowed range.', name: 'Check this value.' })
  expect(JSON.stringify(result)).not.toContain(marker)
})
it('does not consume validation-looking data on another status', () => {
  expect(safeRequestError({ response: { status: 500, data: { errors: [{ field: 'body.name' }] } } }).fields).toEqual({})
})
