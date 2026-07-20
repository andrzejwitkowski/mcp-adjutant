# LLM Config: Provider Profiles + Full Shell

**Date:** 2026-07-20  
**Branch / worktree:** `feat/llm-config-shell`  
**Approach:** Schema + shell together (true shared profiles)

## Goal

Replace the per-agent `LlmClientCatalog` stack with:

1. Shared **Provider Profiles** (provider type + base URL + API key)
2. **Agent phase cards** that reference a profile and set model / temp / max tokens
3. A **full app shell** (left sidebar replaces top nav)

## Decisions (locked)

| Topic | Choice |
| --- | --- |
| Layout | Full shell — sidebar + profiles + agent cards |
| Profile model | Profile = provider + URL + key; agent = profile_id + model + temp + tokens |
| Updates | True shared reference (edit key once → all linked agents) |
| Enable toggles | None in v1 |
| Nav | Sidebar replaces top nav |
| Web Fetcher tunables | Nested under Web Fetcher agent card |
| Provider enum | Keep `deep_seek \| open_router \| open_ai \| custom` (no Anthropic/Ollama types) |
| Deploy | Alias for Save (`PUT /api/config`) |

## Data model

```text
AdjutantConfig {
  profiles: Map<ProfileId, ProviderProfile>
  default_profile_id: ProfileId | null
  phases: Map<AgentPhase, PhaseBinding>
  server_port, storage_path, triage_overrides, web_fetcher  // unchanged
}

ProviderProfile {
  id: string
  name: string
  provider: Provider
  api_key: string | null
  base_url: string
}

PhaseBinding {
  profile_id: string
  model_name: string
  max_tokens: u32
  temperature: f32
}
```

### Runtime resolve

```text
resolve(phase) → PhaseProfile {
  provider, api_key, base_url  from profiles[binding.profile_id]
  model_name, max_tokens, temperature  from binding
}
```

`create_llm_client` / `create_llm_client_for_phase` keep taking `PhaseProfile`. One thin resolve helper on `AdjutantConfig`; no per-client rewrite.

### Migration (load)

1. If JSON already has `profiles` + phase bindings → use as-is (fill missing phases from defaults).
2. Else legacy flat `phases[phase] = { provider, api_key, base_url, model_name, max_tokens, temperature }`:
   - Dedupe identical `(provider, base_url, api_key)` into named profiles
   - Each phase becomes a `PhaseBinding` with that `profile_id` + model/temp/tokens
   - `default_profile_id` = most-referenced profile (tie → first)
3. Save writes **new shape only**. One release of legacy read is enough.

### Delete / default

- Cannot delete a profile while any phase references it (reassign to `default_profile_id` first, or block with error).
- “Set as Global Default” updates `default_profile_id`.
- New phases / missing bindings use `default_profile_id` + phase default model/temp/tokens.

## UI shell

### Sidebar destinations

| Item | Behavior |
| --- | --- |
| Overview | Stub or light status (may reuse Usage summary) |
| LLM Config | Profiles panel + agent cards (this feature) |
| Agent names under Config | Scroll/highlight card — not separate routes |
| Evaluations | Existing `#/evaluations` |
| Scout cache | Existing `#/cache` |
| Web cache | Existing `#/web-cache` |
| Usage | Existing `#/usage` |
| Logs | Stub or existing UI-notify surface |
| Deploy Changes | Save configuration |

Header on Config: breadcrumb `mcp-adjutant / LLM Config`, Reset, Save.

### Agent cards (all phases + icons)

| Phase | Title | Material icon |
| --- | --- | --- |
| scout | Scout | `search` |
| triage | Triage | `rule` |
| builder | Builder | `construction` |
| transformer | Transformer | `auto_fix_high` |
| evaluator | Evaluator | `grade` |
| log_analyzer | Log Analyzer | `terminal` |
| web_fetcher | Web Fetcher | `travel_explore` |
| babysitter | Babysitter | `childcare` |
| planner | Planner (scout) | `map` |
| planner_emit | Planner (emit) | `edit_note` |
| git_janitor | Git Janitor | `mop` |

No on/off toggle. Web Fetcher card expands to include existing `WebFetcherTunables`.

## Components (fewest files)

| File | Role |
| --- | --- |
| `AppShell.tsx` | Sidebar + outlet; replace top `NavBar` inside `PageShell` |
| `ProviderProfilesPanel.tsx` | Profile list + editor |
| `AgentPhaseCard.tsx` | Icon, profile select, model, temp, max tokens |
| `agents.ts` | Phase → title, hint, icon |
| `ConfigApp.tsx` | Load/save, compose panels |
| `types.ts` | Mirror new domain types |
| `src/domain.rs` | `ProviderProfile`, `PhaseBinding`, migrate + resolve |
| `src/llm/factory.rs` | Call resolve before `create_llm_client` |
| `config-ui.css` | Extend with mockup tokens (no Tailwind CDN) |

**Stop using as the Config UI:** per-phase `LlmClientCatalog` expansion. Keep `clients.ts` for provider defaults when creating profiles.

### YAGNI (out of v1)

- Enable toggles, Batch Edit, Sync Models, latency footer, Advanced Overrides
- New provider enum variants (Anthropic, Ollama)
- Full Overview/Logs product features
- New CSS framework / Material font hosting beyond a single icon font link if needed

## Testing

| Area | Cases |
| --- | --- |
| Rust (`tests/config_storage.rs` + domain unit) | Legacy migrate → profiles; resolve merges correctly; delete blocked when referenced |
| Frontend | Editing shared profile key updates all agents using that id; changing model on one card does not rewrite the profile key |

Builder required for new/changed logic-bearing source (`domain` resolve/migrate, React panels with behavior). Triage after edits.

## Implementation notes (ponytail / cove)

- Prefer extending existing hash routing in `App.tsx` over a router library.
- Prefer CSS variables in `config-ui.css` over new design-system packages.
- Mark intentional shortcuts with `ponytail:` (e.g. Overview stub).
- Cove: no verbose wrappers; logic stays, ceremony dies.

## Success criteria

- User can create/edit/delete provider profiles and assign them to any of the 11 agents.
- Changing a profile API key once affects every agent referencing it after Save.
- Existing config files without `profiles` load and save into the new shape.
- Sidebar navigates to Evaluations / caches / Usage; LLM Config shows profiles + cards with icons.
- Deploy/Save persists via existing `/api/config`.
