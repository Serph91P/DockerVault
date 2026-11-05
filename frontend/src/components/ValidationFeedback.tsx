import { useId } from 'react'
import { ResponseError } from '../api/responses'

export function useValidationFeedback(error: unknown) {
  const id = useId()
  const fields = error instanceof ResponseError ? error.fields : {}
  return {
    field: (name: string) => fields[name] ? { 'aria-invalid': true as const, 'aria-describedby': `${id}-${name}` } : {},
    summary: error ? <section role="alert" className="rounded-lg border border-red-500/50 p-3 text-red-200"><h3>Request failed. Review the highlighted fields or retry.</h3>{Object.entries(fields).length ? <ul>{Object.entries(fields).map(([name, message]) => <li id={`${id}-${name}`} key={name}>{name.replace(/_/g, ' ')}: {message}</li>)}</ul> : <p>No final result was confirmed. Your input is retained.</p>}</section> : null,
  }
}
