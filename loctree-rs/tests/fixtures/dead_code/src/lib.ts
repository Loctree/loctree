// Library with dead exports

export function used(): string {
    return 'I am used';
}

// These are never imported anywhere
export function deadFunction(): string {
    return 'I am dead code';
}

export const DEAD_CONSTANT = 'never used';

export class DeadClass {
    method() {
        return 'dead';
    }
}
