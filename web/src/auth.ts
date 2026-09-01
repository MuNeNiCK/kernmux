let bearerToken: string | null = null

export function consumeTokenHash(
  hash: string,
  replace: (hash: string) => void,
): string | null {
  if (!hash.startsWith("#token=")) return null
  const [encodedToken, encodedRoute] = hash.slice("#token=".length).split("&route=", 2)
  const token = decodeURIComponent(encodedToken).trim()
  const route = encodedRoute ? decodeURIComponent(encodedRoute).replace(/^\/?/, "") : ""
  replace(route ? `#/${route}` : "")
  return token || null
}

export function initializeToken(): string | null {
  const consumed = consumeTokenHash(window.location.hash, (hash) => {
    window.history.replaceState(null, "", `${window.location.pathname}${window.location.search}${hash}`)
  })
  if (consumed) bearerToken = consumed
  return bearerToken
}

export function token(): string | null {
  return bearerToken
}

export function clearToken(): void {
  bearerToken = null
}
