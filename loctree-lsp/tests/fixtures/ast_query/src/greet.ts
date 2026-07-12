// TypeScript fixture for ast_query integration tests (Plan 20).
// Two lexical declarations + a function declaration.
export const GREETING: string = "hello";
export const FAREWELL: string = "goodbye";

export function greet(name: string): string {
    return `${GREETING}, ${name}!`;
}
