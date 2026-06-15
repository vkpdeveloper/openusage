import { ProviderCard } from "@/components/provider-card"
import { Button } from "@/components/ui/button"
import type { MetricLine, PluginDisplayState } from "@/lib/plugin-types"
import type { DisplayMode, ResetTimerDisplayMode, TimeFormatMode } from "@/lib/settings"
import { invoke } from "@tauri-apps/api/core"
import { useCallback, useEffect, useState } from "react"

type ProviderAccount = {
  id: string
  providerId: string
  name: string
}

type CurrentLoginStatus = {
  available: boolean
  isNew: boolean
}

type CodexOAuthSession = {
  id: string
  status: "waiting" | "complete" | "error" | "expired"
  authorizationUrl?: string
  error?: string
}

const COMBINED_USAGE_START_LABELS = new Set(["Today", "Yesterday", "Last 30 Days", "Usage Trend"])

function splitCombinedUsageLines(lines: MetricLine[]) {
  const start = lines.findIndex((line) => COMBINED_USAGE_START_LABELS.has(line.label))
  if (start === -1) return { accountLines: lines, usageLines: [] }
  return {
    accountLines: lines.slice(0, start),
    usageLines: lines.slice(start),
  }
}

interface ProviderDetailPageProps {
  plugin: PluginDisplayState | null
  onRetry?: () => void
  displayMode: DisplayMode
  resetTimerDisplayMode: ResetTimerDisplayMode
  timeFormatMode?: TimeFormatMode
  onResetTimerDisplayModeToggle?: () => void
}

export function ProviderDetailPage({
  plugin,
  onRetry,
  displayMode,
  resetTimerDisplayMode,
  timeFormatMode = "auto",
  onResetTimerDisplayModeToggle,
}: ProviderDetailPageProps) {
  if (!plugin) {
    return (
      <div className="text-center text-muted-foreground py-8">
        Provider not found
      </div>
    )
  }

  return (
    <ProviderAccountDetail
      plugin={plugin}
      onRetry={onRetry}
      displayMode={displayMode}
      resetTimerDisplayMode={resetTimerDisplayMode}
      timeFormatMode={timeFormatMode}
      onResetTimerDisplayModeToggle={onResetTimerDisplayModeToggle}
    />
  )
}

function ProviderAccountDetail({
  plugin,
  onRetry,
  displayMode,
  resetTimerDisplayMode,
  timeFormatMode = "auto",
  onResetTimerDisplayModeToggle,
}: ProviderDetailPageProps & { plugin: PluginDisplayState }) {
  const supportsAccounts = plugin.meta.id === "claude" || plugin.meta.id === "codex"
  const [accounts, setAccounts] = useState<ProviderAccount[]>([])
  const [accountsLoaded, setAccountsLoaded] = useState(false)
  const [profileName, setProfileName] = useState("")
  const [saving, setSaving] = useState(false)
  const [accountError, setAccountError] = useState<string | null>(null)
  const [currentLogin, setCurrentLogin] = useState<CurrentLoginStatus | null>(null)
  const [pendingRemoval, setPendingRemoval] = useState<string | null>(null)
  const [codexOAuth, setCodexOAuth] = useState<CodexOAuthSession | null>(null)
  const [startingOAuth, setStartingOAuth] = useState(false)
  const [linkCopied, setLinkCopied] = useState(false)
  const [accountsExpanded, setAccountsExpanded] = useState(false)

  const loadAccounts = useCallback(async () => {
    if (!supportsAccounts) return
    try {
      const [all, status] = await Promise.all([
        invoke<ProviderAccount[]>("list_provider_accounts"),
        invoke<CurrentLoginStatus>("get_current_provider_login_status", {
          providerId: plugin.meta.id,
        }),
      ])
      setAccounts(all.filter((account) => account.providerId === plugin.meta.id))
      setCurrentLogin(status)
    } catch (error) {
      setAccountError(String(error))
    } finally {
      setAccountsLoaded(true)
    }
  }, [plugin.meta.id, supportsAccounts])

  useEffect(() => {
    void loadAccounts()
    if (!supportsAccounts) return
    const timer = window.setInterval(() => void loadAccounts(), 5000)
    return () => window.clearInterval(timer)
  }, [loadAccounts, supportsAccounts])

  const saveCurrentLogin = async () => {
    setSaving(true)
    setAccountError(null)
    try {
      await invoke("import_current_provider_account", {
        providerId: plugin.meta.id,
        name: profileName,
      })
      setProfileName("")
      await loadAccounts()
      await invoke("start_probe_batch", { pluginIds: [plugin.meta.id] })
    } catch (error) {
      setAccountError(String(error))
    } finally {
      setSaving(false)
    }
  }

  const removeAccount = async (accountId: string) => {
    if (pendingRemoval !== accountId) {
      setPendingRemoval(accountId)
      return
    }
    setAccountError(null)
    try {
      await invoke("remove_provider_account", { accountId })
      setPendingRemoval(null)
      await loadAccounts()
      await invoke("start_probe_batch", { pluginIds: [plugin.meta.id] })
    } catch (error) {
      setAccountError(String(error))
    }
  }

  const startCodexOAuth = async () => {
    setStartingOAuth(true)
    setAccountError(null)
    try {
      const session = await invoke<CodexOAuthSession>("start_codex_oauth", {
        name: profileName,
      })
      setCodexOAuth(session)
    } catch (error) {
      setAccountError(String(error))
    } finally {
      setStartingOAuth(false)
    }
  }

  useEffect(() => {
    if (!codexOAuth || codexOAuth.status !== "waiting") return
    const timer = window.setInterval(async () => {
      try {
        const session = await invoke<CodexOAuthSession>("get_codex_oauth_status", {
          sessionId: codexOAuth.id,
        })
        setCodexOAuth(session)
        if (session.status === "complete") {
          setProfileName("")
          await loadAccounts()
          await invoke("start_probe_batch", { pluginIds: [plugin.meta.id] })
        } else if (session.status === "expired") {
          setCodexOAuth(null)
          setLinkCopied(false)
        } else if (session.status === "error") {
          setAccountError(session.error || "Codex device authentication failed.")
          setCodexOAuth(null)
        }
      } catch (error) {
        setAccountError(String(error))
      }
    }, 1000)
    return () => window.clearInterval(timer)
  }, [codexOAuth, loadAccounts, plugin.meta.id])

  const renameAccount = async (accountId: string, name: string) => {
    setAccountError(null)
    try {
      await invoke("rename_provider_account", { accountId, name })
      await loadAccounts()
    } catch (error) {
      setAccountError(String(error))
      await loadAccounts()
    }
  }

  const savedAccountIds = new Set(accounts.map((account) => account.id))
  const visibleSavedStates = (plugin.accounts ?? []).filter((state) => {
    const accountId = state.data?.accountId
    return accountId && savedAccountIds.has(accountId)
  }).sort((left, right) =>
    (left.data?.accountOrder ?? Number.MAX_SAFE_INTEGER) -
    (right.data?.accountOrder ?? Number.MAX_SAFE_INTEGER)
  )
  const accountStates = supportsAccounts && accountsLoaded
    ? accounts.length > 0
      ? visibleSavedStates
      : plugin.data?.accountId
        ? []
        : [plugin]
    : [plugin]
  const combineAccountUsage = supportsAccounts && accountStates.length > 1
  const combinedUsageState = combineAccountUsage
    ? accountStates
      .map((state) => ({
        state,
        lines: splitCombinedUsageLines(state.data?.lines ?? []).usageLines,
      }))
      .sort((left, right) => right.lines.length - left.lines.length)[0]
    : null

  return (
    <div className="space-y-4">
      {supportsAccounts && (
        <section className="rounded-lg border bg-card px-3 py-2">
          <button
            type="button"
            className="flex w-full items-center justify-between gap-3 text-left"
            aria-expanded={accountsExpanded}
            onClick={() => setAccountsExpanded((expanded) => !expanded)}
          >
            <div>
              <h2 className="text-sm font-medium">Saved Accounts</h2>
              <p className="text-xs text-muted-foreground">
                {accounts.length} {accounts.length === 1 ? "Account" : "Accounts"}
              </p>
            </div>
            <span className="text-xs text-muted-foreground">
              {accountsExpanded ? "Hide" : "Manage"}
            </span>
          </button>
          {accountsExpanded && (
            <>
              <p className="mt-2 text-xs text-muted-foreground">
                {plugin.meta.id === "codex"
                  ? "Add accounts without changing your main Codex CLI login."
                  : currentLogin?.isNew
                    ? `A new ${plugin.meta.name} login is ready to save.`
                    : currentLogin?.available
                      ? `The active ${plugin.meta.name} login is already saved.`
                      : `Log in with the official ${plugin.meta.name} client to add an account.`}
              </p>
          {plugin.meta.id === "claude" && currentLogin?.isNew && (
            <div className="mt-3 flex gap-2">
              <input
                value={profileName}
                onChange={(event) => setProfileName(event.target.value)}
                placeholder="Profile Name"
                aria-label="Profile Name"
                className="h-8 min-w-0 flex-1 rounded-md border bg-background px-2.5 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
              />
              <Button
                type="button"
                size="sm"
                disabled={saving || !profileName.trim()}
                onClick={() => void saveCurrentLogin()}
              >
                {saving ? "Saving…" : "Save Current Login"}
              </Button>
            </div>
          )}
          {plugin.meta.id === "codex" && codexOAuth?.status !== "waiting" && (
            <div className="mt-3 flex gap-2">
              <input
                value={profileName}
                onChange={(event) => setProfileName(event.target.value)}
                placeholder="Profile Name"
                aria-label="Profile Name"
                className="h-8 min-w-0 flex-1 rounded-md border bg-background px-2.5 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
              />
              <Button
                type="button"
                size="sm"
                disabled={startingOAuth || !profileName.trim()}
                onClick={() => void startCodexOAuth()}
              >
                {startingOAuth ? "Starting…" : "Add Codex Account"}
              </Button>
            </div>
          )}
          {plugin.meta.id === "codex" && codexOAuth?.status === "waiting" && (
            <div className="mt-3 rounded-md border bg-background p-3">
              <p className="text-xs text-muted-foreground">
                Copy this private sign-in link and open it in your browser.
              </p>
              {codexOAuth.authorizationUrl ? (
                <div className="mt-2">
                  <Button
                    type="button"
                    size="sm"
                    onClick={async () => {
                      await navigator.clipboard.writeText(codexOAuth.authorizationUrl!)
                      setLinkCopied(true)
                      window.setTimeout(() => setLinkCopied(false), 1500)
                    }}
                  >
                    {linkCopied ? "Login Link Copied" : "Copy Login Link"}
                  </Button>
                </div>
              ) : (
                <p className="mt-2 text-sm text-muted-foreground">Preparing Login Link…</p>
              )}
            </div>
          )}
          {accountError && <p className="mt-2 text-xs text-destructive">{accountError}</p>}
          {accounts.length > 0 && (
            <div className="mt-3 divide-y border-t">
              {accounts.map((account) => (
                <div key={account.id} className="flex items-center justify-between gap-3 py-2">
                  <input
                    value={account.name}
                    aria-label={`Profile Name For ${account.name}`}
                    onChange={(event) => {
                      const name = event.target.value
                      setAccounts((current) =>
                        current.map((item) => item.id === account.id ? { ...item, name } : item)
                      )
                    }}
                    onBlur={(event) => void renameAccount(account.id, event.target.value)}
                    onKeyDown={(event) => {
                      if (event.key === "Enter") event.currentTarget.blur()
                    }}
                    className="h-8 min-w-0 flex-1 rounded-md border border-transparent bg-transparent px-2 text-sm outline-none hover:border-border focus:border-border focus:bg-background focus-visible:ring-2 focus-visible:ring-ring"
                  />
                  <Button
                    type="button"
                    size="sm"
                    variant="ghost"
                    onClick={() => void removeAccount(account.id)}
                  >
                    {pendingRemoval === account.id ? "Confirm Remove" : "Remove"}
                  </Button>
                </div>
              ))}
            </div>
          )}
            </>
          )}
        </section>
      )}

      {accountStates.map((state) => (
        <section key={state.data?.instanceId ?? plugin.meta.id}>
          <ProviderCard
            name={state.data?.accountName
              ? `${state.data.accountName} - ${plugin.meta.name}`
              : plugin.meta.name}
            plan={state.data?.plan}
            links={plugin.meta.links}
            showSeparator={false}
            loading={state.loading}
            error={state.error}
            lines={combineAccountUsage
              ? splitCombinedUsageLines(state.data?.lines ?? []).accountLines
              : state.data?.lines ?? []}
            skeletonLines={plugin.meta.lines}
            lastManualRefreshAt={state.lastManualRefreshAt}
            lastUpdatedAt={state.lastUpdatedAt}
            onRetry={onRetry}
            scopeFilter="all"
            displayMode={displayMode}
            resetTimerDisplayMode={resetTimerDisplayMode}
            timeFormatMode={timeFormatMode}
            onResetTimerDisplayModeToggle={onResetTimerDisplayModeToggle}
          />
        </section>
      ))}
      {combinedUsageState && combinedUsageState.lines.length > 0 && (
        <section>
          <ProviderCard
            name={`Combined Usage - ${plugin.meta.name}`}
            showSeparator={false}
            loading={accountStates.some((state) => state.loading)}
            lines={combinedUsageState.lines}
            skeletonLines={plugin.meta.lines}
            lastUpdatedAt={combinedUsageState.state.lastUpdatedAt}
            scopeFilter="all"
            displayMode={displayMode}
            resetTimerDisplayMode={resetTimerDisplayMode}
            timeFormatMode={timeFormatMode}
            onResetTimerDisplayModeToggle={onResetTimerDisplayModeToggle}
          />
        </section>
      )}
      {supportsAccounts && accountsLoaded && accountStates.length === 0 && (
        <div className="py-8 text-center text-sm text-muted-foreground">
          Loading Accounts…
        </div>
      )}
    </div>
  )
}
