# Competitive landscape: AI-agent memory (researched 2026-09-06)

All star counts / activity observed on 2026-09-06 via the GitHub API. Pricing
captured from official pricing pages on the same date. Complaints cite specific
HN/Reddit threads. This file feeds the roadmap: see "Feature recommendations".

## TL;DR

- The "memory layer" market has consolidated into three SaaS leaders (Mem0,
  Zep, Letta) priced -/mo, plus a large open-source cluster (Cognee,
  Graphiti, Supermemory, memU) and a long tail of **local-first MCP memory
  servers**; exactly Recall-MCP's segment, but none of the local servers is
  dominant, and **nobody ships explainable retrieval**.
- The closest direct competitor is **Palace** (Rust, local-first, coding-agent
  memory, MIT core + paid team server), which validates both the positioning and
  the open-core monetization, at real prices (€1,490/yr team tier).
- MCP is now table stakes: Mem0, Zep, Cognee, Supermemory and every local
  server expose MCP. The MCP spec's 2026-07-28 revision (stateless-by-default,
  sessions removed) favors our stateless design.
- Vendor complaints cluster on: black-box memory quality ("doesn't learn user
  patterns"), temporal errors, heavy Python/Node runtimes, Docker + API-key
  setup friction, and per-request credit costs. Every one of these is a
  Recall-MCP talking point.

## The big hosted players

| Player | What it is | Hosted / local | Pricing (official page) | GitHub | 
|---|---|---|---|---|
| Mem0 | Managed memory layer + OSS Python lib; multi-level memory (user/session/agent), platform-exclusive Graph Memory, Memory Decay, Temporal Reasoning | Hosted platform; OSS self-host (Python) | Free (10k memories / 10k req/mo), $19/mo (50k req), $249/mo (500k req), Enterprise custom, [mem0.ai/pricing](https://mem0.ai/pricing) | [mem0ai/mem0](https://github.com/mem0ai/mem0) 64.8k★, Apache-2.0, pushed 2026-09-04 |
| Letta (ex-MemGPT) | Stateful-agent platform; OS-style memory (core/recall/archival), self-editing memory; agent-server, not a drop-in store | Cloud + self-host OSS (Apache-2.0, Python) | Free (3 agents, BYOK), Pro $20/mo (20 agents), API $20/mo + $0.10/agent/mo + $0.00015/s tool-exec, Teams $20/seat, Enterprise, [docs.letta.com/letta-code/pricing](https://docs.letta.com/letta-code/pricing) | [letta-ai/letta](https://github.com/letta-ai/letta) 24.6k★, pushed 2026-08-23 |
| Zep (Graphiti) | Enterprise memory on a temporal knowledge graph ("context graphs", <200ms retrieval, SOC 2) | Cloud only for the memory product; Graphiti OSS self-hosts on Neo4j/FalkorDB; self-hosting the cloud equivalent is Enterprise BYOC | Credit-based: Free 10k credits/mo, Flex $125/mo (50k credits), Flex Plus $375/mo (200k), Enterprise (BYOK/BYOC, HIPAA BAA), [getzep.com/pricing](https://www.getzep.com/pricing) | [getzep/graphiti](https://github.com/getzep/graphiti) 30.6k★, pushed 2026-09-06 |
| Cognee | Open-source "AI memory platform": ECL pipelines → graph + vector + relational store; MCP server (`Codify`/`Search`/`Prune`) | Self-host OSS (Python + Rust SDK); managed/enterprise (~$3.5k/mo per [Graphlit comparison](https://www.graphlit.com/blog)) | OSS free; [cost calculator](https://www.cognee.ai/cost-calculator); enterprise custom | [topoteretes/cognee](https://github.com/topoteretes/cognee) 30.5k★, pushed 2026-09-06 |
| LangMem | LangChain's memory SDK: extract/consolidate semantic-episodic-procedural memory, hot-path tools + background manager, persists in LangGraph `BaseStore` | Library (Python), storage anywhere LangGraph stores; long-term store ships with LangGraph Platform | OSS (MIT); costs = LangGraph Platform + LLM API | [langchain-ai/langmem](https://github.com/langchain-ai/langmem) 1.6k★, pushed 2026-09-04 |

### Latest 2026 moves (evidence they're competing on distribution, not storage)

- **Mem0** ([docs.mem0.ai/changelog](https://docs.mem0.ai/changelog)): Kimi Code
  plugin with hosted MCP + auto capture/recall hooks (2026-08-13); DeepSeek
  harness plugin (2026-08-24); AWS Strands `MemoryStore` with "recall on every
  turn, no tool call required" (2026-08-24); n8n/Zapier nodes (2026-07-30);
  TS SDK 3.1 with 26 providers + 4 rerankers (2026-07-13); Memory Decay +
  Temporal Reasoning + v3 algorithm rewrite claiming LongMemEval 67.8 → 93.4
  (May-Apr 2026). Lesson: **auto-capture hooks and IDE/agent plugins are where
  the leaders are spending their year**, plus a benchmark-claim arms race.
- **Letta**: "Memory Models" (June 2026): memory-native RL so models learn to
  curate their own memory ([letta.com/research](https://www.letta.com/research/));
  Context Constitution (Apr 2026); earlier, the famous result that a **plain
  filesystem scored 74.0% on LoCoMo, beating Mem0's graph variant (68.5%)**
  ([letta.com/blog/benchmarking-ai-agent-memory](https://www.letta.com/blog/benchmarking-ai-agent-memory/),
  (Aug 2025): "memory is more about how agents manage context than the exact
  retrieval mechanism".
- **Zep**: MCP Server seats sold per plan; credit metering where retrieval and
  storage cost 0 credits but each 350-byte "episode" costs ≥1 credit
  ([getzep.com/pricing](https://www.getzep.com/pricing)).
- **Cognee** ([docs.cognee.ai/changelog](https://docs.cognee.ai/changelog)):
  release pipeline now auto-publishes the `cognee-mcp` Docker image; 2026 work
  on search relevance and ingestion robustness.

### Documented complaints / gaps (with receipts)

- **Mem0**: "Mem0 stores memories, but doesn't learn user patterns" (Ask HN,
  Feb 2026, [news.ycombinator.com/item?id=46891715](https://news.ycombinator.com/item?id=46891715));
  "Mem0 thinks our 2023 conversation happened in 2026" (Apr 2026,
  [item 47961750](https://news.ycombinator.com/item?id=47961750), plus
  [aurra.us blog](https://aurra.us/blog/mem0-vs-aurra)); third-party eval put
  Mem0 at 60.6% vs 51.8% raw BM25 on a coding-memory set (HN comment,
  [algolia search](https://hn.algolia.com/api/v1/search?query=mem0&tags=comment)
BM25 alone got ~52%, i.e. fancy memory barely beat keyword
  search); the OSS stack pulls a Python/Node "heavy runtime" per a competing
  launcher's HN comment (same link).
- **Letta**: agent-managed memory costs tokens every turn and slows loops
  ([sureprompts.com walkthrough](https://sureprompts.com/blog/letta-memgpt-walkthrough));
  LoCoMo benchmark controversy with Mem0 (both claims disputed, Letta blog above).
- **Zep**: no free self-hosted edition of the cloud product (BYOC is
  (Enterprise-only), pricing page above; credit math is opaque (350-byte
  chunking rule); r/LLMDevs users note Graphiti requires running a graph DB
  ([reddit.com/r/LLMDevs/comments/1fq302p](https://www.reddit.com/r/LLMDevs/comments/1fq302p/zep_opensource_graph_memory_for_ai_apps/)).
- **LangMem**: no MCP story of its own; Python-only; effectively requires the
  LangGraph stack.
- **Supermemory**: consumer-app heritage; benchmark deltas cited against it
  (47.6% in the same third-party eval above).

## The local-first / MCP-native cluster (Recall-MCP's actual segment)

| Project | Approach | Storage | License | GitHub (2026-09-06) |
|---|---|---|---|---|
| [Palace](https://palacememory.com) | **Closest direct competitor.** Local-first memory for coding agents (Cursor, Claude Code, Codex) via `palace_*` MCP tools; semantic + BM25 + "diaries" + knowledge graph; local ONNX embeddings; air-gapped OK; JSON export | Local crate `palace-rs` (Rust); optional self-hosted Team Server | MIT core; paid license (Ed25519 keys) for server | [palacememory.com/pricing](https://palacememory.com/pricing): Free (1 dev), Team €1,490/yr (≤10), Growth €3,490/yr (≤30), Enterprise from €8k/yr |
| [OpenMemory MCP](https://mem0.ai/blog/introducing-openmemory-mcp) (Mem0) | Local MCP memory + dashboard; tools `add_memories`/`search_memory`/`list_memories`/`delete_all_memories` | Local Docker stack | OSS | Requires **Docker + an OpenAI API key**, setup friction is our wedge |
| [basic-memory](https://github.com/basicmachines-co/basic-memory) | MCP over local Markdown/Obsidian-compatible knowledge files + graph links | Local .md files | AGPL-3.0 | 3.9k★, Python, pushed 2026-09-06 |
| [mcp-memory-service](https://github.com/doobidoo/mcp-memory-service) | Persistent memory + knowledge graph + autonomous consolidation; remote MCP | SQLite (sqlite-vec) | Apache-2.0 | 1.9k★, Python, pushed 2026-09-06 |
| [mcp-local-memory](https://github.com/Beledarian/mcp-local-memory) | Local hybrid semantic search, SQLite FTS5 + knowledge graph (technically nearest to us) | SQLite FTS5 | OSS | small |
| [Cortex](https://github.com/gambletan/cortex) | Rust, local-first, **end-to-end-encrypted**, zero-telemetry memory engine, MCP; claims to beat Mem0 on LoCoMo | Local | MIT | 31★, right idea, no distribution (Show HN Mar 2026, 4 points) |
| [Memora](https://www.reddit.com/r/LocalLLaMA/comments/1pofkjk/built_a_localfirst_memory_server_for_mcp_clients/) | Self-hosted, 100% local SQLite-backed MCP memory server | SQLite | OSS | r/LocalLLaMA launch |
| [@modelcontextprotocol/server-memory](https://github.com/modelcontextprotocol/servers) | Official reference "memory" server: JSON-file knowledge graph | JSON file | MIT | The baseline everyone has tried and outgrown |
| [Supermemory](https://supermemory.ai/pricing/) | Memory API / "context engine"; OSS core can "run fully locally" (TS, Cloudflare/Postgres) | Hosted or local | MIT core | 29.2k★; Free ~$5 usage, Personal $19/mo, Max $100/mo, Scale custom |
| [memU](https://github.com/NevaMind-AI/memU) | "Personal memory across agents"; agent-dir/memory-filesystem metaphor; claude-skills/MCP topics | Local dir + hosted | "Other" (license not OSI-clear) | 14.4k★, fast-growing |
| [Memobase](https://github.com/memodb-io/memobase) | Profile-based long-term memory for chatbots | Self-host | Apache-2.0 | 2.9k★, last push 2026-01-11, **fading** |

Landscape survey of this cluster: ["13 Memory MCP Servers Compared (2026):
Local-First or Hosted"](https://mnemoverse.com/docs/library/memory-mcp-servers-compared).

## Table stakes in 2026 (what users now expect)

1. **MCP exposure**: every serious memory product ships MCP (Mem0 hosted MCP,
   Zep MCP seats, Cognee MCP image, Palace/OpenMemory/local servers).
2. **Namespacing / multi-agent scoping**: Mem0's `userId`/`agentId`/`runId`,
   Zep users/groups/projects, Palace team server. Our `workspace_id` matches;
   per-agent scoping within a workspace is the next increment.
3. **Forgetting / decay**: Mem0 Memory Decay, mcp-memory-service
   consolidation. We have recency decay; we lack deletion policy/consolidation.
4. **Graph memories**: everyone markets them, but Letta's filesystem result
   (74.0% LoCoMo) shows graphs are not automatically better. Treat as a
   checkbox (entity links), not a moat.
5. **Temporal validity**: Graphiti's temporal edges, Mem0 Temporal Reasoning;
   Mem0's 2023→2026 timestamp bug shows users care about correctness here.
6. **Privacy: encryption at rest / zero telemetry**: Cortex (E2E-encrypted),
   Palace (air-gapped, license keys verified locally), Zep (BYOK/BYOC),
   basic-memory (privacy-first). **Plaintext single-file storage is now a
   marketed weakness, not a neutral default.**
7. **Published evals**: LoCoMo/LongMemEval numbers are quoted in every HN
   launch. No evals = not credible; dishonest evals = public backlash.
8. **Portable export**, Palace advertises JSON export "no lock-in"; our
   export/import is already ahead of the hosted players.

## Gaps in the market, where Recall-MCP sits

Checked against every player above as of 2026-09-06:

- **Explainable retrieval**: nobody ships a per-result score decomposition.
  Mem0/Zep/Letta/Cognee all return black-box top-k. Our
  `{bm25, vector, recency, pinned_boost, total}` breakdown is unique in the
  category, and "deterministic + explainable" directly answers the #1
  complaint class (memory quality mystery).
- **MCP-native + local-first + zero-friction**: OpenMemory needs Docker + an
  OpenAI key; Palace's team features are paid and closed; Cortex has no
  distribution; the official server is a toy JSON file. A **single static Rust
  binary, one SQLite file, stdio or HTTP, no account, no API key required**
  is still an open niche, and a HN commenter explicitly complains that Mem0/
  Letta/Zep are "heavy runtimes (Python/Node)" versus local tools.
- **Honest, reproducible evals**: everyone quotes benchmarks; nobody publishes
  the harness. Our `evals/` golden set + hit-rate gate can become the trust
  brand ("reproducible in one command").
- **Explainability for agents, not just humans**: score breakdowns let the
  *agent* decide whether to trust/re-query, unexploited by anyone.

## Feature recommendations (ranked by product value)

1. **Encryption at rest for the memory file** (SQLCipher or OS keychain-wrapped
   key). *Gap*: Cortex markets E2E encryption; Palace markets air-gapped
   deployment; Zep sells BYOK, privacy is the buying reason for local-first,
   and we currently store plaintext. Low effort, high positioning value.
2. **Auto-capture / hook ingestion** (e.g., session-end JSON import + IDE
   plugins). *Gap*: Mem0's entire 2026 changelog is capture hooks (Kimi Code,
   DeepSeek, Strands) because manual `remember()` calls starve the store;
   every local competitor expects the agent to remember to remember.
3. **Self-hosted Team Server** (shared namespaces, sync, OIDC SSO, audit log).
   *Gap*: Palace charges €1,490/yr for exactly
   this; Zep's team features are cloud-only; nobody offers self-hosted team
   memory with permissive licensing.
4. **Memory update/consolidation** (edit, merge, dedup, contradiction
   supersede with temporal validity). *Gap*: Mem0 "doesn't learn user
   patterns" (HN) and misdates memories (HN); Letta's answer is to spend agent
   tokens. We can do deterministic, explainable consolidation, and our v0.1
   only has insert/delete today.
5. **Local ONNX embedding option** (fastembed-rs behind a feature flag).
   *Gap*: HashEmbedder is lexical-only; OpenMemory requires an OpenAI key
   (privacy/cost); Palace already ships local ONNX embeddings. Semantic
   quality without network or keys is the single biggest retrieval-quality
   lever we can buy off the shelf.
6. **Publish a reproducible benchmark** (LoCoMo subset + LongMemEval + our
   golden set, one command, CI-enforced). *Gap*: benchmark claims drive the
   whole category's HN discourse, and the two leaders' claims are mutually
   disputed; honest harness-as-marketing is open real estate.
7. **Per-agent scoping + MCP registry/distribution push** (skills/plugins for
   Claude Code, Cursor, Codex; list on MCP registries). *Gap*: distribution,
   not features, is why Cortex (31★) lost to Mem0 (64.8k★); Mem0's changelog
   shows the leaders buy growth one integration at a time.
8. **Entity links (lightweight graph) as an *explainable* overlay**: links
   stored and rendered in score breakdowns ("this memory matched because of
   edge X"), not a graph database. *Gap*: graphs are table-stakes marketing;
   Letta's filesystem result shows heavy graphs can underperform, which is an
   explainable, optional overlay threads the needle without Neo4j-style ops.

## Method note

Star counts and `pushed_at` from the GitHub REST API on 2026-09-06; pricing
from official pages same day; complaints from linked HN/Reddit threads. Web
search was rate-limited during research; Reddit coverage is thinner than HN;
revisit r/LocalLLaMA and r/MCP sentiment before the public launch.
