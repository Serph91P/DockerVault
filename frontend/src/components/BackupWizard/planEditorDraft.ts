import { initialData, type WizardData } from './planEditorModel'

const PREFIX = 'dockervault-plan-draft:'
const TTL = 24 * 60 * 60 * 1000
export const draftKey = (owner: number, intent: string) => `${PREFIX}${owner}:${intent}`
export interface PlanDraft {
  version: 1
  owner: number
  intent: string
  revision: string | null
  savedAt: number
  group: number
  data: WizardData
}

// Both levels are allowlisted: copying a typed object alone does not remove runtime extras.
export function safeIntent(data: WizardData): WizardData {
  const safe = Object.fromEntries(
    Object.keys(initialData).map((key) => [key, data[key as keyof WizardData] ?? initialData[key as keyof WizardData]])
  ) as unknown as WizardData
  const stringList = (value: unknown) => Array.isArray(value) && value.every((item) => typeof item === 'string')
  const validId = (value: unknown) => value === null || (Number.isSafeInteger(value) && Number(value) > 0)
  const record = (value: unknown) => value !== null && typeof value === 'object' && !Array.isArray(value)
  const invalid =
    ['targetName', 'containerName', 'volumeName', 'hostPath', 'stackName'].some(
      (key) => typeof safe[key as keyof WizardData] !== 'string'
    ) ||
    ['enabled', 'stopContainer', 'deleteLocalAfterSync'].some(
      (key) => typeof safe[key as keyof WizardData] !== 'boolean'
    ) ||
    ['selectedVolumes', 'includePaths', 'excludePaths', 'dependencies'].some(
      (key) => !stringList(safe[key as keyof WizardData])
    ) ||
    !Array.isArray(safe.remoteStorageIds) ||
    safe.remoteStorageIds.some((id) => id === null || !validId(id)) ||
    ![null, 'container', 'stack', 'volume', 'path'].includes(safe.targetType) ||
    !['all', 'subset'].includes(safe.volumeMode) ||
    !['manual', 'legacy', 'shared'].includes(safe.scheduleMode) ||
    !['none', 'gzip'].includes(safe.compression) ||
    ![null, 'schedule', 'retention', 'target'].includes(safe.unconfirmed) ||
    ![safe.scheduleId, safe.retentionPolicyId, safe.confirmedTargetId].every(validId) ||
    !(safe.scheduleCron === null || typeof safe.scheduleCron === 'string') ||
    !record(safe.perVolumeRules)
  if (invalid) throw new Error('Invalid safe draft')
  for (const rule of Object.values(safe.perVolumeRules))
    if (
      !record(rule) ||
      ['includePaths', 'excludePaths'].some(
        (key) => rule[key as keyof typeof rule] !== undefined && !stringList(rule[key as keyof typeof rule])
      )
    )
      throw new Error('Invalid draft rule')
  if (
    safe.newSchedule &&
    (!record(safe.newSchedule) ||
      ['name', 'cronExpression', 'description'].some(
        (key) => typeof safe.newSchedule![key as keyof typeof safe.newSchedule] !== 'string'
      ) ||
      !(safe.newSchedule.enabled === undefined || typeof safe.newSchedule.enabled === 'boolean'))
  )
    throw new Error('Invalid schedule draft')
  if (
    safe.newRetentionPolicy &&
    (!record(safe.newRetentionPolicy) ||
      typeof safe.newRetentionPolicy.name !== 'string' ||
      ['keepLast', 'keepDaily', 'keepWeekly', 'keepMonthly', 'keepYearly', 'maxAgeDays'].some(
        (key) => {
          const value = safe.newRetentionPolicy![key as keyof typeof safe.newRetentionPolicy]
          return value !== '' && !Number.isFinite(value)
        }
      ))
  )
    throw new Error('Invalid retention draft')
  safe.perVolumeRules = Object.fromEntries(
    Object.entries(safe.perVolumeRules).map(([key, rule]) => [
      key,
      {
        ...(Array.isArray(rule.includePaths)
          ? { includePaths: rule.includePaths.filter((p) => typeof p === 'string') }
          : {}),
        ...(Array.isArray(rule.excludePaths)
          ? { excludePaths: rule.excludePaths.filter((p) => typeof p === 'string') }
          : {}),
      },
    ])
  )
  if (safe.newSchedule) {
    const s = safe.newSchedule
    safe.newSchedule = {
      name: s.name,
      cronExpression: s.cronExpression,
      description: s.description,
      enabled: s.enabled,
    }
  }
  if (safe.newRetentionPolicy) {
    const r = safe.newRetentionPolicy
    safe.newRetentionPolicy = {
      name: r.name,
      keepLast: r.keepLast,
      keepDaily: r.keepDaily,
      keepWeekly: r.keepWeekly,
      keepMonthly: r.keepMonthly,
      keepYearly: r.keepYearly,
      maxAgeDays: r.maxAgeDays,
    }
  }
  return structuredClone(safe)
}
export function readDraft(owner: number, intent: string): PlanDraft | null {
  const key = draftKey(owner, intent)
  try {
    const legacy = localStorage.getItem('dockervault-wizard-draft')
    if (legacy) {
      localStorage.removeItem('dockervault-wizard-draft')
      try {
        const old = JSON.parse(legacy)
        if (
          intent === 'new' &&
          old.ownerId === owner &&
          old.data &&
          Number.isFinite(old.savedAt) &&
          Date.now() - old.savedAt < TTL &&
          !localStorage.getItem(key)
        ) {
          localStorage.setItem(
            key,
            JSON.stringify({
              version: 1,
              owner,
              intent,
              revision: null,
              savedAt: old.savedAt,
              group: old.currentStep >= 7 ? 2 : old.currentStep >= 4 ? 1 : 0,
              data: safeIntent({ ...initialData, ...old.data, volumeMode: old.data.volumeMode ?? (old.data.selectedVolumes?.length ? 'subset' : 'all') }),
            })
          )
        }
      } catch {
        /* Unowned/malformed legacy drafts cannot be adopted safely. */
      }
    }
    const draft = JSON.parse(localStorage.getItem(key) || 'null') as PlanDraft | null
    if (!draft) return null
    if (
      draft.version !== 1 ||
      draft.owner !== owner ||
      draft.intent !== intent ||
      !Number.isFinite(draft.savedAt) ||
      Date.now() - draft.savedAt >= TTL ||
      draft.savedAt > Date.now() ||
      !draft.data ||
      !Number.isInteger(draft.group) ||
      draft.group < 0 ||
      draft.group > 2
    ) {
      localStorage.removeItem(key)
      return null
    }
    draft.data = safeIntent(draft.data)
    localStorage.setItem(key, JSON.stringify(draft))
    return draft
  } catch {
    localStorage.removeItem(key)
    return null
  }
}
export function clearPlanDrafts(preserveOwner?: number) {
  for (const key of Object.keys(localStorage))
    if (key.startsWith(PREFIX) && (!preserveOwner || !key.startsWith(`${PREFIX}${preserveOwner}:`)))
      localStorage.removeItem(key)
}
export class DraftWriter {
  private timer: ReturnType<typeof setTimeout> | undefined
  private generation = 0
  constructor(
    private owner: number,
    private intent: string,
    private revision: string | null
  ) {}
  cancel() {
    ++this.generation
    clearTimeout(this.timer)
  }
  queue(data: WizardData, group: number) {
    this.cancel()
    const generation = this.generation
    const safe = safeIntent(data)
    this.timer = setTimeout(() => {
      if (generation !== this.generation) return
      try {
        localStorage.setItem(
          draftKey(this.owner, this.intent),
          JSON.stringify({
            version: 1,
            owner: this.owner,
            intent: this.intent,
            revision: this.revision,
            savedAt: Date.now(),
            group,
            data: safe,
          } satisfies PlanDraft)
        )
      } catch {
        /* Unavailable storage never blocks editing. */
      }
    }, 300)
  }
  checkpoint(data: WizardData, group: number, revision = this.revision) {
    this.cancel()
    try {
      localStorage.setItem(
        draftKey(this.owner, this.intent),
        JSON.stringify({
          version: 1,
          owner: this.owner,
          intent: this.intent,
          revision,
          savedAt: Date.now(),
          group,
          data: safeIntent(data),
        } satisfies PlanDraft)
      )
    } catch {
      /* Storage unavailable. */
    }
  }
  clear() {
    this.cancel()
    try {
      localStorage.removeItem(draftKey(this.owner, this.intent))
    } catch {
      /* Storage unavailable. */
    }
  }
}
