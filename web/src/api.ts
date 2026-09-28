/** Typed API client for the Recall-MCP HTTP API. Relative paths: the UI is
 * served by the same server in production; vite proxies in dev. */

export interface Namespace {
  id: string;
  name: string;
  created_at: number;
}

export interface Memory {
  id: string;
  namespace_id: string;
  text: string;
  tags: string[];
  source: string;
  pinned: boolean;
  created_at: number;
  last_accessed_at: number;
}

/** One server-driven page of memories plus the unpaginated total. */
export interface MemoryPage {
  memories: Memory[];
  total: number;
}

export interface ScoreBreakdown {
  bm25: number;
  vector: number;
  recency: number;
  pinned_boost: number;
  total: number;
}

export interface RecallHit {
  id: string;
  namespace_id: string;
  text: string;
  tags: string[];
  source: string;
  pinned: boolean;
  created_at: number;
  last_accessed_at: number;
  breakdown: ScoreBreakdown;
}

export interface Stats {
  total_namespaces: number;
  total_memories: number;
  pinned_memories: number;
  per_namespace: { namespace: string; count: number }[];
}

export interface CaptureMessage {
  role?: string;
  content: string;
}

export interface CapturePayload {
  namespace: string;
  tags: string[];
  transcript: CaptureMessage[];
}

export interface CaptureResult {
  namespace: string;
  captured: number;
  memories: Memory[];
}

export interface ApiError {
  error: string;
}

/** Fetch that throws the API's `{"error": ...}` message on failure and returns
 * the Response on success — callers may need headers (`X-Total-Count`) or
 * non-JSON bodies (`/metrics` text). */
async function requestResponse(path: string, init?: RequestInit): Promise<Response> {
  const res = await fetch(path, {
    headers: { "content-type": "application/json" },
    ...init,
  });
  if (!res.ok) {
    const body: unknown = await res.json().catch(() => null);
    const detail =
      body && typeof body === "object" && "error" in body
        ? String((body as ApiError).error)
        : `${res.status} ${res.statusText}`;
    throw new Error(detail);
  }
  return res;
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await requestResponse(path, init);
  return (await res.json()) as T;
}

export const api = {
  listNamespaces: () => request<Namespace[]>("/v1/namespaces"),
  createNamespace: (name: string) =>
    request<Namespace>("/v1/namespaces", { method: "POST", body: JSON.stringify({ name }) }),
  remember: (namespace: string, text: string, tags: string[]) =>
    request<unknown>("/v1/memories", {
      method: "POST",
      body: JSON.stringify({ namespace, text, tags }),
    }),
  recall: (namespace: string, query: string, k: number) =>
    request<RecallHit[]>("/v1/recall", {
      method: "POST",
      body: JSON.stringify({ namespace, query, k }),
    }),
  stats: () => request<Stats>("/v1/stats"),
  /** `GET /v1/memories` with server-side pagination; the total comes from the
   * `X-Total-Count` response header, not the page body. */
  listMemories: async (
    namespace: string | undefined,
    limit: number,
    offset: number,
  ): Promise<MemoryPage> => {
    const params = new URLSearchParams({ limit: String(limit), offset: String(offset) });
    if (namespace) params.set("namespace", namespace);
    const res = await requestResponse(`/v1/memories?${params.toString()}`);
    const header = res.headers.get("x-total-count");
    const total = header === null ? Number.NaN : Number(header);
    if (Number.isNaN(total)) {
      throw new Error("list memories response is missing X-Total-Count");
    }
    return { memories: (await res.json()) as Memory[], total };
  },
  capture: (payload: CapturePayload) =>
    request<CaptureResult>("/v1/capture", { method: "POST", body: JSON.stringify(payload) }),
  /** Prometheus text exposition (0.0.4), parsed for display by `parseMetrics`. */
  metrics: async (): Promise<string> => (await requestResponse("/metrics")).text(),
};
