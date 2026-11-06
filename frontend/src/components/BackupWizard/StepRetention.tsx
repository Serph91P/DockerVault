import type { RetentionPolicy } from '../../api'
import { retentionDefaults, type WizardData } from './planEditorModel'
import { RetentionFields } from './PolicyFields'
import type { useValidationFeedback } from '../ValidationFeedback'

export default function StepRetention({
  data,
  updateData,
  policies,
  isLoadingPolicies,
  feedback,
}: {
  data: WizardData
  updateData: (updates: Partial<WizardData>) => void
  policies: RetentionPolicy[]
  isLoadingPolicies: boolean
  feedback?: ReturnType<typeof useValidationFeedback>['field']
}) {
  const selected = policies.find((p) => p.id === data.retentionPolicyId)
  const effective = selected || (!data.retentionPolicyId ? policies.find((p) => p.name === 'default') : undefined)
  return (
    <section className="space-y-3" aria-label="Retention policy">
      <h3 className="text-lg font-semibold">Retention Policy</h3>
      {isLoadingPolicies && <p role="status">Loading policies...</p>}
      <label className="block">
        Retention Policy
        <select
          className="input w-full"
          value={data.newRetentionPolicy ? 'new' : data.retentionPolicyId || ''}
          onChange={(e) =>
            updateData({
              retentionPolicyId: e.target.value && e.target.value !== 'new' ? Number(e.target.value) : null,
              newRetentionPolicy: e.target.value === 'new' ? { ...retentionDefaults } : null,
            })
          }
        >
          <option value="">Use standard fallback (clear explicit policy)</option>
          {data.retentionPolicyId && !selected && (
            <option value={data.retentionPolicyId}>Missing policy #{data.retentionPolicyId} — recheck</option>
          )}
          {policies.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}
            </option>
          ))}
          <option value="new">Create new policy</option>
        </select>
      </label>
      {data.newRetentionPolicy ? (
        <>
          <div className="flex flex-wrap gap-2">
            {[
              { name: 'Keep Last 7', keepLast: 7, keepDaily: 0, keepWeekly: 0, keepMonthly: 0 },
              { name: 'Daily for a week', keepLast: 3, keepDaily: 7, keepWeekly: 0, keepMonthly: 0 },
              { name: '7 daily + 4 weekly', keepLast: 3, keepDaily: 7, keepWeekly: 4, keepMonthly: 0 },
              { name: 'Full rotation', keepLast: 7, keepDaily: 7, keepWeekly: 4, keepMonthly: 6 },
            ].map((p) => (
              <button
                type="button"
                className="btn-secondary"
                key={p.name}
                onClick={() => updateData({ newRetentionPolicy: { ...data.newRetentionPolicy!, ...p } })}
              >
                {p.name}
              </button>
            ))}
          </div>
          <RetentionFields
            feedback={feedback}
            value={data.newRetentionPolicy}
            onChange={(newRetentionPolicy) => updateData({ newRetentionPolicy })}
          />
        </>
      ) : (
        <>
          {!data.retentionPolicyId && (
            <p>
              No explicit policy: the backend uses the policy named default when available. This does not turn retention
              off.
            </p>
          )}
          {effective && (
            <p className="text-sm">
              {effective.name}: last {effective.keep_last}; daily {effective.keep_daily}; weekly {effective.keep_weekly}
              ; monthly {effective.keep_monthly}; yearly {effective.keep_yearly}; max age {effective.max_age_days} days.
              Buckets overlap; this is not a deletion preview.
            </p>
          )}
        </>
      )}
    </section>
  )
}
