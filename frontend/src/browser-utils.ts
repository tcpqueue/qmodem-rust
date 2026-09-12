export function requestId(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[6] = (bytes[6]! & 15) | 64;
  bytes[8] = (bytes[8]! & 63) | 128;
  const hex = Array.from(bytes, b => b.toString(16).padStart(2, "0")).join("");
  return [hex.slice(0,8),hex.slice(8,12),hex.slice(12,16),hex.slice(16,20),hex.slice(20)].join("-");
}
export function cloneConfig<T>(value: T): T { return JSON.parse(JSON.stringify(value)); }

const tokenKey = "qmodem.access-token";
export function savedToken(): string {
  try { return sessionStorage.getItem(tokenKey) || ""; } catch { return ""; }
}
export function rememberToken(token: string): void {
  try {
    if (token) sessionStorage.setItem(tokenKey, token);
    else sessionStorage.removeItem(tokenKey);
  } catch { /* Storage may be disabled by the browser. */ }
}
