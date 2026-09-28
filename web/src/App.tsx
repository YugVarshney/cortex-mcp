import { useCallback, useEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import { api } from "./api";
import type { MemoryPage, Namespace, RecallHit, Stats } from "./api";

function formatScore(value: number): string {
  return value.toFixed(3);
}

export default function App() {
  // Namespace list is shared: creating one in the namespaces section must
  // immediately be selectable in the search section.
  const [namespaces, setNamespaces] = useState<Namespace[] | null>(null);

  const refreshNamespaces = useCallback(async () => {
    try {
      setNamespaces(await api.listNamespaces());
    } catch {
      // The per-section error regions render details; keep the shared list here.
    }
  }, []);

  useEffect(() => {
    void refreshNamespaces();
  }, [refreshNamespaces]);

  return (
    <div className="mx-auto max-w-4xl px-4 py-8">
      <header>
        <h1 className="text-2xl font-bold">Cortex-MCP memory console</h1>
        <nav aria-label="Section navigation" className="mt-2">
          <ul className="flex flex-wrap gap-4">
            <li>
              <a className="text-blue-700 underline" href="#namespaces">Namespaces</a>
            </li>
            <li>
              <a className="text-blue-700 underline" href="#search">Search</a>
            </li>
            <li>
              <a className="text-blue-700 underline" href="#memories">Memories</a>
            </li>
            <li>
              <a className="text-blue-700 underline" href="#capture">Capture</a>
            </li>
            <li>
              <a className="text-blue-700 underline" href="#stats">Stats</a>
            </li>
            <li>
              <a className="text-blue-700 underline" href="#metrics">Metrics</a>
            </li>
          </ul>
        </nav>
      </header>

      <NamespacesSection namespaces={namespaces} onRefresh={refreshNamespaces} />
      <SearchSection namespaces={namespaces} onRefresh={refreshNamespaces} />
      <MemoriesSection namespaces={namespaces} />
      <CaptureSection namespaces={namespaces} onRefresh={refreshNamespaces} />
      <StatsSection />
      <MetricsSection />

      <footer className="mt-10 text-sm text-gray-600">
        Local-first memory broker · REST + MCP · scores explained on every result.
      </footer>
    </div>
  );
}

interface NamespacesSectionProps {
  namespaces: Namespace[] | null;
  onRefresh: () => Promise<void>;
}

function NamespacesSection({ namespaces, onRefresh }: NamespacesSectionProps) {
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setError(null);
    try {
      await api.createNamespace(name);
      setNotice(`Namespace “${name}” created.`);
      setName("");
      await onRefresh();
    } catch (e) {
      setNotice(null);
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <section id="namespaces" aria-labelledby="namespaces-heading" className="mt-8">
      <h2 id="namespaces-heading" className="text-xl font-semibold">Namespaces</h2>
      <p className="mt-1 text-sm text-gray-700">Workspaces never share memories with each other.</p>

      <form onSubmit={submit} className="mt-3 flex flex-wrap items-end gap-2">
        <div>
          <label htmlFor="namespace-name" className="block text-sm font-medium">New namespace name</label>
          <input
            id="namespace-name"
            value={name}
            onChange={(e) => setName(e.target.value)}
            required
            className="mt-1 rounded border border-gray-400 px-2 py-1"
          />
        </div>
        <button type="submit" className="rounded bg-blue-700 px-3 py-1.5 font-medium text-white">
          Create namespace
        </button>
      </form>

      <p role="status" aria-live="polite" className={`mt-2 min-h-5 text-sm ${error ? "text-red-700" : "text-green-800"}`}>
        {error ?? notice ?? ""}
      </p>

      <table className="mt-2 w-full border-collapse text-left text-sm">
        <caption className="sr-only">All namespaces with creation time</caption>
        <thead>
          <tr className="border-b border-gray-300">
            <th scope="col" className="py-1 pr-4">Name</th>
            <th scope="col" className="py-1 pr-4">Created (unix)</th>
          </tr>
        </thead>
        <tbody>
          {(namespaces ?? []).map((ns) => (
            <tr key={ns.id} className="border-b border-gray-200">
              <td className="py-1 pr-4">{ns.name}</td>
              <td className="py-1 pr-4">{ns.created_at}</td>
            </tr>
          ))}
          {namespaces && namespaces.length === 0 && (
            <tr>
              <td colSpan={2} className="py-1 text-gray-600">No namespaces yet.</td>
            </tr>
          )}
        </tbody>
      </table>
    </section>
  );
}

interface SearchSectionProps {
  namespaces: Namespace[] | null;
  onRefresh: () => Promise<void>;
}

function SearchSection({ namespaces, onRefresh }: SearchSectionProps) {
  const [namespace, setNamespace] = useState("");
  const [memoryText, setMemoryText] = useState("");
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<RecallHit[] | null>(null);
  const [status, setStatus] = useState<{ kind: "info" | "error"; text: string } | null>(null);

  // Follow the shared list: default to the first namespace until the user picks one.
  useEffect(() => {
    const first = namespaces?.[0];
    if (first && !namespaces.some((ns) => ns.name === namespace)) {
      setNamespace(first.name);
    }
  }, [namespaces, namespace]);

  const remember = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    try {
      await api.remember(namespace, memoryText, []);
      setStatus({ kind: "info", text: "Memory stored." });
      setMemoryText("");
      await onRefresh();
    } catch (e) {
      setStatus({ kind: "error", text: e instanceof Error ? e.message : String(e) });
    }
  };

  const search = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    try {
      const results = await api.recall(namespace, query, 5);
      setHits(results);
      setStatus({ kind: "info", text: `${results.length} result(s).` });
    } catch (e) {
      setHits(null);
      setStatus({ kind: "error", text: e instanceof Error ? e.message : String(e) });
    }
  };

  return (
    <section id="search" aria-labelledby="search-heading" className="mt-10">
      <h2 id="search-heading" className="text-xl font-semibold">Search memories</h2>

      <form onSubmit={remember} className="mt-3 flex flex-wrap items-end gap-2">
        <div>
          <label htmlFor="remember-namespace" className="block text-sm font-medium">Namespace</label>
          <select
            id="remember-namespace"
            value={namespace}
            onChange={(e) => setNamespace(e.target.value)}
            className="mt-1 rounded border border-gray-400 px-2 py-1"
          >
            {(namespaces ?? []).map((ns) => (
              <option key={ns.id} value={ns.name}>{ns.name}</option>
            ))}
          </select>
        </div>
        <div className="grow">
          <label htmlFor="remember-text" className="block text-sm font-medium">New memory text</label>
          <input
            id="remember-text"
            value={memoryText}
            onChange={(e) => setMemoryText(e.target.value)}
            required
            className="mt-1 w-full rounded border border-gray-400 px-2 py-1"
          />
        </div>
        <button type="submit" className="rounded bg-blue-700 px-3 py-1.5 font-medium text-white">
          Remember
        </button>
      </form>

      <form onSubmit={search} className="mt-4 flex flex-wrap items-end gap-2">
        <div className="grow">
          <label htmlFor="search-query" className="block text-sm font-medium">Query</label>
          <input
            id="search-query"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            required
            className="mt-1 w-full rounded border border-gray-400 px-2 py-1"
          />
        </div>
        <button type="submit" className="rounded bg-blue-700 px-3 py-1.5 font-medium text-white">
          Recall
        </button>
      </form>

      <p
        role="status"
        aria-live="polite"
        className={`mt-2 min-h-5 text-sm ${status?.kind === "error" ? "text-red-700" : "text-green-800"}`}
      >
        {status?.text ?? ""}
      </p>

      {hits && (
        <div className="mt-2 overflow-x-auto">
          <table className="w-full border-collapse text-left text-sm">
            <caption className="sr-only">Recalled memories with score breakdowns</caption>
            <thead>
              <tr className="border-b border-gray-300">
                <th scope="col" className="py-1 pr-3">Memory</th>
                <th scope="col" className="py-1 pr-3">BM25</th>
                <th scope="col" className="py-1 pr-3">Vector</th>
                <th scope="col" className="py-1 pr-3">Recency</th>
                <th scope="col" className="py-1 pr-3">Pinned</th>
                <th scope="col" className="py-1 pr-3">Total</th>
              </tr>
            </thead>
            <tbody>
              {hits.map((hit) => (
                <tr key={hit.id} className="border-b border-gray-200 align-top">
                  <td className="py-1 pr-3">
                    {hit.text}
                    {hit.tags.length > 0 && (
                      <span className="ml-2 text-gray-600">[{hit.tags.join(", ")}]</span>
                    )}
                    {hit.pinned && <span className="ml-2 font-medium">(pinned)</span>}
                  </td>
                  <td className="py-1 pr-3 tabular-nums">{formatScore(hit.breakdown.bm25)}</td>
                  <td className="py-1 pr-3 tabular-nums">{formatScore(hit.breakdown.vector)}</td>
                  <td className="py-1 pr-3 tabular-nums">{formatScore(hit.breakdown.recency)}</td>
                  <td className="py-1 pr-3 tabular-nums">{formatScore(hit.breakdown.pinned_boost)}</td>
                  <td className="py-1 pr-3 font-medium tabular-nums">{formatScore(hit.breakdown.total)}</td>
                </tr>
              ))}
              {hits.length === 0 && (
                <tr>
                  <td colSpan={6} className="py-1 text-gray-600">No matching memories.</td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

const PAGE_SIZES = [10, 25, 50] as const;

interface MemoriesSectionProps {
  namespaces: Namespace[] | null;
}

function MemoriesSection({ namespaces }: MemoriesSectionProps) {
  // Empty string selects every namespace: `GET /v1/memories` treats an absent
  // namespace filter as "all".
  const [namespace, setNamespace] = useState("");
  const [limit, setLimit] = useState<number>(PAGE_SIZES[0]);
  const [page, setPage] = useState(0);
  const [pageData, setPageData] = useState<MemoryPage | null>(null);
  const [status, setStatus] = useState<{ kind: "info" | "error"; text: string } | null>(null);
  // Drops stale responses when page/namespace changes race each other.
  const loadSeq = useRef(0);

  useEffect(() => {
    const seq = ++loadSeq.current;
    api
      .listMemories(namespace || undefined, limit, page * limit)
      .then((fresh) => {
        if (seq !== loadSeq.current) return;
        // The store shrank while we were viewing past the last row: follow it.
        if (fresh.total > 0 && page * limit >= fresh.total) {
          setPage(Math.max(0, Math.ceil(fresh.total / limit) - 1));
          return;
        }
        setPageData(fresh);
        setStatus({
          kind: "info",
          text:
            fresh.total === 0
              ? "No memories yet."
              : `Page ${page + 1} of ${Math.ceil(fresh.total / limit)} · memories ${
                  page * limit + 1
                }–${page * limit + fresh.memories.length} of ${fresh.total} · newest first`,
        });
      })
      .catch((e) => {
        if (seq !== loadSeq.current) return;
        setStatus({ kind: "error", text: e instanceof Error ? e.message : String(e) });
      });
  }, [namespace, limit, page]);

  const total = pageData?.total ?? 0;
  const nextDisabled = pageData === null || (page + 1) * limit >= total;

  const changePage = (delta: number) => {
    setStatus(null);
    setPage((current) => Math.max(0, current + delta));
  };

  return (
    <section id="memories" aria-labelledby="memories-heading" className="mt-10">
      <h2 id="memories-heading" className="text-xl font-semibold">Browse memories</h2>
      <p className="mt-1 text-sm text-gray-700">
        Every stored memory, newest first, one page at a time.
      </p>

      <div className="mt-3 flex flex-wrap items-end gap-2">
        <div>
          <label htmlFor="memories-namespace" className="block text-sm font-medium">Namespace</label>
          <select
            id="memories-namespace"
            value={namespace}
            onChange={(e) => {
              setNamespace(e.target.value);
              setPage(0);
            }}
            className="mt-1 rounded border border-gray-400 px-2 py-1"
          >
            <option value="">All namespaces</option>
            {(namespaces ?? []).map((ns) => (
              <option key={ns.id} value={ns.name}>{ns.name}</option>
            ))}
          </select>
        </div>
        <div>
          <label htmlFor="memories-page-size" className="block text-sm font-medium">Memories per page</label>
          <select
            id="memories-page-size"
            value={limit}
            onChange={(e) => {
              setLimit(Number(e.target.value));
              setPage(0);
            }}
            className="mt-1 rounded border border-gray-400 px-2 py-1"
          >
            {PAGE_SIZES.map((size) => (
              <option key={size} value={size}>{size}</option>
            ))}
          </select>
        </div>
        <button
          type="button"
          onClick={() => changePage(-1)}
          disabled={page === 0}
          className="rounded bg-blue-700 px-3 py-1.5 font-medium text-white disabled:opacity-50"
        >
          Previous page
        </button>
        <button
          type="button"
          onClick={() => changePage(1)}
          disabled={nextDisabled}
          className="rounded bg-blue-700 px-3 py-1.5 font-medium text-white disabled:opacity-50"
        >
          Next page
        </button>
      </div>

      <p
        role="status"
        aria-live="polite"
        className={`mt-2 min-h-5 text-sm ${status?.kind === "error" ? "text-red-700" : "text-green-800"}`}
      >
        {status?.text ?? ""}
      </p>

      {pageData && (
        <table className="mt-2 w-full border-collapse text-left text-sm">
          <caption className="sr-only">Paged list of stored memories</caption>
          <thead>
            <tr className="border-b border-gray-300">
              <th scope="col" className="py-1 pr-4">Memory</th>
              <th scope="col" className="py-1 pr-4">Tags</th>
              <th scope="col" className="py-1 pr-4">Source</th>
              <th scope="col" className="py-1 pr-4">Pinned</th>
              <th scope="col" className="py-1 pr-4">Created (unix)</th>
            </tr>
          </thead>
          <tbody>
            {pageData.memories.map((memory) => (
              <tr key={memory.id} className="border-b border-gray-200 align-top">
                <td className="py-1 pr-4">{memory.text}</td>
                <td className="py-1 pr-4">{memory.tags.length > 0 ? memory.tags.join(", ") : ""}</td>
                <td className="py-1 pr-4">{memory.source}</td>
                <td className="py-1 pr-4">{memory.pinned ? "Yes" : "No"}</td>
                <td className="py-1 pr-4 tabular-nums">{memory.created_at}</td>
              </tr>
            ))}
            {pageData.memories.length === 0 && (
              <tr>
                <td colSpan={5} className="py-1 text-gray-600">No memories in this view.</td>
              </tr>
            )}
          </tbody>
        </table>
      )}
    </section>
  );
}

interface CaptureDraft {
  role: string;
  content: string;
}

interface CaptureSectionProps {
  namespaces: Namespace[] | null;
  onRefresh: () => Promise<void>;
}

function CaptureSection({ namespaces, onRefresh }: CaptureSectionProps) {
  const [namespace, setNamespace] = useState("");
  const [tags, setTags] = useState("");
  const [messages, setMessages] = useState<CaptureDraft[]>([{ role: "", content: "" }]);
  const [status, setStatus] = useState<{ kind: "info" | "error"; text: string } | null>(null);
  // Pre-fill once from the shared list, then never again: the field is free
  // text (the endpoint auto-creates unknown namespaces, D-009), so a refresh
  // must not clobber what the user typed.
  const namespaceTouched = useRef(false);

  useEffect(() => {
    const first = namespaces?.[0];
    if (first && !namespaceTouched.current) {
      setNamespace(first.name);
    }
  }, [namespaces]);

  const editMessage = (index: number, patch: Partial<CaptureDraft>) => {
    setMessages((current) =>
      current.map((message, i) => (i === index ? { ...message, ...patch } : message)),
    );
  };

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const payload = {
      namespace,
      tags: tags.split(",").map((tag) => tag.trim()).filter(Boolean),
      transcript: messages.map((message) => ({
        content: message.content,
        ...(message.role.trim() ? { role: message.role.trim() } : {}),
      })),
    };
    try {
      const result = await api.capture(payload);
      const unit = result.captured === 1 ? "memory" : "memories";
      setStatus({
        kind: "info",
        text: `Captured ${result.captured} ${unit} into “${result.namespace}”.`,
      });
      setMessages([{ role: "", content: "" }]);
      setTags("");
      await onRefresh();
    } catch (e) {
      setStatus({ kind: "error", text: e instanceof Error ? e.message : String(e) });
    }
  };

  return (
    <section id="capture" aria-labelledby="capture-heading" className="mt-10">
      <h2 id="capture-heading" className="text-xl font-semibold">Capture a transcript</h2>
      <p className="mt-1 text-sm text-gray-700">
        Split conversation turns into chunked, embedded memories (source “capture”).
      </p>

      <form onSubmit={submit} className="mt-3 space-y-3">
        <div className="flex flex-wrap items-end gap-2">
          <div>
            <label htmlFor="capture-namespace" className="block text-sm font-medium">Namespace</label>
            <input
              id="capture-namespace"
              value={namespace}
              onChange={(e) => {
                namespaceTouched.current = true;
                setNamespace(e.target.value);
              }}
              list="capture-namespace-options"
              required
              className="mt-1 rounded border border-gray-400 px-2 py-1"
            />
            <datalist id="capture-namespace-options">
              {(namespaces ?? []).map((ns) => (
                <option key={ns.id} value={ns.name} />
              ))}
            </datalist>
            <p className="mt-1 text-sm text-gray-700">Created automatically if it does not exist.</p>
          </div>
          <div>
            <label htmlFor="capture-tags" className="block text-sm font-medium">Tags (comma-separated)</label>
            <input
              id="capture-tags"
              value={tags}
              onChange={(e) => setTags(e.target.value)}
              className="mt-1 rounded border border-gray-400 px-2 py-1"
            />
          </div>
        </div>

        <ol className="space-y-3">
          {messages.map((message, index) => (
            <li key={index}>
              <fieldset className="rounded border border-gray-300 p-3">
                <legend className="px-1 text-sm font-medium">Message {index + 1}</legend>
                <div className="flex flex-wrap items-end gap-2">
                  <div>
                    <label htmlFor={`capture-role-${index}`} className="block text-sm font-medium">
                      Role (optional)
                    </label>
                    <input
                      id={`capture-role-${index}`}
                      value={message.role}
                      onChange={(e) => editMessage(index, { role: e.target.value })}
                      className="mt-1 rounded border border-gray-400 px-2 py-1"
                    />
                  </div>
                  <div className="grow">
                    <label htmlFor={`capture-content-${index}`} className="block text-sm font-medium">
                      Content
                    </label>
                    <textarea
                      id={`capture-content-${index}`}
                      value={message.content}
                      onChange={(e) => editMessage(index, { content: e.target.value })}
                      required
                      rows={2}
                      className="mt-1 w-full rounded border border-gray-400 px-2 py-1"
                    />
                  </div>
                  <button
                    type="button"
                    onClick={() => setMessages((current) => current.filter((_, i) => i !== index))}
                    disabled={messages.length === 1}
                    aria-label={`Remove message ${index + 1}`}
                    className="rounded bg-blue-700 px-3 py-1.5 font-medium text-white disabled:opacity-50"
                  >
                    Remove
                  </button>
                </div>
              </fieldset>
            </li>
          ))}
        </ol>

        <div className="flex flex-wrap gap-2">
          <button
            type="button"
            onClick={() => setMessages((current) => [...current, { role: "", content: "" }])}
            className="rounded border border-gray-400 px-3 py-1.5 font-medium"
          >
            Add message
          </button>
          <button type="submit" className="rounded bg-blue-700 px-3 py-1.5 font-medium text-white">
            Capture transcript
          </button>
        </div>
      </form>

      <p
        role="status"
        aria-live="polite"
        className={`mt-2 min-h-5 text-sm ${status?.kind === "error" ? "text-red-700" : "text-green-800"}`}
      >
        {status?.text ?? ""}
      </p>
    </section>
  );
}

interface MetricSample {
  name: string;
  value: string;
}

/** Flatten Prometheus text exposition (0.0.4) into name/value rows: comment
 * lines (# HELP / # TYPE) are stripped; every sample line becomes one row. */
export function parseMetrics(text: string): MetricSample[] {
  return text
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0 && !line.startsWith("#"))
    .map((line) => {
      const splitAt = line.lastIndexOf(" ");
      return { name: line.slice(0, splitAt), value: line.slice(splitAt + 1) };
    });
}

function MetricsSection() {
  const [samples, setSamples] = useState<MetricSample[] | null>(null);
  const [rawText, setRawText] = useState<string | null>(null);
  const [status, setStatus] = useState<{ kind: "info" | "error"; text: string } | null>(null);

  const load = useCallback(async () => {
    try {
      const text = await api.metrics();
      const parsed = parseMetrics(text);
      setRawText(text);
      setSamples(parsed);
      setStatus({ kind: "info", text: `Metrics refreshed · ${parsed.length} samples.` });
    } catch (e) {
      setStatus({ kind: "error", text: e instanceof Error ? e.message : String(e) });
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <section id="metrics" aria-labelledby="metrics-heading" className="mt-10">
      <h2 id="metrics-heading" className="text-xl font-semibold">Server metrics</h2>
      <p className="mt-1 text-sm text-gray-700">
        The same counters and gauges <code>/metrics</code> exposes to Prometheus.
      </p>

      <button
        type="button"
        onClick={() => void load()}
        className="mt-3 rounded bg-blue-700 px-3 py-1.5 font-medium text-white"
      >
        Refresh metrics
      </button>

      <p
        role="status"
        aria-live="polite"
        className={`mt-2 min-h-5 text-sm ${status?.kind === "error" ? "text-red-700" : "text-green-800"}`}
      >
        {status?.text ?? ""}
      </p>

      {samples && (
        <>
          <table className="mt-2 w-full border-collapse text-left text-sm">
            <caption className="sr-only">Server metrics with values</caption>
            <thead>
              <tr className="border-b border-gray-300">
                <th scope="col" className="py-1 pr-4">Metric</th>
                <th scope="col" className="py-1 pr-4">Value</th>
              </tr>
            </thead>
            <tbody>
              {samples.map((sample) => (
                <tr key={sample.name} className="border-b border-gray-200">
                  <th scope="row" className="py-1 pr-4 font-normal">{sample.name}</th>
                  <td className="py-1 pr-4 tabular-nums">{sample.value}</td>
                </tr>
              ))}
            </tbody>
          </table>
          {rawText !== null && (
            <details className="mt-3">
              <summary className="font-medium">Raw Prometheus text</summary>
              <pre
                tabIndex={0}
                aria-label="Raw Prometheus text"
                className="mt-2 max-h-80 overflow-auto rounded border border-gray-300 p-2 text-xs"
              >
                {rawText}
              </pre>
            </details>
          )}
        </>
      )}
    </section>
  );
}

function StatsSection() {
  const [stats, setStats] = useState<Stats | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .stats()
      .then(setStats)
      .catch((e) => setError(e instanceof Error ? e.message : String(e)));
  }, []);

  return (
    <section id="stats" aria-labelledby="stats-heading" className="mt-10">
      <h2 id="stats-heading" className="text-xl font-semibold">Server stats</h2>
      {error && <p role="alert" className="mt-2 text-sm text-red-700">{error}</p>}
      {stats && (
        <>
          <dl className="mt-3 grid grid-cols-3 gap-4 text-sm">
            <div>
              <dt className="font-medium">Namespaces</dt>
              <dd className="tabular-nums">{stats.total_namespaces}</dd>
            </div>
            <div>
              <dt className="font-medium">Memories</dt>
              <dd className="tabular-nums">{stats.total_memories}</dd>
            </div>
            <div>
              <dt className="font-medium">Pinned</dt>
              <dd className="tabular-nums">{stats.pinned_memories}</dd>
            </div>
          </dl>
          <table className="mt-4 w-full border-collapse text-left text-sm">
            <caption className="sr-only">Memories per namespace</caption>
            <thead>
              <tr className="border-b border-gray-300">
                <th scope="col" className="py-1 pr-4">Namespace</th>
                <th scope="col" className="py-1 pr-4">Memories</th>
              </tr>
            </thead>
            <tbody>
              {stats.per_namespace.map((row) => (
                <tr key={row.namespace} className="border-b border-gray-200">
                  <td className="py-1 pr-4">{row.namespace}</td>
                  <td className="py-1 pr-4 tabular-nums">{row.count}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </>
      )}
    </section>
  );
}
