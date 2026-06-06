import { useState, type ReactNode } from 'react'
import { toast } from 'sonner'

import { cn } from '../lib/utils'
import { useConnectionStore } from '../stores/connectionStore'

export function ConnectionGate({ children }: { children: ReactNode }) {
  const baseUrl = useConnectionStore((state) => state.baseUrl)
  const apiToken = useConnectionStore((state) => state.apiToken)
  const setConnection = useConnectionStore((state) => state.setConnection)
  const [draftBaseUrl, setDraftBaseUrl] = useState(baseUrl)
  const [draftToken, setDraftToken] = useState(apiToken)
  const [showToken, setShowToken] = useState(false)

  if (apiToken.trim()) {
    return <>{children}</>
  }

  function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    const normalizedBaseUrl = draftBaseUrl.trim().replace(/\/$/, '')
    const normalizedToken = draftToken.trim()
    if (!normalizedBaseUrl) {
      toast.error('Backend URL is required')
      return
    }
    if (!normalizedToken) {
      toast.error('API token is required')
      return
    }
    setConnection({ baseUrl: normalizedBaseUrl, apiToken: normalizedToken })
  }

  return (
    <main className="flex min-h-screen items-center justify-center bg-[radial-gradient(circle_at_top_left,_rgba(34,211,238,0.16),_transparent_28rem),linear-gradient(135deg,#f8fafc_0%,#eef2ff_45%,#f5f3ff_100%)] px-6 py-10 text-slate-950">
      <div className="w-full max-w-lg rounded-3xl border border-white/70 bg-white/90 p-7 shadow-xl shadow-blue-950/10 backdrop-blur">
        <div className="flex items-center gap-3">
          <div className="flex h-11 w-11 items-center justify-center rounded-2xl bg-gradient-to-br from-cyan-500 via-blue-600 to-violet-600 text-sm font-semibold text-white shadow-sm shadow-blue-500/25">
            SW
          </div>
          <div>
            <p className="text-sm font-medium text-blue-600">Starweaver Claw Console</p>
            <h1 className="text-2xl font-semibold tracking-tight text-slate-950">
              Connect to runtime
            </h1>
          </div>
        </div>

        <p className="mt-4 text-sm leading-6 text-slate-500">
          Enter the backend URL and API token used by this browser. The values
          are stored locally for future visits.
        </p>

        <form className="mt-7 space-y-5" onSubmit={submit}>
          <label className="block">
            <span className="text-sm font-medium text-slate-700">
              Backend URL
            </span>
            <input
              className="mt-2 w-full rounded-xl border border-slate-200 bg-slate-50 px-3 py-2.5 text-sm outline-none ring-blue-600 transition focus:bg-white focus:ring-2"
              value={draftBaseUrl}
              onChange={(event) => setDraftBaseUrl(event.target.value)}
              placeholder={
                typeof window !== 'undefined'
                  ? window.location.origin
                  : 'http://127.0.0.1:9042'
              }
              autoComplete="url"
            />
            <span className="mt-1 block text-xs text-slate-400">
              Defaults to the current address bar origin.
            </span>
          </label>

          <label className="block">
            <span className="text-sm font-medium text-slate-700">
              API Token
            </span>
            <div className="mt-2 flex rounded-xl border border-slate-200 bg-slate-50 transition focus-within:bg-white focus-within:ring-2 focus-within:ring-blue-600">
              <input
                className="min-w-0 flex-1 rounded-l-xl bg-transparent px-3 py-2.5 text-sm outline-none"
                value={draftToken}
                onChange={(event) => setDraftToken(event.target.value)}
                type={showToken ? 'text' : 'password'}
                placeholder="STARWEAVER_CLAW_API_TOKEN"
                autoComplete="current-password"
              />
              <button
                type="button"
                className="rounded-r-xl border-l border-slate-200 px-3 text-xs font-medium text-slate-600 transition hover:bg-slate-100"
                onClick={() => setShowToken((current) => !current)}
              >
                {showToken ? 'Hide' : 'Show'}
              </button>
            </div>
          </label>

          <button
            type="submit"
            className={cn(
              'w-full rounded-xl bg-blue-600 px-4 py-2.5 text-sm font-semibold text-white shadow-sm transition hover:bg-blue-700',
              'focus:outline-none focus:ring-2 focus:ring-blue-600 focus:ring-offset-2',
            )}
          >
            Enter console
          </button>
        </form>
      </div>
    </main>
  )
}
