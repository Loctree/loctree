// TSX fixture for ast_query integration tests (Plan 20).
// Mixes JSX with a lexical declaration so the auto-dispatch test sees
// every supported language at least once.
export const LABEL: string = "click";

export function Button() {
    return <button>{LABEL}</button>;
}
