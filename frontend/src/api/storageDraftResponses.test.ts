import { describe, expect, it } from 'vitest'
import { validateWrite } from './responses'

describe('draft test response boundary', () => {
  it.each([{success:true,message:'Observed'},{success:false,message:'Failed'}])('accepts only a boolean observation with text', value => expect(() => validateWrite('/storage/test-config',value)).not.toThrow())
  it.each([{}, {success:'true',message:'private'}, {success:true}, {success:null,message:'unknown'}])('rejects malformed observations', value => expect(() => validateWrite('/storage/test-config', value)).toThrow())
})
