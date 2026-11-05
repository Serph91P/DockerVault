import { useState } from 'react'
import { Plus, Trash2 } from 'lucide-react'
import type { Container, Stack } from '../../api'
import { availableVolumes, volumeRuleKey, type WizardData, type VolumeRule } from './planEditorModel'

export function PathEditor({
  label,
  paths,
  onChange,
}: {
  label: string
  paths: string[]
  onChange: (paths: string[]) => void
}) {
  const [text, setText] = useState('')
  const add = () => {
    if (text.trim() && !paths.includes(text.trim())) {
      onChange([...paths, text.trim()])
      setText('')
    }
  }
  return (
    <div className="space-y-2">
      <label className="block text-sm text-dark-200">
        {label}
        <input
          className="input w-full mt-1"
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault()
              add()
            }
          }}
          placeholder="Relative path or pattern"
        />
      </label>
      <button type="button" className="btn-secondary" aria-label={`Add ${label}`} disabled={!text.trim()} onClick={add}>
        <Plus className="size-4" /> Add path
      </button>
      <ul className="space-y-1">
        {paths.map((path) => (
          <li className="flex items-center gap-2 break-all text-sm" key={path}>
            {path}
            <button
              type="button"
              className="btn-secondary"
              aria-label={`Remove ${label}: ${path}`}
              onClick={() => onChange(paths.filter((p) => p !== path))}
            >
              <Trash2 className="size-4" />
            </button>
          </li>
        ))}
      </ul>
    </div>
  )
}
export default function StepVolumeConfig({
  data,
  updateData,
  containers,
  stacks,
}: {
  data: WizardData
  updateData: (updates: Partial<WizardData>) => void
  containers: Container[]
  stacks: Stack[]
}) {
  const volumes = availableVolumes(data, containers, stacks)
  const multi = data.targetType === 'container' || data.targetType === 'stack'
  const active = data.volumeMode === 'all' ? volumes : data.selectedVolumes
  const setRule = (key: string, rule?: VolumeRule) => {
    const rules = { ...data.perVolumeRules }
    if (rule === undefined) delete rules[key]
    else rules[key] = rule
    updateData({ perVolumeRules: rules })
  }
  return (
    <section className="space-y-5" aria-label="Volume and path configuration">
      <h3 className="text-lg font-semibold">Volume & Path Configuration</h3>
      {multi && (
        <>
          <p>
            Only named Docker volumes are protected, not bind mounts or the container filesystem. All volumes follows
            future topology; a subset never silently widens.
          </p>
          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              className="btn-secondary"
              aria-pressed={data.volumeMode === 'all'}
              onClick={() => updateData({ volumeMode: 'all', selectedVolumes: [] })}
            >
              Backup all volumes
            </button>
            <button
              type="button"
              className="btn-secondary"
              aria-pressed={data.volumeMode === 'subset'}
              onClick={() =>
                updateData({
                  volumeMode: 'subset',
                  selectedVolumes: data.volumeMode === 'all' ? volumes : data.selectedVolumes,
                })
              }
            >
              Select specific volumes
            </button>
          </div>
          {data.volumeMode === 'subset' && (
            <fieldset className="space-y-2">
              <legend>Selected volumes</legend>
              {[...new Set([...volumes, ...data.selectedVolumes])].map((name) => (
                <label className="flex items-center gap-2 break-all" key={name}>
                  <input
                    type="checkbox"
                    checked={data.selectedVolumes.includes(name)}
                    onChange={() =>
                      updateData({
                        selectedVolumes: data.selectedVolumes.includes(name)
                          ? data.selectedVolumes.filter((v) => v !== name)
                          : [...data.selectedVolumes, name],
                      })
                    }
                  />
                  {name}
                  {!volumes.includes(name) && ' — missing, selection preserved'}
                </label>
              ))}
              {!data.selectedVolumes.length && (
                <p role="alert">Select at least one volume or explicitly choose all volumes.</p>
              )}
            </fieldset>
          )}
          {!volumes.length && (
            <p role="alert">No named volumes confirmed for this source. Recheck discovery before saving.</p>
          )}
        </>
      )}
      <h4 className="font-semibold">Global path filters</h4>
      <p className="text-sm text-dark-300">
        Relative to each archive source. Exclusions win. Empty includes means all paths; empty excludes means no
        exclusions.
      </p>
      <PathEditor
        label="Include Paths"
        paths={data.includePaths}
        onChange={(includePaths) => updateData({ includePaths })}
      />
      <PathEditor
        label="Exclude Paths"
        paths={data.excludePaths}
        onChange={(excludePaths) => updateData({ excludePaths })}
      />
      {multi && (
        <>
          <h4 className="font-semibold">Per-volume overrides</h4>
          <p className="text-sm text-dark-300">
            An override replaces both global lists. Explicitly empty lists include everything; inherit uses the global
            filters. Stable Docker volume identities resolve to every archive mount at execution.
          </p>
          {volumes.map((name) => {
            const key = volumeRuleKey(name),
              rule = data.perVolumeRules[key]
            return (
              <details key={name} className="border-t border-dark-600 py-3">
                <summary className="cursor-pointer break-all">
                  {name} — {rule && Object.keys(rule).length ? 'override' : 'inherits global filters'}
                  {!active.includes(name) && ' (not selected; dormant)'}
                </summary>
                <div className="space-y-3 pt-3">
                  <button
                    type="button"
                    className="btn-secondary"
                    onClick={() => setRule(key, { includePaths: [], excludePaths: [] })}
                  >
                    Override with all paths for {name}
                  </button>
                  <button type="button" className="btn-secondary" onClick={() => setRule(key)}>
                    Inherit global filters for {name}
                  </button>
                  {rule && (
                    <>
                      <PathEditor
                        label={`${name} include paths`}
                        paths={rule.includePaths || []}
                        onChange={(includePaths) =>
                          setRule(key, { includePaths, excludePaths: rule.excludePaths || [] })
                        }
                      />
                      <PathEditor
                        label={`${name} exclude paths`}
                        paths={rule.excludePaths || []}
                        onChange={(excludePaths) =>
                          setRule(key, { includePaths: rule.includePaths || [], excludePaths })
                        }
                      />
                    </>
                  )}
                </div>
              </details>
            )
          })}
        </>
      )}
      {Object.entries(data.perVolumeRules)
        .filter(([key]) => !key.startsWith('volume:') || !volumes.includes(key.slice(7)))
        .map(([key, rule]) => (
          <div key={key} className="space-y-2 border-t border-dark-600 py-3">
            <p className="break-all">
              {key.startsWith('volume:') ? 'Missing volume rule' : 'Legacy archive-root rule (mapping is not assumed)'}:{' '}
              {key}
            </p>
            <pre className="whitespace-pre-wrap break-all text-sm">{JSON.stringify(rule)}</pre>
            <button type="button" className="btn-secondary" onClick={() => setRule(key)}>
              Remove rule {key}
            </button>
            {multi && (
              <label className="block">
                Explicitly rebind {key}
                <select
                  className="input"
                  value=""
                  onChange={(e) => {
                    if (!e.target.value || data.perVolumeRules[volumeRuleKey(e.target.value)]) return
                    const rules = { ...data.perVolumeRules }
                    delete rules[key]
                    rules[volumeRuleKey(e.target.value)] = rule
                    updateData({ perVolumeRules: rules })
                  }}
                >
                  <option value="">Choose a volume</option>
                  {volumes
                    .filter((name) => !data.perVolumeRules[volumeRuleKey(name)])
                    .map((name) => (
                      <option key={name}>{name}</option>
                    ))}
                </select>
              </label>
            )}
          </div>
        ))}
    </section>
  )
}
