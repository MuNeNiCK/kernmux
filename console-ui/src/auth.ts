export function consumeFragmentToken(location: Location = window.location, history: History = window.history): string {
  const encoded = location.hash.startsWith("#token=") ? location.hash.slice("#token=".length) : ""
  history.replaceState(null, "", `${location.pathname}${location.search}`)
  let token = ""
  try { token = decodeURIComponent(encoded) } catch { throw new Error("Open this host with a valid management credential.") }
  if (token.length < 32 || token.length > 512 || /\s/.test(token)) {
    throw new Error("Open this host with a valid management credential.")
  }
  return token
}
