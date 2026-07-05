# loct find comparison: 0.8.8 vs 0.8.9-dev

Generated: 2026-01-20 07:00

## Binaries

- system: `/home/maciejgad/.local/bin/loct` (loctree 0.8.8)
- local: `/home/maciejgad/hosted/loctree/target/debug/loct` (loctree 0.8.9-dev)

## Symbol match deltas (10 queries)

| query | system matches | local matches | delta |
| --- | ---: | ---: | ---: |
| `filterTasksForReminder` | 0 | 0 | 0 |
| `ORB_DRAG_THRESHOLD` | 2 | 4 | 2 |
| `orbDragPending` | 0 | 0 | 0 |
| `RemindersPanel` | 2 | 4 | 2 |
| `startOrbDrag` | 0 | 0 | 0 |
| `stopOrbDrag` | 0 | 0 | 0 |
| `taskFilter` | 0 | 0 | 0 |
| `useAssistantAnchor` | 2 | 9 | 7 |
| `useWorkspaceRemindersPanel` | 2 | 4 | 2 |
| `workspaceTaskFilter` | 0 | 0 | 0 |

## Highlights

- `ORB_DRAG_THRESHOLD`: 2 → 4 (+2)
- `RemindersPanel`: 2 → 4 (+2)
- `useAssistantAnchor`: 2 → 9 (+7)
- `useWorkspaceRemindersPanel`: 2 → 4 (+2)

## Raw side-by-side output

Full output: `/home/maciejgad/hosted/loctree/reports/loct-find-compare-0.8.8-vs-0.8.9-dev.txt`

---
Example:
## 0.8.8
```
===== loct find: useAssistantAnchor =====
Search results for: useAssistantAnchor

=== Symbol Matches (2) ===
  File: src/contexts/anchor/AssistantAnchorContext.tsx
    [DEF] src/contexts/anchor/AssistantAnchorContext.tsx:933 - export function useAssistantAnchor
  File: src/contexts/anchor/index.ts
    [DEF] src/contexts/anchor/index.ts:33 - export reexport useAssistantAnchor

=== Semantic Matches (20) ===
  useAssistantAnchor (score: 1.00)
    in export in src/contexts/anchor/AssistantAnchorContext.tsx
  useAssistantAnchor (score: 1.00)
    in export in src/contexts/anchor/index.ts
  useAssistantActivity (score: 0.65)
    in export in src/features/ai-suite/hooks/useAssistantActivity.ts
  useAssistantActionRouter (score: 0.62)
    in export in src/features/ai-suite/assistant/hooks/index.ts
  useAssistantActionRouter (score: 0.62)
    in export in src/features/ai-suite/assistant/hooks/useAssistantCore.tsx
  useAssistantActionRouter (score: 0.62)
    in export in src/features/ai-suite/assistant/index.ts
  useAssistantPresence (score: 0.60)
    in export in src/contexts/AssistantPresenceContext.tsx
  useAssistantQuickActions (score: 0.58)
    in export in src/features/ai-suite/assistant/hooks/index.ts
  useAssistantQuickActions (score: 0.58)
    in export in src/features/ai-suite/assistant/hooks/useAssistantCore.tsx
  useAssistantQuickActions (score: 0.58)
    in export in src/features/ai-suite/assistant/index.ts
  emitAssistantAction (score: 0.58)
    in export in src/utils/assistantActions.ts
  emitAssistantActions (score: 0.55)
    in export in src/utils/assistantActions.ts
  useAssistantToolCommands (score: 0.54)
    in export in src/features/ai-suite/ui/hooks/index.ts
  useAssistantToolCommands (score: 0.54)
    in export in src/features/ai-suite/ui/hooks/useAssistantToolCommands.ts
  useAssistantToolCommands (score: 0.54)
    in export in src/features/ai-suite/ui/index.ts
  AssistantAnchorProvider (score: 0.52)
    in export in src/contexts/anchor/AssistantAnchorContext.tsx
  AssistantAnchorProvider (score: 0.52)
    in export in src/contexts/anchor/index.ts
  useAssistantHostTelemetry (score: 0.52)
    in export in src/features/ai-suite/assistant/hooks/index.ts
  useAssistantHostTelemetry (score: 0.52)
    in export in src/features/ai-suite/assistant/hooks/useAssistantCore.tsx
  useAssistantHostTelemetry (score: 0.52)
    in export in src/features/ai-suite/assistant/index.ts

=== Dead Code Status ===
  OK: Symbol is used.

[SYSTEM_0-8-8][TIME] loct find "useAssistantAnchor" -> 0s
```
## 0.8.9-dev
```bash
===== loct find: useAssistantAnchor =====
Search results for: useAssistantAnchor

=== Symbol Matches (9) ===
  File: src/contexts/__tests__/AssistantAnchorContext.test.tsx
    [DEF] src/contexts/__tests__/AssistantAnchorContext.test.tsx:0 - import useAssistantAnchor from @/contexts/anchor
  File: src/contexts/anchor/AssistantAnchorContext.tsx
    [DEF] src/contexts/anchor/AssistantAnchorContext.tsx:933 - export function useAssistantAnchor
  File: src/contexts/anchor/index.ts
    [DEF] src/contexts/anchor/index.ts:33 - export reexport useAssistantAnchor
  File: src/features/ai-suite/floating/__tests__/AssistantAnchorLayout.test.tsx
    [DEF] src/features/ai-suite/floating/__tests__/AssistantAnchorLayout.test.tsx:0 - import useAssistantAnchor from @/contexts/anchor
  File: src/features/ai-suite/floating/hooks/useChatCore/useChatPanelLayout.ts
    [DEF] src/features/ai-suite/floating/hooks/useChatCore/useChatPanelLayout.ts:0 - import useAssistantAnchor from @/contexts/anchor
  File: src/features/ai-suite/hosts/__tests__/AssistantHostManager.test.tsx
    [DEF] src/features/ai-suite/hosts/__tests__/AssistantHostManager.test.tsx:0 - import useAssistantAnchor from @/contexts/anchor
  File: src/features/ai-suite/hosts/AIFloatingHost.tsx
    [DEF] src/features/ai-suite/hosts/AIFloatingHost.tsx:0 - import useAssistantAnchor from @/contexts/anchor
  File: src/features/ai-suite/hosts/AssistantHostManager.tsx
    [DEF] src/features/ai-suite/hosts/AssistantHostManager.tsx:0 - import useAssistantAnchor from @/contexts/anchor
  File: src/features/ai-suite/orb/components/AIFloatingOrb.tsx
    [DEF] src/features/ai-suite/orb/components/AIFloatingOrb.tsx:0 - import useAssistantAnchor from @/contexts/anchor

=== Semantic Matches (20) ===
  useAssistantAnchor (score: 1.00)
    in export in src/contexts/anchor/AssistantAnchorContext.tsx
  useAssistantAnchor (score: 1.00)
    in export in src/contexts/anchor/index.ts
  useAssistantActivity (score: 0.65)
    in export in src/features/ai-suite/hooks/useAssistantActivity.ts
  useAssistantActionRouter (score: 0.62)
    in export in src/features/ai-suite/assistant/hooks/index.ts
  useAssistantActionRouter (score: 0.62)
    in export in src/features/ai-suite/assistant/hooks/useAssistantCore.tsx
  useAssistantActionRouter (score: 0.62)
    in export in src/features/ai-suite/assistant/index.ts
  useAssistantPresence (score: 0.60)
    in export in src/contexts/AssistantPresenceContext.tsx
  useAssistantQuickActions (score: 0.58)
    in export in src/features/ai-suite/assistant/hooks/index.ts
  useAssistantQuickActions (score: 0.58)
    in export in src/features/ai-suite/assistant/hooks/useAssistantCore.tsx
  useAssistantQuickActions (score: 0.58)
    in export in src/features/ai-suite/assistant/index.ts
  emitAssistantAction (score: 0.58)
    in export in src/utils/assistantActions.ts
  emitAssistantActions (score: 0.55)
    in export in src/utils/assistantActions.ts
  useAssistantToolCommands (score: 0.54)
    in export in src/features/ai-suite/ui/hooks/index.ts
  useAssistantToolCommands (score: 0.54)
    in export in src/features/ai-suite/ui/hooks/useAssistantToolCommands.ts
  useAssistantToolCommands (score: 0.54)
    in export in src/features/ai-suite/ui/index.ts
  AssistantAnchorProvider (score: 0.52)
    in export in src/contexts/anchor/AssistantAnchorContext.tsx
  AssistantAnchorProvider (score: 0.52)
    in export in src/contexts/anchor/index.ts
  useAssistantHostTelemetry (score: 0.52)
    in export in src/features/ai-suite/assistant/hooks/index.ts
  useAssistantHostTelemetry (score: 0.52)
    in export in src/features/ai-suite/assistant/hooks/useAssistantCore.tsx
  useAssistantHostTelemetry (score: 0.52)
    in export in src/features/ai-suite/assistant/index.ts

=== Dead Code Status ===
  OK: Symbol is used.

[LOCAL_0-8-9-DEV][TIME] loct find "useAssistantAnchor" -> 3s
```
## rg
```bash
rg -n "useAssistantAnchor" -g '*.{ts,tsx}' src
src/contexts/anchor/index.ts
35:  useAssistantAnchor,

src/contexts/__tests__/AssistantAnchorContext.test.tsx
4:import { AssistantAnchorProvider, useAssistantAnchor } from '@/contexts/anchor';
36:  const anchorRef: { current: ReturnType<typeof useAssistantAnchor> | null } = {
40:    const value = useAssistantAnchor();

src/contexts/anchor/AssistantAnchorContext.tsx
933:export function useAssistantAnchor(): AssistantAnchorValue {
937:      'useAssistantAnchor must be used within AssistantAnchorProvider'

src/features/ai-suite/orb/components/AIFloatingOrb.tsx
17:import { ASSISTANT_ORB_SIZE, useAssistantAnchor } from '@/contexts/anchor';
55:  } = useAssistantAnchor();

src/features/ai-suite/hosts/__tests__/AssistantHostManager.test.tsx
8:import { useAssistantAnchor } from '@/contexts/anchor';
56:  useAssistantAnchor: vi.fn(),
78:const mockedUseAssistantAnchor = vi.mocked(useAssistantAnchor);

src/features/ai-suite/hosts/AIFloatingHost.tsx
22:  useAssistantAnchor,
47:  const { startPanelDrag } = useAssistantAnchor();

src/features/ai-suite/floating/hooks/useChatCore/useChatPanelLayout.ts
15:  useAssistantAnchor,
60:  } = useAssistantAnchor();

src/features/ai-suite/hosts/AssistantHostManager.tsx
10:import { useAssistantAnchor } from '@/contexts/anchor';
21:  const { setAutoHomeSuspended } = useAssistantAnchor();

src/features/ai-suite/floating/__tests__/AssistantAnchorLayout.test.tsx
3:import { AssistantAnchorProvider, useAssistantAnchor } from '@/contexts/anchor';
7:      const anchor = useAssistantAnchor();
```
---
## Pełna interpretacja:

  - W 0.8.8 loct find widzi tylko definicje + re-exporty (2).
  - W 0.8.9-dev widzi definicje + re-exporty + import-usage (9), czyli to, co pokazuje rg.
  - rg widzi wszystkie wystąpienia tekstu (definicje + importy + użycia + testy), czyli w porównaniu do 0.8.8 daje pełny obraz usage, ale nie mówi o typie wystąpienia (DEF/import/usage) ani odead‑code. W porównaniu do 0.8.9‑dev daje podobny zestaw plików, ale bez kontekstu semantycznego (brak klasyfikacji, brak relacji i brak statusu użycia).
  - 0.8.8 pokazuje tylko publiczną powierzchnię (definicje + re-exporty), i w porównaniu do rg nie daje pełnej listy wystąpień/usage, za to daje semantyczne dopasowania i status dead‑code.
  - 0.8.9‑dev pokazuje definicje + re‑exporty + import‑usage, i w porównaniu do rg daje klasyfikację (co jest DEF vs import) oraz status użycia, ale nadal nie pokazuje wszystkich lokalnych usage w treści pliku (np. wywołań funkcji), jeśli nie przechodzą przez import/eksport.